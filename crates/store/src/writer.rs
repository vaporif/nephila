//! Single-writer-thread for the SQLite database.
//!
//! Per ADR-0002, the writer owns a typed command enum rather than accepting
//! an arbitrary `FnOnce`. This lets the writer thread own the lazy
//! `next_sequence` cache directly — keyed off
//! `(aggregate_type, aggregate_id)` — so `append_batch` can stamp sequences
//! inside the same SQLite transaction as the row INSERTs.
//!
//! Generic execution still works through `WriterCmd::Generic`, which the
//! older stores (agent CRUD, checkpoint CRUD, etc.) continue to use via
//! `WriterHandle::execute`.

use crate::blob::PreparedBlob;
use crate::subscribe::BroadcastRegistry;
use nephila_eventsourcing::envelope::EventEnvelope;
use nephila_eventsourcing::store::EventStoreError;
use rusqlite::{Connection, params};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc;
use tokio::sync::oneshot;

type GenericFunc = Box<dyn FnOnce(&mut Connection) + Send>;

/// Typed command sent to the writer thread.
pub(crate) enum WriterCmd {
    Generic {
        func: GenericFunc,
    },
    Append {
        envelopes: Vec<EventEnvelope>,
        blobs: Vec<PreparedBlob>,
        reply: oneshot::Sender<Result<Vec<u64>, EventStoreError>>,
    },
    HeadSequence {
        agg_type: String,
        agg_id: String,
        reply: oneshot::Sender<Result<u64, EventStoreError>>,
    },
    Prune {
        agg_type: String,
        agg_id: String,
        before: u64,
        reply: oneshot::Sender<Result<u64, EventStoreError>>,
    },
}

#[derive(Clone)]
pub struct WriterHandle {
    tx: mpsc::Sender<WriterCmd>,
    broadcasts: Arc<BroadcastRegistry>,
}

impl WriterHandle {
    pub fn new(conn: Connection) -> Self {
        Self::new_with_broadcasts(conn, Arc::new(BroadcastRegistry::default()))
    }

    pub(crate) fn new_with_broadcasts(
        conn: Connection,
        broadcasts: Arc<BroadcastRegistry>,
    ) -> Self {
        // mmap/cache/temp_store/wal_autocheckpoint/synchronous. mmap and WAL
        // pragmas are no-ops on in-memory; safe to apply universally.
        if let Err(e) = crate::schema::apply_tuning_pragmas(&conn) {
            tracing::warn!(%e, "writer pragma application failed");
        }
        let (tx, rx) = mpsc::channel::<WriterCmd>();
        let bcasts = broadcasts.clone();
        std::thread::spawn(move || writer_loop(conn, rx, bcasts));
        Self { tx, broadcasts }
    }

    pub(crate) fn broadcasts(&self) -> Arc<BroadcastRegistry> {
        self.broadcasts.clone()
    }

    pub async fn execute<F, R>(&self, func: F) -> Result<R, crate::StoreError>
    where
        F: FnOnce(&Connection) -> Result<R, rusqlite::Error> + Send + 'static,
        R: Send + 'static,
    {
        let (resp_tx, resp_rx) = oneshot::channel();
        let cmd = WriterCmd::Generic {
            func: Box::new(move |conn| {
                let result = func(conn);
                let _ = resp_tx.send(result);
            }),
        };
        self.tx
            .send(cmd)
            .map_err(|_| crate::StoreError::WriterClosed)?;
        resp_rx
            .await
            .map_err(|_| crate::StoreError::WriterClosed)?
            .map_err(crate::StoreError::Sqlite)
    }

    pub(crate) async fn append_batch(
        &self,
        envelopes: Vec<EventEnvelope>,
        blobs: Vec<PreparedBlob>,
    ) -> Result<Vec<u64>, EventStoreError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(WriterCmd::Append {
                envelopes,
                blobs,
                reply,
            })
            .map_err(|_| EventStoreError::Storage("writer closed".into()))?;
        rx.await
            .map_err(|_| EventStoreError::Storage("writer reply dropped".into()))?
    }

    pub(crate) async fn head_sequence(
        &self,
        agg_type: &str,
        agg_id: &str,
    ) -> Result<u64, EventStoreError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(WriterCmd::HeadSequence {
                agg_type: agg_type.into(),
                agg_id: agg_id.into(),
                reply,
            })
            .map_err(|_| EventStoreError::Storage("writer closed".into()))?;
        rx.await
            .map_err(|_| EventStoreError::Storage("writer reply dropped".into()))?
    }

    pub(crate) async fn prune(
        &self,
        agg_type: &str,
        agg_id: &str,
        before: u64,
    ) -> Result<u64, EventStoreError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(WriterCmd::Prune {
                agg_type: agg_type.into(),
                agg_id: agg_id.into(),
                before,
                reply,
            })
            .map_err(|_| EventStoreError::Storage("writer closed".into()))?;
        rx.await
            .map_err(|_| EventStoreError::Storage("writer reply dropped".into()))?
    }
}

fn writer_loop(
    mut conn: Connection,
    rx: mpsc::Receiver<WriterCmd>,
    broadcasts: Arc<BroadcastRegistry>,
) {
    let mut next_sequence: HashMap<(String, String), u64> = HashMap::new();
    while let Ok(cmd) = rx.recv() {
        match cmd {
            WriterCmd::Generic { func } => {
                func(&mut conn);
            }
            WriterCmd::Append {
                envelopes,
                blobs,
                reply,
            } => {
                let result = handle_append(&mut conn, envelopes, blobs, &mut next_sequence);
                let assigned_for_publish = match &result {
                    Ok((seqs, envs)) => Some((seqs.clone(), envs.clone())),
                    Err(_) => None,
                };
                let send_result = result.map(|(seqs, _)| seqs);
                if let Some((_seqs, envs)) = assigned_for_publish {
                    for env in envs {
                        let sender = broadcasts.sender_for(&env.aggregate_type, &env.aggregate_id);
                        let _ = sender.send(env);
                    }
                }
                let _ = reply.send(send_result);
            }
            WriterCmd::HeadSequence {
                agg_type,
                agg_id,
                reply,
            } => {
                let result: Result<u64, EventStoreError> = (|| {
                    let max: i64 = conn
                        .query_row(
                            "SELECT COALESCE(MAX(sequence), 0) FROM domain_events
                             WHERE aggregate_type = ?1 AND aggregate_id = ?2",
                            params![&agg_type, &agg_id],
                            |row| row.get(0),
                        )
                        .map_err(|e| EventStoreError::Storage(e.to_string()))?;
                    Ok(max as u64)
                })();
                let _ = reply.send(result);
            }
            WriterCmd::Prune {
                agg_type,
                agg_id,
                before,
                reply,
            } => {
                let result: Result<u64, EventStoreError> = conn
                    .execute(
                        "DELETE FROM domain_events
                         WHERE aggregate_type = ?1 AND aggregate_id = ?2 AND sequence < ?3",
                        params![&agg_type, &agg_id, before as i64],
                    )
                    .map(|n| n as u64)
                    .map_err(|e| EventStoreError::Storage(e.to_string()));
                if result.is_ok() {
                    // Drop cached next_sequence — pruning may invalidate the
                    // lazy cache when ALL rows are deleted; the next append
                    // will rewarm from MAX(=0).
                    next_sequence.remove(&(agg_type.clone(), agg_id.clone()));
                }
                let _ = reply.send(result);
            }
        }
    }
}

#[allow(clippy::type_complexity)]
fn handle_append(
    conn: &mut Connection,
    mut envelopes: Vec<EventEnvelope>,
    blobs: Vec<PreparedBlob>,
    next_sequence: &mut HashMap<(String, String), u64>,
) -> Result<(Vec<u64>, Vec<EventEnvelope>), EventStoreError> {
    let tx = conn
        .transaction()
        .map_err(|e| EventStoreError::Storage(e.to_string()))?;

    for blob in &blobs {
        tx.execute(
            "INSERT OR IGNORE INTO blobs (hash, payload, original_len) VALUES (?1, ?2, ?3)",
            params![&blob.hash, &blob.payload, blob.original_len as i64],
        )
        .map_err(|e| EventStoreError::Storage(e.to_string()))?;
    }

    // Cache writes are deferred until AFTER `tx.commit()` succeeds. If the
    // commit fails (disk full, SQLITE_BUSY, WAL stall), the durable MAX
    // hasn't moved, so the in-memory cache must NOT move either; otherwise
    // the next append would issue `stale_max + 1` and either trip the
    // UNIQUE constraint or silently leave a permanent gap. Same-batch
    // lookahead is served from `batch_max` so envelopes sharing an
    // aggregate within one batch see consistent monotonic sequences without
    // touching the durable cache.
    let mut assigned = Vec::with_capacity(envelopes.len());
    let mut batch_max: HashMap<(String, String), u64> = HashMap::new();
    for env in &mut envelopes {
        debug_assert!(
            env.sequence == 0,
            "EventEnvelope::sequence must be 0 on append; writer stamps it (ADR-0002)"
        );
        let key = (env.aggregate_type.clone(), env.aggregate_id.clone());
        let next = match batch_max.get(&key) {
            Some(&n) => n + 1,
            None => match next_sequence.get(&key) {
                Some(&n) => n + 1,
                None => {
                    let from_disk: i64 = tx
                        .query_row(
                            "SELECT COALESCE(MAX(sequence), 0) FROM domain_events
                             WHERE aggregate_type = ?1 AND aggregate_id = ?2",
                            params![&env.aggregate_type, &env.aggregate_id],
                            |r| r.get(0),
                        )
                        .map_err(|e| EventStoreError::Storage(e.to_string()))?;
                    from_disk as u64 + 1
                }
            },
        };
        env.sequence = next;
        batch_max.insert(key, next);
        assigned.push(next);

        let id = env.id.0.to_string();
        let payload = serde_json::to_string(&env.payload)
            .map_err(|e| EventStoreError::Serialization(e.to_string()))?;
        let outcome = serde_json::to_string(&env.outcome)
            .map_err(|e| EventStoreError::Serialization(e.to_string()))?;
        let context_snapshot = serde_json::to_string(&env.context_snapshot)
            .map_err(|e| EventStoreError::Serialization(e.to_string()))?;
        let metadata = serde_json::to_string(&env.metadata)
            .map_err(|e| EventStoreError::Serialization(e.to_string()))?;
        let timestamp = env.timestamp.to_rfc3339();

        tx.execute(
            "INSERT INTO domain_events (id, aggregate_type, aggregate_id, sequence, event_type, payload, trace_id, outcome, timestamp, context_snapshot, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                id,
                &env.aggregate_type,
                &env.aggregate_id,
                env.sequence as i64,
                &env.event_type,
                payload,
                &env.trace_id.0,
                outcome,
                timestamp,
                context_snapshot,
                metadata,
            ],
        )
        .map_err(|e| EventStoreError::Storage(e.to_string()))?;
    }

    // Test seam: simulate a commit failure to exercise the cache-rollback
    // path. The seam fires AFTER all per-row INSERTs but BEFORE the durable
    // `tx.commit()`, so the failure is indistinguishable from a real disk /
    // WAL commit error from the perspective of `next_sequence`.
    #[cfg(any(test, feature = "test-seam"))]
    if FORCE_COMMIT_FAILURE.load(std::sync::atomic::Ordering::SeqCst) {
        FORCE_COMMIT_FAILURE.store(false, std::sync::atomic::Ordering::SeqCst);
        return Err(EventStoreError::Storage("forced commit failure".into()));
    }

    tx.commit()
        .map_err(|e| EventStoreError::Storage(e.to_string()))?;

    // Commit succeeded; promote per-batch monotonic state into the durable
    // writer-thread cache so subsequent appends start from this max.
    for (k, v) in batch_max {
        next_sequence.insert(k, v);
    }

    Ok((assigned, envelopes))
}

/// Test seam: forces the next `handle_append` invocation to abort with a
/// `Storage("forced commit failure")` after staging row INSERTs but before
/// the durable `tx.commit()`. Single-shot — auto-resets on use.
#[cfg(any(test, feature = "test-seam"))]
pub static FORCE_COMMIT_FAILURE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema;

    #[tokio::test]
    async fn insert_and_query_back() {
        schema::register_sqlite_vec();
        let conn = Connection::open_in_memory().unwrap();
        schema::init_db(&conn).unwrap();
        schema::init_vec_tables(&conn, 384).unwrap();
        let writer = WriterHandle::new(conn);

        writer
            .execute(|conn| {
                let now = chrono::Utc::now().to_rfc3339();
                conn.execute(
                    "INSERT INTO agents (id, state, directive, directory, objective_id, spawn_origin_type, created_at, updated_at)
                     VALUES ('agent-1', 'starting', 'continue', '/tmp', 'obj-1', 'operator', ?1, ?2)",
                    rusqlite::params![&now, &now],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let count: i64 = writer
            .execute(|conn| conn.query_row("SELECT count(*) FROM agents", [], |row| row.get(0)))
            .await
            .unwrap();

        assert_eq!(count, 1);
    }
}

//! `DomainEventStore` impl on `SqliteStore`.
//!
//! Slice 1b (ADR-0002): the writer thread now stamps sequences inside the
//! INSERT transaction. `append_batch` is the new typed entry point;
//! `append` is preserved for back-compat — it routes to `append_batch`
//! with a single envelope.

use crate::SqliteStore;
use crate::blob::PreparedBlob;
use crate::metrics;
#[cfg(any(test, feature = "test-seam"))]
use crate::subscribe::SubscribeAfterHooks;
use crate::util::parse_rfc3339;
use chrono::{DateTime, Utc};
use futures::Stream;
use nephila_eventsourcing::envelope::EventEnvelope;
use nephila_eventsourcing::id::{EventId, TraceId};
use nephila_eventsourcing::outcome::Outcome;
use nephila_eventsourcing::snapshot::{ContextSnapshot, Snapshot};
use nephila_eventsourcing::store::{DomainEventStore, EventStoreError, EventStream};
use std::collections::HashMap;
use std::pin::Pin;
use tokio::sync::broadcast;
use tracing::info_span;

impl DomainEventStore for SqliteStore {
    async fn append(&self, envelope: &EventEnvelope) -> Result<(), EventStoreError> {
        // Single-envelope append routes through append_batch so sequence
        // stamping is consistent. Caller-supplied non-zero sequence is
        // overwritten in release builds; debug-asserted in debug builds.
        let env = envelope.clone();
        self.append_batch(vec![env]).await.map(|_| ())
    }

    async fn append_batch(
        &self,
        envelopes: Vec<EventEnvelope>,
    ) -> Result<Vec<u64>, EventStoreError> {
        if envelopes.is_empty() {
            return Ok(Vec::new());
        }
        let len = envelopes.len() as u64;
        let agg_type = envelopes[0].aggregate_type.clone();
        let agg_id = envelopes[0].aggregate_id.clone();
        let seqs = self.writer.append_batch(envelopes, Vec::new()).await?;
        metrics::record_append_batch_size(&agg_type, &agg_id, len);
        Ok(seqs)
    }

    async fn load_events(
        &self,
        aggregate_type: &str,
        aggregate_id: &str,
        since_sequence: u64,
    ) -> Result<Vec<EventEnvelope>, EventStoreError> {
        let agg_type = aggregate_type.to_string();
        let agg_id = aggregate_id.to_string();
        let envelopes = self
            .writer
            .execute(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, aggregate_type, aggregate_id, sequence, event_type, payload, trace_id, outcome, timestamp, context_snapshot, metadata
                     FROM domain_events
                     WHERE aggregate_type = ?1 AND aggregate_id = ?2 AND sequence > ?3
                     ORDER BY sequence ASC",
                )?;
                let rows = stmt
                    .query_map(
                        rusqlite::params![agg_type, agg_id, since_sequence as i64],
                        row_to_envelope,
                    )?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await
            .map_err(|e| EventStoreError::Storage(e.to_string()))?;
        Ok(envelopes)
    }

    async fn load_by_trace_id(
        &self,
        trace_id: &TraceId,
    ) -> Result<Vec<EventEnvelope>, EventStoreError> {
        let tid = trace_id.0.clone();
        let envelopes = self
            .writer
            .execute(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, aggregate_type, aggregate_id, sequence, event_type, payload, trace_id, outcome, timestamp, context_snapshot, metadata
                     FROM domain_events
                     WHERE trace_id = ?1
                     ORDER BY timestamp ASC",
                )?;
                let rows = stmt
                    .query_map(rusqlite::params![tid], row_to_envelope)?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await
            .map_err(|e| EventStoreError::Storage(e.to_string()))?;
        Ok(envelopes)
    }

    async fn load_by_time_range(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<EventEnvelope>, EventStoreError> {
        let from_str = from.to_rfc3339();
        let to_str = to.to_rfc3339();
        let envelopes = self
            .writer
            .execute(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, aggregate_type, aggregate_id, sequence, event_type, payload, trace_id, outcome, timestamp, context_snapshot, metadata
                     FROM domain_events
                     WHERE timestamp >= ?1 AND timestamp <= ?2
                     ORDER BY timestamp ASC",
                )?;
                let rows = stmt
                    .query_map(rusqlite::params![from_str, to_str], row_to_envelope)?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await
            .map_err(|e| EventStoreError::Storage(e.to_string()))?;
        Ok(envelopes)
    }

    async fn save_snapshot(&self, snapshot: &Snapshot) -> Result<(), EventStoreError> {
        let aggregate_type = snapshot.aggregate_type.clone();
        let aggregate_id = snapshot.aggregate_id.clone();
        let sequence = snapshot.sequence;
        let state = serde_json::to_string(&snapshot.state)
            .map_err(|e| EventStoreError::Serialization(e.to_string()))?;
        let timestamp = snapshot.timestamp.to_rfc3339();

        self.writer
            .execute(move |conn| {
                conn.execute(
                    "INSERT OR REPLACE INTO aggregate_snapshots (aggregate_type, aggregate_id, sequence, state, timestamp)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![aggregate_type, aggregate_id, sequence as i64, state, timestamp],
                )?;
                Ok(())
            })
            .await
            .map_err(|e| EventStoreError::Storage(e.to_string()))?;
        Ok(())
    }

    async fn load_latest_snapshot(
        &self,
        aggregate_type: &str,
        aggregate_id: &str,
    ) -> Result<Option<Snapshot>, EventStoreError> {
        let agg_type = aggregate_type.to_string();
        let agg_id = aggregate_id.to_string();
        let snapshot = self
            .writer
            .execute(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT aggregate_type, aggregate_id, sequence, state, timestamp
                     FROM aggregate_snapshots
                     WHERE aggregate_type = ?1 AND aggregate_id = ?2
                     ORDER BY sequence DESC
                     LIMIT 1",
                )?;
                let mut rows =
                    stmt.query_map(rusqlite::params![agg_type, agg_id], row_to_snapshot)?;
                match rows.next() {
                    Some(row) => Ok(Some(row?)),
                    None => Ok(None),
                }
            })
            .await
            .map_err(|e| EventStoreError::Storage(e.to_string()))?;
        Ok(snapshot)
    }

    async fn subscribe_after(
        &self,
        aggregate_type: &str,
        aggregate_id: &str,
        since_sequence: u64,
    ) -> Result<EventStream, EventStoreError> {
        self.subscribe_after_inner(aggregate_type, aggregate_id, since_sequence, None)
            .await
    }

    async fn prune_aggregate(
        &self,
        aggregate_type: &str,
        aggregate_id: &str,
        before_sequence: u64,
    ) -> Result<u64, EventStoreError> {
        self.writer
            .prune(aggregate_type, aggregate_id, before_sequence)
            .await
    }
}

impl SqliteStore {
    /// Internal `subscribe_after` implementation used by both the trait method
    /// (production, no hook) and the gated `subscribe_after_with_hooks` (tests).
    /// `pre_head_snapshot` is an optional callback invoked between the listener
    /// attach and the head-sequence snapshot — the synchronisation seam used
    /// to deterministically race appends against the head snapshot in tests.
    async fn subscribe_after_inner(
        &self,
        aggregate_type: &str,
        aggregate_id: &str,
        since_sequence: u64,
        pre_head_snapshot: Option<&(dyn Fn() + Send + Sync)>,
    ) -> Result<EventStream, EventStoreError> {
        let span = info_span!(
            "subscribe_after",
            aggregate_type = %aggregate_type,
            aggregate_id = %aggregate_id,
            since_sequence = since_sequence,
        );
        let _enter = span.enter();

        // 1. Attach listener FIRST.
        let listener = self
            .writer
            .broadcasts()
            .sender_for(aggregate_type, aggregate_id)
            .subscribe();

        // Test seam: sleep / synchronise between listener-attach and head-snapshot.
        if let Some(f) = pre_head_snapshot {
            f();
        }

        // 2. Snapshot head AFTER listener attached.
        let head = self
            .writer
            .head_sequence(aggregate_type, aggregate_id)
            .await?;

        // 3. Backfill via the read pool (NOT the writer thread).
        let backfill = self
            .load_events_from_pool(aggregate_type, aggregate_id, since_sequence, head)
            .await?;
        let backfill_len = backfill.len() as u64;
        metrics::record_backfill_rows(aggregate_type, aggregate_id, backfill_len);
        metrics::record_head_lag(
            aggregate_type,
            aggregate_id,
            head.saturating_sub(since_sequence),
        );

        // 4. Spawn the snapshot trigger B (consumer-side, fire-and-forget).
        self.maybe_spawn_snapshot_task(aggregate_type.to_owned(), aggregate_id.to_owned(), head);

        // 5. Merge: yield backfill in order, then listener with seq > head, deduped.
        let stream = build_stream(backfill, listener, head);
        Ok(Box::pin(stream))
    }

    /// `subscribe_after` with test-only hooks injected between listener-attach
    /// and head-snapshot. Gated behind `cfg(any(test, feature = "test-seam"))`
    /// so it is absent from release builds (ADR-0002 / plan step 13).
    /// Production callers go through the trait method.
    #[cfg(any(test, feature = "test-seam"))]
    pub async fn subscribe_after_with_hooks(
        &self,
        aggregate_type: &str,
        aggregate_id: &str,
        since_sequence: u64,
        hooks: SubscribeAfterHooks,
    ) -> Result<EventStream, EventStoreError> {
        // Pull the hook out of the Arc. The lifetime of the trait object
        // borrowed from `hooks.pre_head_snapshot` is bound by the hooks
        // value, which lives until this `await` returns — long enough for
        // `subscribe_after_inner` to invoke it.
        match hooks.pre_head_snapshot.as_ref() {
            Some(arc) => {
                let f: &(dyn Fn() + Send + Sync) = arc.as_ref();
                self.subscribe_after_inner(aggregate_type, aggregate_id, since_sequence, Some(f))
                    .await
            }
            None => {
                self.subscribe_after_inner(aggregate_type, aggregate_id, since_sequence, None)
                    .await
            }
        }
    }

    /// Trigger B (consumer-side): if no snapshot exists or the latest
    /// snapshot lags behind by more than `SNAPSHOT_INTERVAL`, spawn a
    /// background task that replays the aggregate and writes a fresh
    /// snapshot. Fire-and-forget — the consumer's stream is not blocked.
    /// Per-aggregate `DashMap` prevents thundering-herd snapshotting.
    ///
    /// Trigger A (producer-side, on `SessionEnded` in
    /// `crates/connector/src/session.rs`) is deferred to the slice-1b
    /// integration window — see Task 3 step 12 in
    /// `docs/plans/2026-05-03-claude-session-streaming.md`.
    fn maybe_spawn_snapshot_task(&self, agg_type: String, agg_id: String, head_at_subscribe: u64) {
        // Only sessions get snapshot trigger B in slice 1b. Other aggregates
        // continue to manage snapshots externally.
        if agg_type != "session" {
            return;
        }
        let key = (agg_type.clone(), agg_id.clone());
        // Atomically reserve the slot; bail if another task is already
        // snapshotting this aggregate.
        if self.snapshot_locks.insert(key.clone(), ()).is_some() {
            return;
        }
        // Construct the drop guard BEFORE `tokio::spawn` so it is moved into
        // the spawned future already armed. If the closure panics on its
        // first poll (or even before, during runtime worker pickup), the
        // guard's `Drop` still fires and releases the lock — preventing a
        // permanent leak that would suppress all future snapshot triggers
        // for this aggregate.
        let guard = SnapshotLockGuard {
            locks: self.snapshot_locks.clone(),
            key,
        };
        let store = self.clone();
        tokio::spawn(async move {
            let _drop_guard = guard;
            if let Err(e) =
                run_session_snapshot_task(&store, &agg_type, &agg_id, head_at_subscribe).await
            {
                tracing::warn!(
                    target: "nephila_store::snapshot",
                    aggregate_type = %agg_type,
                    aggregate_id = %agg_id,
                    error = %e,
                    "snapshot trigger B task failed",
                );
            }
        });
    }

    /// Public for tests; not part of the stable surface area.
    #[doc(hidden)]
    pub async fn load_events_from_pool(
        &self,
        agg_type: &str,
        agg_id: &str,
        since_exclusive: u64,
        head_inclusive: u64,
    ) -> Result<Vec<EventEnvelope>, EventStoreError> {
        debug_assert!(since_exclusive <= head_inclusive);
        let pool = self.read_pool.clone();
        let agg_type = agg_type.to_owned();
        let agg_id = agg_id.to_owned();
        tokio::task::spawn_blocking(move || -> Result<Vec<EventEnvelope>, EventStoreError> {
            // RAII-guarded acquisition: the connection returns to the pool
            // on drop even if the closure panics partway through (e.g.,
            // OOM, FFI assert in rusqlite). Without this guard, a panic
            // would permanently leak the connection.
            let conn = pool
                .acquire_guarded()
                .map_err(|_| EventStoreError::PoolExhausted)?;
            let mut stmt = conn.prepare_cached(
                "SELECT id, aggregate_type, aggregate_id, sequence, event_type, payload, trace_id, outcome, timestamp, context_snapshot, metadata
                 FROM domain_events
                 WHERE aggregate_type = ?1 AND aggregate_id = ?2
                   AND sequence > ?3 AND sequence <= ?4
                 ORDER BY sequence ASC",
            ).map_err(|e| EventStoreError::Storage(e.to_string()))?;
            let rows: Result<Vec<EventEnvelope>, rusqlite::Error> = stmt
                .query_map(
                    rusqlite::params![agg_type, agg_id, since_exclusive as i64, head_inclusive as i64],
                    row_to_envelope,
                )
                .map_err(|e| EventStoreError::Storage(e.to_string()))?
                .collect();
            rows.map_err(|e| EventStoreError::Storage(e.to_string()))
        })
        .await
        .map_err(|e| EventStoreError::Storage(format!("join: {e}")))?
    }

    /// Internal helper for `append_batch_with_blobs` — used by step 11
    /// (connector ToolResult spillover). Atomic: blob rows + event rows in
    /// one SQLite transaction.
    pub async fn append_batch_with_blobs(
        &self,
        envelopes: Vec<EventEnvelope>,
        blobs: Vec<PreparedBlob>,
    ) -> Result<Vec<u64>, EventStoreError> {
        if envelopes.is_empty() {
            return Ok(Vec::new());
        }
        for blob in &blobs {
            metrics::record_session_event_blob_spilled(
                envelopes[0].aggregate_id.as_str(),
                blob.original_len,
            );
        }
        let len = envelopes.len() as u64;
        let agg_type = envelopes[0].aggregate_type.clone();
        let agg_id = envelopes[0].aggregate_id.clone();
        let seqs = self.writer.append_batch(envelopes, blobs).await?;
        metrics::record_append_batch_size(&agg_type, &agg_id, len);
        Ok(seqs)
    }

    /// Test seam: seed pre-stamped sequences via direct INSERT, bypassing
    /// the writer thread's stamping logic. Used by the upgrade-path
    /// regression tests in `tests/sequence_stamping_upgrade.rs`.
    ///
    /// Gated behind `cfg(any(test, feature = "test-seam"))` so it is absent
    /// from release builds (ADR-0002 / plan step 4). Integration tests under
    /// `tests/` enable the feature via the self-referencing dev-dependency
    /// in `crates/store/Cargo.toml`.
    #[cfg(any(test, feature = "test-seam"))]
    #[doc(hidden)]
    pub async fn raw_seed_for_test(&self, agg_type: &str, agg_id: &str, sequences: &[u64]) {
        let agg_type = agg_type.to_owned();
        let agg_id = agg_id.to_owned();
        let seqs: Vec<u64> = sequences.to_vec();
        self.writer
            .execute(move |conn| {
                for &seq in &seqs {
                    let id = EventId::new();
                    let now = Utc::now().to_rfc3339();
                    conn.execute(
                        "INSERT INTO domain_events (id, aggregate_type, aggregate_id, sequence, event_type, payload, trace_id, outcome, timestamp, context_snapshot, metadata)
                         VALUES (?1, ?2, ?3, ?4, 'seed', '{}', 't', 'null', ?5, 'null', '{}')",
                        rusqlite::params![id.0.to_string(), agg_type, agg_id, seq as i64, now],
                    )?;
                }
                Ok(())
            })
            .await
            .expect("raw_seed_for_test");
    }

    /// Test seam: invoke `run_session_snapshot_task` directly so concurrency
    /// tests can race appends against the snapshot replay without going
    /// through `subscribe_after`'s spawn path.
    ///
    /// Gated behind `cfg(any(test, feature = "test-seam"))` so it is absent
    /// from release builds. Integration tests under `tests/` enable the
    /// feature via the self-referencing dev-dependency in
    /// `crates/store/Cargo.toml`.
    #[cfg(any(test, feature = "test-seam"))]
    #[doc(hidden)]
    pub async fn run_session_snapshot_task_for_test(
        &self,
        agg_id: &str,
        head: u64,
    ) -> Result<(), EventStoreError> {
        run_session_snapshot_task(self, "session", agg_id, head).await
    }
}

struct SnapshotLockGuard {
    locks: crate::SnapshotLocks,
    key: (String, String),
}

impl Drop for SnapshotLockGuard {
    fn drop(&mut self) {
        self.locks.remove(&self.key);
    }
}

async fn run_session_snapshot_task(
    store: &SqliteStore,
    agg_type: &str,
    agg_id: &str,
    head: u64,
) -> Result<(), EventStoreError> {
    use nephila_core::session::Session;
    use nephila_eventsourcing::aggregate::EventSourced;

    // Snapshot interval: only act if we've drifted at least SNAPSHOT_INTERVAL
    // events past the most recent snapshot (or no snapshot at all).
    let latest = store.load_latest_snapshot(agg_type, agg_id).await?;
    let last_snap_seq = latest.as_ref().map(|s| s.sequence).unwrap_or(0);
    if head < last_snap_seq + crate::SNAPSHOT_INTERVAL {
        return Ok(());
    }
    let mut state = if let Some(snap) = latest.as_ref() {
        snap.into_state::<Session>()
            .map_err(|e| EventStoreError::Serialization(e.to_string()))?
    } else {
        Session::default_state()
    };
    let events = store
        .load_events_from_pool(agg_type, agg_id, last_snap_seq, head)
        .await?;
    let mut last_seq = last_snap_seq;
    for env in &events {
        state = state
            .apply_envelope(env)
            .map_err(|e| EventStoreError::Serialization(e.to_string()))?;
        last_seq = env.sequence;
    }
    if last_seq <= last_snap_seq {
        return Ok(());
    }
    let snap = Snapshot {
        aggregate_type: agg_type.into(),
        aggregate_id: agg_id.into(),
        sequence: last_seq,
        state: serde_json::to_value(&state)
            .map_err(|e| EventStoreError::Serialization(e.to_string()))?,
        timestamp: Utc::now(),
    };
    store.save_snapshot(&snap).await?;
    Ok(())
}

/// Build the unfold-driven backfill+listener merge.
fn build_stream(
    backfill: Vec<EventEnvelope>,
    listener: broadcast::Receiver<EventEnvelope>,
    head_at_subscribe: u64,
) -> Pin<Box<dyn Stream<Item = Result<EventEnvelope, EventStoreError>> + Send>> {
    enum State {
        Backfill(std::vec::IntoIter<EventEnvelope>),
        Live,
    }

    let init = (State::Backfill(backfill.into_iter()), listener);
    let stream = futures::stream::unfold(init, move |(state, mut listener)| async move {
        match state {
            State::Backfill(mut iter) => match iter.next() {
                Some(env) => Some((Ok(env), (State::Backfill(iter), listener))),
                None => loop {
                    match listener.recv().await {
                        Ok(env) if env.sequence > head_at_subscribe => {
                            return Some((Ok(env), (State::Live, listener)));
                        }
                        Ok(_) => continue,
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            return Some((
                                Err(EventStoreError::Lagged(n)),
                                (State::Live, listener),
                            ));
                        }
                        Err(broadcast::error::RecvError::Closed) => return None,
                    }
                },
            },
            State::Live => loop {
                match listener.recv().await {
                    Ok(env) if env.sequence > head_at_subscribe => {
                        return Some((Ok(env), (State::Live, listener)));
                    }
                    Ok(_) => continue,
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        return Some((Err(EventStoreError::Lagged(n)), (State::Live, listener)));
                    }
                    Err(broadcast::error::RecvError::Closed) => return None,
                }
            },
        }
    });
    Box::pin(stream)
}

fn row_to_envelope(row: &rusqlite::Row) -> Result<EventEnvelope, rusqlite::Error> {
    let id_str: String = row.get(0)?;
    let sequence: i64 = row.get(3)?;
    let payload_str: String = row.get(5)?;
    let outcome_str: String = row.get(7)?;
    let timestamp_str: String = row.get(8)?;
    let context_snapshot_str: String = row.get(9)?;
    let metadata_str: String = row.get(10)?;

    let id = uuid::Uuid::parse_str(&id_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let payload: serde_json::Value = serde_json::from_str(&payload_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let outcome: Option<Outcome> = serde_json::from_str(&outcome_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let context_snapshot: Option<ContextSnapshot> = serde_json::from_str(&context_snapshot_str)
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?;
    let metadata: HashMap<String, String> = serde_json::from_str(&metadata_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;

    Ok(EventEnvelope {
        id: EventId(id),
        aggregate_type: row.get(1)?,
        aggregate_id: row.get(2)?,
        sequence: sequence as u64,
        event_type: row.get(4)?,
        payload,
        trace_id: TraceId(row.get(6)?),
        outcome,
        timestamp: parse_rfc3339(&timestamp_str)?,
        context_snapshot,
        metadata,
    })
}

fn row_to_snapshot(row: &rusqlite::Row) -> Result<Snapshot, rusqlite::Error> {
    let sequence: i64 = row.get(2)?;
    let state_str: String = row.get(3)?;
    let timestamp_str: String = row.get(4)?;

    let state: serde_json::Value = serde_json::from_str(&state_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;

    Ok(Snapshot {
        aggregate_type: row.get(0)?,
        aggregate_id: row.get(1)?,
        sequence: sequence as u64,
        state,
        timestamp: parse_rfc3339(&timestamp_str)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SqliteStore;
    use chrono::Utc;
    use nephila_eventsourcing::id::EventId;

    fn make_store() -> SqliteStore {
        SqliteStore::open_in_memory(384).unwrap()
    }

    /// Fresh-envelope helper. Caller-supplied sequences are documented
    /// tests-only post-1b — production constructs envelopes with
    /// `sequence: 0` and lets the writer stamp them.
    fn make_envelope(aggregate_type: &str, aggregate_id: &str, sequence: u64) -> EventEnvelope {
        EventEnvelope {
            id: EventId::new(),
            aggregate_type: aggregate_type.to_string(),
            aggregate_id: aggregate_id.to_string(),
            sequence,
            event_type: "test_event".to_string(),
            payload: serde_json::json!({"key": "value"}),
            trace_id: TraceId("trace-1".to_string()),
            outcome: Some(Outcome::Success),
            timestamp: Utc::now(),
            context_snapshot: None,
            metadata: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn append_batch_stamps_sequences() {
        let store = make_store();
        let seqs = store
            .append_batch(vec![
                make_envelope("agent", "a1", 0),
                make_envelope("agent", "a1", 0),
                make_envelope("agent", "a1", 0),
            ])
            .await
            .unwrap();
        assert_eq!(seqs, vec![1, 2, 3]);

        let events = store.load_events("agent", "a1", 0).await.unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(
            events.iter().map(|e| e.sequence).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    #[tokio::test]
    async fn append_and_load_events() {
        let store = make_store();
        store
            .append(&make_envelope("agent", "a1", 0))
            .await
            .unwrap();
        store
            .append(&make_envelope("agent", "a1", 0))
            .await
            .unwrap();
        let events = store.load_events("agent", "a1", 0).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].sequence, 1);
        assert_eq!(events[1].sequence, 2);
    }

    #[tokio::test]
    async fn load_events_since_sequence() {
        let store = make_store();
        for _ in 1..=5 {
            store
                .append(&make_envelope("agent", "a1", 0))
                .await
                .unwrap();
        }
        let events = store.load_events("agent", "a1", 3).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].sequence, 4);
    }

    #[tokio::test]
    async fn load_by_trace_id() {
        let store = make_store();
        store
            .append(&make_envelope("agent", "a1", 0))
            .await
            .unwrap();
        let events = store
            .load_by_trace_id(&TraceId("trace-1".to_string()))
            .await
            .unwrap();
        assert_eq!(events.len(), 1);

        let empty = store
            .load_by_trace_id(&TraceId("nonexistent".to_string()))
            .await
            .unwrap();
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn save_and_load_snapshot() {
        let store = make_store();
        let snapshot = Snapshot {
            aggregate_type: "agent".to_string(),
            aggregate_id: "a1".to_string(),
            sequence: 10,
            state: serde_json::json!({"counter": 42}),
            timestamp: Utc::now(),
        };
        store.save_snapshot(&snapshot).await.unwrap();
        let loaded = store
            .load_latest_snapshot("agent", "a1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.sequence, 10);
        assert_eq!(loaded.state, serde_json::json!({"counter": 42}));
    }

    #[tokio::test]
    async fn load_latest_snapshot_returns_highest_sequence() {
        let store = make_store();
        for seq in [5, 10, 15] {
            let snapshot = Snapshot {
                aggregate_type: "agent".to_string(),
                aggregate_id: "a1".to_string(),
                sequence: seq,
                state: serde_json::json!({"seq": seq}),
                timestamp: Utc::now(),
            };
            store.save_snapshot(&snapshot).await.unwrap();
        }
        let loaded = store
            .load_latest_snapshot("agent", "a1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.sequence, 15);
    }

    #[tokio::test]
    async fn load_latest_snapshot_none_when_missing() {
        let store = make_store();
        let result = store
            .load_latest_snapshot("agent", "nonexistent")
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn prune_aggregate_removes_below_sequence() {
        let store = make_store();
        for _ in 1..=10 {
            store
                .append(&make_envelope("agent", "a1", 0))
                .await
                .unwrap();
        }
        let removed = store.prune_aggregate("agent", "a1", 5).await.unwrap();
        assert_eq!(removed, 4); // sequences 1..=4
        let remaining = store.load_events("agent", "a1", 0).await.unwrap();
        assert_eq!(remaining.len(), 6);
        assert_eq!(remaining[0].sequence, 5);
    }
}

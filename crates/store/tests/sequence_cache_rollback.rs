//! Regression test: when a writer-thread `tx.commit()` fails, the in-memory
//! `next_sequence` cache MUST NOT advance. The next successful append must
//! continue from the durable `MAX(sequence)`, not from a stale speculative
//! value.
//!
//! Uses the `FORCE_COMMIT_FAILURE` test seam (gated on
//! `cfg(any(test, feature = "test-seam"))`) to inject a one-shot commit
//! failure after row INSERTs but before the durable commit — semantically
//! identical to a real disk-full / SQLITE_BUSY error from the cache's
//! perspective.

use chrono::Utc;
use nephila_eventsourcing::envelope::{EventEnvelope, NewEventEnvelope};
use nephila_eventsourcing::id::{EventId, TraceId};
use nephila_eventsourcing::store::DomainEventStore;
use nephila_store::SqliteStore;
use nephila_store::writer::FORCE_COMMIT_FAILURE;
use std::collections::HashMap;
use std::sync::atomic::Ordering;

fn env(agg: &str, id: &str) -> EventEnvelope {
    EventEnvelope::new(NewEventEnvelope {
        id: EventId::new(),
        aggregate_type: agg.into(),
        aggregate_id: id.into(),
        event_type: "x".into(),
        payload: serde_json::json!({}),
        trace_id: TraceId("t".into()),
        outcome: None,
        timestamp: Utc::now(),
        context_snapshot: None,
        metadata: HashMap::new(),
    })
}

#[tokio::test]
async fn cache_rolls_back_on_commit_failure() {
    let store = SqliteStore::open_in_memory(384).unwrap();

    // First append succeeds, warming next_sequence to 3 for ("agent", "a1").
    let seqs = store
        .append_batch(vec![
            env("agent", "a1"),
            env("agent", "a1"),
            env("agent", "a1"),
        ])
        .await
        .unwrap();
    assert_eq!(seqs, vec![1, 2, 3]);

    // Force the next append to fail at the commit step. The writer thread
    // will have INSERTed rows into the in-flight transaction, then bail
    // before committing — the rollback is implicit (the rusqlite
    // `Transaction` Drop impl rolls back on non-commit drop).
    FORCE_COMMIT_FAILURE.store(true, Ordering::SeqCst);
    let result = store
        .append_batch(vec![
            env("agent", "a1"),
            env("agent", "a1"),
            env("agent", "a1"),
        ])
        .await;
    assert!(
        result.is_err(),
        "expected forced commit failure, got {result:?}",
    );

    // The durable MAX(sequence) is still 3 (the failed batch rolled back).
    // A subsequent successful append MUST stamp 4, 5, 6 — not 7, 8, 9.
    // If the cache had advanced speculatively, the next batch would either
    // trip the UNIQUE constraint on (4, 5, 6) or skip ahead to (7, 8, 9),
    // leaving a permanent gap.
    let after = store
        .append_batch(vec![
            env("agent", "a1"),
            env("agent", "a1"),
            env("agent", "a1"),
        ])
        .await
        .unwrap();
    assert_eq!(
        after,
        vec![4, 5, 6],
        "next_sequence cache leaked speculative values after commit failure"
    );

    // Sanity: total rows on disk = 6, sequences 1..=6 contiguous.
    let all = store.load_events("agent", "a1", 0).await.unwrap();
    assert_eq!(all.len(), 6);
    assert_eq!(
        all.iter().map(|e| e.sequence()).collect::<Vec<_>>(),
        vec![1, 2, 3, 4, 5, 6],
    );
}

#[tokio::test]
async fn same_batch_lookahead_uses_per_batch_max() {
    // Verifies the per-batch `batch_max` map serves same-aggregate envelopes
    // within one append_batch — without populating `next_sequence`. If two
    // envelopes for the same aggregate land in one batch, the second must
    // see `first.sequence + 1`. This is the bookkeeping that makes the
    // post-commit cache promotion safe.
    let store = SqliteStore::open_in_memory(384).unwrap();
    let seqs = store
        .append_batch(vec![
            env("agent", "a1"),
            env("agent", "a1"),
            env("agent", "a2"), // different aggregate, starts at 1
            env("agent", "a1"),
            env("agent", "a2"),
        ])
        .await
        .unwrap();
    assert_eq!(seqs, vec![1, 2, 1, 3, 2]);
}

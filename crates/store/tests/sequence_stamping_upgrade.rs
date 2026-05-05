//! Upgrade-path regression tests for the sequence-stamping contract introduced
//! in slice-1b (ADR-0002). These tests verify:
//!
//!   1. Pre-1b rows (caller-stamped) coexist with post-1b appends; the writer
//!      continues from `MAX(sequence)` per aggregate.
//!   2. Gapped pre-1b histories are NOT auto-renumbered; the writer continues
//!      from MAX as if the gap were intentional.
//!   3. The release-build contract: when a caller passes a non-zero sequence,
//!      the writer overwrites it unconditionally (matches the debug-only
//!      `debug_assert!` in the writer thread).

use chrono::Utc;
use nephila_eventsourcing::envelope::{EventEnvelope, NewEventEnvelope};
use nephila_eventsourcing::id::{EventId, TraceId};
use nephila_eventsourcing::store::DomainEventStore;
use nephila_store::SqliteStore;
use std::collections::HashMap;

fn env_with_seq_zero(agg_type: &str, agg_id: &str) -> EventEnvelope {
    EventEnvelope::new(NewEventEnvelope {
        id: EventId::new(),
        aggregate_type: agg_type.into(),
        aggregate_id: agg_id.into(),
        event_type: "test".into(),
        payload: serde_json::json!({}),
        trace_id: TraceId("trace".into()),
        outcome: None,
        timestamp: Utc::now(),
        context_snapshot: None,
        metadata: HashMap::new(),
    })
}

#[tokio::test]
async fn writer_continues_from_max_when_pre_existing_rows_have_caller_stamped_sequences() {
    let store = SqliteStore::open_in_memory(384).unwrap();
    store
        .raw_seed_for_test("agent", "agent-A", &[1, 2, 3])
        .await;

    let appended = store
        .append_batch(vec![
            env_with_seq_zero("agent", "agent-A"),
            env_with_seq_zero("agent", "agent-A"),
            env_with_seq_zero("agent", "agent-A"),
        ])
        .await
        .unwrap();
    assert_eq!(appended, vec![4, 5, 6]);

    let all = store.load_events("agent", "agent-A", 0).await.unwrap();
    assert_eq!(
        all.iter().map(|e| e.sequence()).collect::<Vec<_>>(),
        vec![1, 2, 3, 4, 5, 6]
    );
}

#[tokio::test]
async fn writer_handles_gapped_pre_existing_rows_by_continuing_from_max() {
    let store = SqliteStore::open_in_memory(384).unwrap();
    store
        .raw_seed_for_test("agent", "agent-B", &[1, 2, 5])
        .await;
    let appended = store
        .append_batch(vec![env_with_seq_zero("agent", "agent-B")])
        .await
        .unwrap();
    assert_eq!(appended, vec![6]);
}

#[tokio::test]
async fn writer_overwrites_caller_supplied_nonzero_sequence_in_release_builds() {
    // Release-build contract: even if a caller forgets and pre-stamps a
    // sequence, the writer overwrites it. The debug_assert! is a developer
    // convenience; this test pins the release-build behavior so a future
    // "helpful" early-return when sequence != 0 cannot land.
    if cfg!(debug_assertions) {
        // In debug builds the assert fires before we get here.
        return;
    }

    let store = SqliteStore::open_in_memory(384).unwrap();
    let mut env = env_with_seq_zero("agent", "agent-C");
    env.set_sequence(999);
    let appended = store.append_batch(vec![env]).await.unwrap();
    assert_eq!(appended, vec![1], "writer must overwrite caller value");

    let rows = store.load_events("agent", "agent-C", 0).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].sequence(), 1);
}

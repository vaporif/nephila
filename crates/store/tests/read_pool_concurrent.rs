//! Verifies that backfill loads through the read-only pool can run
//! concurrently and stay independent of the writer thread.

use chrono::Utc;
use nephila_eventsourcing::envelope::EventEnvelope;
use nephila_eventsourcing::id::{EventId, TraceId};
use nephila_eventsourcing::store::DomainEventStore;
use nephila_store::SqliteStore;
use std::collections::HashMap;
use std::sync::Arc;

fn env(agg: &str, id: &str) -> EventEnvelope {
    EventEnvelope {
        id: EventId::new(),
        aggregate_type: agg.into(),
        aggregate_id: id.into(),
        sequence: 0,
        event_type: "x".into(),
        payload: serde_json::json!({}),
        trace_id: TraceId("t".into()),
        outcome: None,
        timestamp: Utc::now(),
        context_snapshot: None,
        metadata: HashMap::new(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_backfill_yields_consistent_results() {
    let store = Arc::new(SqliteStore::open_in_memory(384).unwrap());
    let envs: Vec<_> = (0..100).map(|_| env("session", "s1")).collect();
    let _ = store.append_batch(envs).await.unwrap();

    // Pool size is 4; running 4 in parallel exercises full saturation
    // without exhausting it.
    let mut handles = Vec::new();
    for _ in 0..4 {
        let s = store.clone();
        handles.push(tokio::spawn(async move {
            let head = 100u64;
            s.load_events_from_pool("session", "s1", 0, head).await
        }));
    }

    let mut all = Vec::new();
    for h in handles {
        let result = h.await.unwrap().unwrap();
        all.push(result);
    }
    for set in &all {
        assert_eq!(set.len(), 100);
        assert_eq!(
            set.iter().map(|e| e.sequence).collect::<Vec<_>>(),
            (1u64..=100).collect::<Vec<_>>()
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn backfill_independent_of_writer() {
    let store = Arc::new(SqliteStore::open_in_memory(384).unwrap());
    let envs: Vec<_> = (0..50).map(|_| env("session", "s2")).collect();
    let _ = store.append_batch(envs).await.unwrap();

    // Start a slow append in the background (the writer is single-threaded).
    let s_writer = store.clone();
    let writer_task = tokio::spawn(async move {
        let envs: Vec<_> = (0..50).map(|_| env("session", "s2")).collect();
        s_writer.append_batch(envs).await
    });

    // While the writer is working, hit the read pool — must not block.
    let read_set = store
        .load_events_from_pool("session", "s2", 0, 50)
        .await
        .unwrap();
    assert_eq!(read_set.len(), 50);
    let _ = writer_task.await.unwrap();
}

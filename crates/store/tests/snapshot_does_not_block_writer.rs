//! Regression: the snapshot replay must not stall concurrent appends.
//!
//! Prior to H-P4, `run_session_snapshot_task` loaded events through the
//! writer thread. Replaying ~1k+ events serialised every concurrent append
//! behind the SELECT + row-decode. This test pins the new behaviour:
//! the replay reads via the pool, so an append racing alongside completes
//! in well under the writer-bound case.

use chrono::Utc;
use nephila_core::session_event::SessionEvent;
use nephila_core::store::AgentStore;
use nephila_eventsourcing::envelope::{EventEnvelope, NewEventEnvelope};
use nephila_eventsourcing::id::{EventId, TraceId};
use nephila_eventsourcing::store::DomainEventStore;
use nephila_store::SqliteStore;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn make_test_envelope(agg: &str, message_id: &str) -> EventEnvelope {
    // AssistantMessage is state-neutral in `Session::apply`, so we can seed
    // any number of these without worrying about phase invariants.
    let event = SessionEvent::AssistantMessage {
        message_id: message_id.to_owned(),
        seq_in_message: 0,
        delta_text: "x".to_owned(),
        is_final: false,
        truncated: false,
        ts: Utc::now(),
    };
    EventEnvelope::new(NewEventEnvelope {
        id: EventId::new(),
        aggregate_type: "session".to_owned(),
        aggregate_id: agg.to_owned(),
        event_type: "AssistantMessage".to_owned(),
        payload: serde_json::to_value(&event).unwrap(),
        trace_id: TraceId(agg.to_owned()),
        outcome: None,
        timestamp: Utc::now(),
        context_snapshot: None,
        metadata: HashMap::new(),
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn append_not_starved_during_snapshot_replay() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("db.sqlite");
    let store = Arc::new(SqliteStore::open(&path, 384).unwrap());

    // Seed enough events that the writer-bound replay (SELECT + decode of
    // every row) takes long enough to be measurable. With 2k rows in debug,
    // the SELECT alone is microseconds and the regression doesn't show.
    // 30k gives us a comfortable signal-to-noise ratio.
    let agg = "test-agg".to_owned();
    const SEED_COUNT: usize = 30_000;
    let mut envs = Vec::with_capacity(SEED_COUNT);
    for i in 0..SEED_COUNT {
        envs.push(make_test_envelope(&agg, &format!("m{i}")));
    }
    store.append_batch(envs).await.unwrap();

    // Trigger a snapshot task in the background via the test seam.
    let store_clone = Arc::clone(&store);
    let agg_clone = agg.clone();
    let snap_task = tokio::spawn(async move {
        store_clone
            .run_session_snapshot_task_for_test(&agg_clone, SEED_COUNT as u64)
            .await
    });

    // Give the spawned task a chance to start loading events before we
    // measure. Without this yield/sleep the test can finish the racing
    // append before the snapshot task has even hit the writer queue.
    tokio::time::sleep(Duration::from_millis(5)).await;

    // Race a small append against the snapshot. Measure latency.
    let start = Instant::now();
    store
        .append_batch(vec![make_test_envelope(&agg, "concurrent")])
        .await
        .unwrap();
    let append_latency = start.elapsed();

    snap_task.await.unwrap().unwrap();

    assert!(
        append_latency < Duration::from_millis(50),
        "append blocked by snapshot: {append_latency:?}"
    );
}

/// Regression: `AgentStore::list` (and the other read methods on the store)
/// must not be serialised behind a long-running write batch. With the old
/// behaviour, every read went through the writer thread, so a slow append
/// batch starved concurrent reads. Reads now flow through the read pool;
/// this test pins that.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_list_not_starved_during_long_append() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("db.sqlite");
    let store = Arc::new(SqliteStore::open(&path, 384).unwrap());

    // Kick off a slow append in the background. 30k rows is enough to keep
    // the writer thread busy long enough that a writer-routed read would
    // queue behind it for tens of milliseconds.
    let store_clone = Arc::clone(&store);
    let big_batch: Vec<_> = (0..30_000)
        .map(|i| make_test_envelope("agg", &format!("m{i}")))
        .collect();
    let writer_task =
        tokio::spawn(async move { store_clone.append_batch(big_batch).await.unwrap() });

    // Give the writer a head start so the read fires while it's still busy.
    tokio::time::sleep(Duration::from_millis(2)).await;

    let start = Instant::now();
    let _ = AgentStore::list(&*store).await.unwrap();
    let read_latency = start.elapsed();

    writer_task.await.unwrap();

    assert!(
        read_latency < Duration::from_millis(20),
        "read blocked by writer: {read_latency:?}"
    );
}

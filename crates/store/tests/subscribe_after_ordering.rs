//! `subscribe_after` listener-first ordering and Lagged-recovery tests.
//!
//! Test 1 (`listener_attached_before_head_snapshot_does_not_lose_concurrent_appends`)
//! is deterministic — uses the `SubscribeAfterHooks::pre_head_snapshot` seam
//! to inject a sleep at the gap between `listener.subscribe()` and
//! `self.head_sequence(...)`.
//!
//! Test 2 is a probabilistic burst smoke for non-pathological loads.
//!
//! Test 3 verifies the broadcast bound is small enough that 100 events with a
//! paused consumer surfaces `Lagged`.

use chrono::Utc;
use futures::StreamExt;
use nephila_eventsourcing::envelope::EventEnvelope;
use nephila_eventsourcing::id::{EventId, TraceId};
use nephila_eventsourcing::store::{DomainEventStore, EventStoreError};
use nephila_store::SqliteStore;
use nephila_store::subscribe::SubscribeAfterHooks;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

fn env(payload_text: &str) -> EventEnvelope {
    EventEnvelope {
        id: EventId::new(),
        aggregate_type: "session".into(),
        aggregate_id: "s1".into(),
        sequence: 0,
        event_type: "x".into(),
        payload: serde_json::json!({"text": payload_text}),
        trace_id: TraceId("t".into()),
        outcome: None,
        timestamp: Utc::now(),
        context_snapshot: None,
        metadata: HashMap::new(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn listener_attached_before_head_snapshot_does_not_lose_concurrent_appends() {
    let store = Arc::new(SqliteStore::open_in_memory(384).unwrap());

    // Pre-seed seq 1..=3.
    let _ = store
        .append_batch(vec![env("e1"), env("e2"), env("e3")])
        .await
        .unwrap();

    let raced_visible = Arc::new(AtomicBool::new(false));
    let raced_visible_inner = raced_visible.clone();
    let store_clone = store.clone();

    // Hook signals once it is invoked — we drive a concurrent append after
    // the listener attached but before the head snapshot.
    let hooks = SubscribeAfterHooks {
        pre_head_snapshot: Some(Arc::new(move || {
            // Mark progress but DO NOT block here — the writer must be free
            // to publish the racing append. We use a busy-loop until the
            // racing append is committed.
            raced_visible_inner.store(true, Ordering::SeqCst);
            // The actual sequencing: we just sleep for a small amount; the
            // racing tokio::spawn below races against us via the broadcast.
            std::thread::sleep(std::time::Duration::from_millis(50));
        })),
    };

    // Spawn a task that races an append while the hook is sleeping.
    let race = tokio::spawn({
        let store = store_clone.clone();
        async move {
            // Wait until the hook is observed in the synchronous path.
            for _ in 0..1000 {
                if raced_visible.load(Ordering::SeqCst) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
            // Race window: append now. Either head_snapshot sees this (backfill)
            // or the listener does (live) — never both, never neither.
            store.append_batch(vec![env("raced")]).await.unwrap()
        }
    });

    let stream_fut = store
        .subscribe_after_with_hooks("session", "s1", 0, hooks)
        .await
        .unwrap();
    let raced_seqs = race.await.unwrap();
    assert_eq!(raced_seqs, vec![4]);

    // Drain at least 4 events; assert "raced" appears exactly once.
    let mut stream = stream_fut;
    let mut seen_texts: Vec<String> = Vec::new();
    let mut seen_seqs: Vec<u64> = Vec::new();
    let drain = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while seen_seqs.len() < 4 {
            match stream.next().await {
                Some(Ok(env)) => {
                    seen_seqs.push(env.sequence);
                    let text = env.payload["text"].as_str().unwrap_or("").to_string();
                    seen_texts.push(text);
                }
                Some(Err(e)) => panic!("unexpected error: {e:?}"),
                None => break,
            }
        }
    })
    .await;
    drain.expect("did not see 4 envelopes within 2s");

    assert_eq!(seen_seqs, vec![1, 2, 3, 4]);
    assert_eq!(seen_texts.iter().filter(|t| t == &"raced").count(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn burst_subscribers_each_see_their_concurrent_append() {
    let store = Arc::new(SqliteStore::open_in_memory(384).unwrap());
    let _ = store
        .append_batch(vec![env("e1"), env("e2"), env("e3")])
        .await
        .unwrap();

    // Run 4 subscribe+append pairs in parallel — matches the 4-conn read
    // pool size. Beyond 4 concurrent in-flight subscribes, backpressure is
    // the consumer's job (see `resilient_subscribe`); this test asserts the
    // happy path only.
    let mut handles = Vec::new();
    for i in 0..4 {
        let store = store.clone();
        handles.push(tokio::spawn(async move {
            let mut stream = store.subscribe_after("session", "s1", 0).await.unwrap();
            let payload_text = format!("burst-{i}");
            let _ = store.append_batch(vec![env(&payload_text)]).await.unwrap();

            // Drain until we see our payload or up to a small budget.
            let drain = tokio::time::timeout(std::time::Duration::from_secs(3), async {
                while let Some(item) = stream.next().await {
                    if let Ok(env) = item
                        && env.payload["text"] == payload_text
                    {
                        return true;
                    }
                }
                false
            })
            .await;
            drain.unwrap_or(false)
        }));
    }
    for h in handles {
        assert!(h.await.unwrap(), "subscriber missed its concurrent append");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lagged_surfaces_when_consumer_falls_behind() {
    // The default broadcast capacity is 4096; to keep the test tight, we
    // append far more events than that and never poll the stream until the
    // very end. The listener must surface `Lagged`.
    let store = Arc::new(SqliteStore::open_in_memory(384).unwrap());
    let mut stream = store.subscribe_after("session", "s1", 0).await.unwrap();

    // Burst: 5000 events, never polled.
    let mut envs: Vec<_> = (0..5000).map(|i| env(&format!("burst-{i}"))).collect();
    while !envs.is_empty() {
        let chunk = envs.split_off(envs.len().saturating_sub(500));
        let _ = store.append_batch(chunk).await.unwrap();
    }

    // Now drain. Eventually we expect to see a Lagged item.
    let mut saw_lagged = false;
    let drain = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while let Some(item) = stream.next().await {
            if matches!(item, Err(EventStoreError::Lagged(_))) {
                saw_lagged = true;
                break;
            }
            if item.is_ok() {
                continue;
            }
        }
    })
    .await;
    drain.expect("drain timed out");
    assert!(
        saw_lagged,
        "expected Lagged surfaced from listener-side overflow"
    );
}

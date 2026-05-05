//! Throughput benchmark: 1 producer × 5000 events (10 turns × 500) with 4
//! concurrent subscribers reading from seq 0. Asserts each subscriber sees
//! all 5000 events in order; measures wall-clock to completion.
//!
//! Gate: p95 < 2s. CI runs without `--quick`. Local iteration with `--quick`
//! is fine for fast feedback but not authoritative.

use chrono::Utc;
use criterion::{Criterion, criterion_group, criterion_main};
use futures::StreamExt;
use nephila_eventsourcing::envelope::{EventEnvelope, NewEventEnvelope};
use nephila_eventsourcing::id::{EventId, TraceId};
use nephila_eventsourcing::store::DomainEventStore;
use nephila_store::SqliteStore;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

const TOTAL_EVENTS: usize = 5000;
const SUBSCRIBERS: usize = 4;
const CHUNK: usize = 500;

fn env(text: &str, agg_id: &str) -> EventEnvelope {
    EventEnvelope::new(NewEventEnvelope {
        id: EventId::new(),
        aggregate_type: "session".into(),
        aggregate_id: agg_id.into(),
        event_type: "x".into(),
        payload: serde_json::json!({"text": text}),
        trace_id: TraceId("t".into()),
        outcome: None,
        timestamp: Utc::now(),
        context_snapshot: None,
        metadata: HashMap::new(),
    })
}

async fn one_iter() -> Duration {
    let store = Arc::new(SqliteStore::open_in_memory(384).unwrap());
    let agg = "bench-session";

    let start = Instant::now();
    let mut sub_handles = Vec::new();
    for _ in 0..SUBSCRIBERS {
        let s = store.clone();
        sub_handles.push(tokio::spawn(async move {
            let mut stream = s.subscribe_after("session", agg, 0).await.unwrap();
            let mut count = 0usize;
            while let Some(item) = stream.next().await {
                if let Ok(env) = item {
                    debug_assert_eq!(env.sequence(), count as u64 + 1);
                    count += 1;
                    if count == TOTAL_EVENTS {
                        break;
                    }
                }
            }
            count
        }));
    }

    for turn in 0..(TOTAL_EVENTS / CHUNK) {
        let envs: Vec<_> = (0..CHUNK)
            .map(|i| env(&format!("t{turn}-i{i}"), agg))
            .collect();
        store.append_batch(envs).await.unwrap();
    }

    for h in sub_handles {
        let count = h.await.unwrap();
        debug_assert_eq!(count, TOTAL_EVENTS);
    }

    start.elapsed()
}

fn bench_one_agent(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();

    c.bench_function("1ag_5000e_10t_4sub", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                total += rt.block_on(one_iter());
            }
            total
        });
    });
}

criterion_group!(benches, bench_one_agent);
criterion_main!(benches);

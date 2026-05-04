//! Livelock guard — sustained `Lagged` returns must escalate to
//! `PersistentLag` rather than cycling forever.
//!
//! Drives the wrapper against a fake store that yields a configurable burst
//! of `Lagged` errors, then closes. With `hard_limit=3` and ~zero cooldown
//! the test resolves in milliseconds.

use chrono::{DateTime, Utc};
use futures::{Stream, StreamExt};
use nephila_eventsourcing::envelope::EventEnvelope;
use nephila_eventsourcing::id::TraceId;
use nephila_eventsourcing::snapshot::Snapshot;
use nephila_eventsourcing::store::{DomainEventStore, EventStoreError, EventStream};
use nephila_store::resilient_subscribe::{RetryConfig, resilient_subscribe_with_config};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

struct AlwaysLaggedStore {
    laggings_remaining: AtomicUsize,
}

impl AlwaysLaggedStore {
    fn new(count: usize) -> Self {
        Self {
            laggings_remaining: AtomicUsize::new(count),
        }
    }
}

impl DomainEventStore for AlwaysLaggedStore {
    async fn append(&self, _envelope: &EventEnvelope) -> Result<(), EventStoreError> {
        Ok(())
    }
    async fn append_batch(
        &self,
        _envelopes: Vec<EventEnvelope>,
    ) -> Result<Vec<u64>, EventStoreError> {
        Ok(Vec::new())
    }
    async fn load_events(
        &self,
        _aggregate_type: &str,
        _aggregate_id: &str,
        _since_sequence: u64,
    ) -> Result<Vec<EventEnvelope>, EventStoreError> {
        Ok(Vec::new())
    }
    async fn load_by_trace_id(
        &self,
        _trace_id: &TraceId,
    ) -> Result<Vec<EventEnvelope>, EventStoreError> {
        Ok(Vec::new())
    }
    async fn load_by_time_range(
        &self,
        _from: DateTime<Utc>,
        _to: DateTime<Utc>,
    ) -> Result<Vec<EventEnvelope>, EventStoreError> {
        Ok(Vec::new())
    }
    async fn save_snapshot(&self, _snapshot: &Snapshot) -> Result<(), EventStoreError> {
        Ok(())
    }
    async fn load_latest_snapshot(
        &self,
        _aggregate_type: &str,
        _aggregate_id: &str,
    ) -> Result<Option<Snapshot>, EventStoreError> {
        Ok(None)
    }
    async fn subscribe_after(
        &self,
        _aggregate_type: &str,
        _aggregate_id: &str,
        _since_sequence: u64,
    ) -> Result<EventStream, EventStoreError> {
        let remaining = self.laggings_remaining.fetch_sub(1, Ordering::SeqCst);
        if remaining == 0 {
            Ok(Box::pin(EmptyStream) as EventStream)
        } else {
            Ok(Box::pin(LaggedOnceStream { yielded: false }) as EventStream)
        }
    }
    async fn prune_aggregate(
        &self,
        _aggregate_type: &str,
        _aggregate_id: &str,
        _before_sequence: u64,
    ) -> Result<u64, EventStoreError> {
        Ok(0)
    }
}

struct EmptyStream;
impl Stream for EmptyStream {
    type Item = Result<EventEnvelope, EventStoreError>;
    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(None)
    }
}

struct LaggedOnceStream {
    yielded: bool,
}
impl Stream for LaggedOnceStream {
    type Item = Result<EventEnvelope, EventStoreError>;
    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.yielded {
            Poll::Ready(None)
        } else {
            self.yielded = true;
            Poll::Ready(Some(Err(EventStoreError::Lagged(42))))
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sustained_lag_escalates_to_persistent_lag() {
    let cfg = RetryConfig {
        retry_window: Duration::from_secs(60),
        quiet_period: Duration::from_secs(60),
        cooldown: Duration::from_millis(0),
        soft_limit: 1,
        hard_limit: 3,
    };
    let store = Arc::new(AlwaysLaggedStore::new(100));
    let mut stream = resilient_subscribe_with_config(store, "session".into(), "s1".into(), 0, cfg);

    let mut saw = false;
    let drain = tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(item) = stream.next().await {
            if matches!(item, Err(EventStoreError::PersistentLag(_))) {
                saw = true;
                break;
            }
        }
    })
    .await;
    drain.expect("drain timed out");
    assert!(saw, "expected PersistentLag escalation");
}

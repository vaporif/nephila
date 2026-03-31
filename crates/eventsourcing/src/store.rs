use crate::envelope::EventEnvelope;
use crate::id::{SpanId, TraceId};
use crate::snapshot::Snapshot;
use crate::tracing::StoredSpan;
use chrono::{DateTime, Utc};

#[derive(Debug, thiserror::Error)]
pub enum EventStoreError {
    #[error("storage error: {0}")]
    Storage(String),
    #[error("serialization error: {0}")]
    Serialization(String),
    #[error("concurrency conflict: {0}")]
    ConcurrencyConflict(String),
}

#[derive(Debug, thiserror::Error)]
pub enum TracingStoreError {
    #[error("storage error: {0}")]
    Storage(String),
    #[error("serialization error: {0}")]
    Serialization(String),
}

pub trait DomainEventStore: Send + Sync {
    fn append(
        &self,
        envelope: &EventEnvelope,
    ) -> impl std::future::Future<Output = Result<(), EventStoreError>> + Send;

    fn load_events(
        &self,
        aggregate_type: &str,
        aggregate_id: &str,
        since_sequence: u64,
    ) -> impl std::future::Future<Output = Result<Vec<EventEnvelope>, EventStoreError>> + Send;

    fn load_by_trace_id(
        &self,
        trace_id: &TraceId,
    ) -> impl std::future::Future<Output = Result<Vec<EventEnvelope>, EventStoreError>> + Send;

    fn load_by_time_range(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> impl std::future::Future<Output = Result<Vec<EventEnvelope>, EventStoreError>> + Send;

    fn save_snapshot(
        &self,
        snapshot: &Snapshot,
    ) -> impl std::future::Future<Output = Result<(), EventStoreError>> + Send;

    fn load_latest_snapshot(
        &self,
        aggregate_type: &str,
        aggregate_id: &str,
    ) -> impl std::future::Future<Output = Result<Option<Snapshot>, EventStoreError>> + Send;
}

pub trait TracingStore: Send + Sync {
    fn record_span(
        &self,
        span: &StoredSpan,
    ) -> impl std::future::Future<Output = Result<(), TracingStoreError>> + Send;

    fn load_spans_by_trace(
        &self,
        trace_id: &TraceId,
    ) -> impl std::future::Future<Output = Result<Vec<StoredSpan>, TracingStoreError>> + Send;

    fn load_spans_by_time_range(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> impl std::future::Future<Output = Result<Vec<StoredSpan>, TracingStoreError>> + Send;

    fn load_child_spans(
        &self,
        parent_span_id: &SpanId,
    ) -> impl std::future::Future<Output = Result<Vec<StoredSpan>, TracingStoreError>> + Send;
}

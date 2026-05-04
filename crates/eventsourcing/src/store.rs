use crate::envelope::EventEnvelope;
use crate::id::{SpanId, TraceId};
use crate::snapshot::Snapshot;
use crate::tracing::StoredSpan;
use chrono::{DateTime, Utc};
use std::pin::Pin;

#[derive(Debug, thiserror::Error)]
pub enum EventStoreError {
    #[error("storage error: {0}")]
    Storage(String),
    #[error("serialization error: {0}")]
    Serialization(String),
    #[error("concurrency conflict: {0}")]
    ConcurrencyConflict(String),
    /// Surfaced from a live subscription when the broadcast channel dropped
    /// `n` events because the consumer fell behind. The consumer should fall
    /// back to `load_events` and re-subscribe from the last seen sequence.
    #[error("broadcast lagged: {0} events dropped")]
    Lagged(u64),
    /// Repeated `Lagged` recovery failed within the configured retry budget.
    /// The consumer should escalate (UI overlay, supervisor pause).
    #[error("persistent lag: {0} retries within budget exhausted")]
    PersistentLag(u32),
    /// Read-pool ran out of connections. Indicates either misconfiguration or
    /// a runaway consumer; the caller should retry after a short backoff.
    #[error("read pool exhausted")]
    PoolExhausted,
}

/// Stream item produced by [`DomainEventStore::subscribe_after`]. Each yielded
/// `Result` is either a fresh envelope from backfill+live merge or a
/// transient error (`Lagged` is the most common, signalling broadcast overflow).
pub type EventStream =
    Pin<Box<dyn futures::Stream<Item = Result<EventEnvelope, EventStoreError>> + Send>>;

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

    /// Append a batch of envelopes atomically, stamping sequences inside the
    /// writer thread. Returns the assigned sequences in input order.
    ///
    /// Callers MUST set `envelope.sequence = 0`; the writer overwrites it.
    /// See ADR-0002.
    fn append_batch(
        &self,
        envelopes: Vec<EventEnvelope>,
    ) -> impl std::future::Future<Output = Result<Vec<u64>, EventStoreError>> + Send;

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

    /// Subscribe to an aggregate's event stream starting strictly AFTER
    /// `since_sequence`. Backfill is yielded first (in order), then live
    /// envelopes from the broadcast listener, deduped against the head
    /// snapshot to keep the join lossless and exactly-once.
    ///
    /// The stream may yield `Err(EventStoreError::Lagged(n))` if the consumer
    /// falls behind; recovery is the consumer's responsibility (see
    /// `nephila_store::resilient_subscribe`).
    fn subscribe_after(
        &self,
        aggregate_type: &str,
        aggregate_id: &str,
        since_sequence: u64,
    ) -> impl std::future::Future<Output = Result<EventStream, EventStoreError>> + Send;

    /// Delete all events for `(aggregate_type, aggregate_id)` with
    /// `sequence < before_sequence`. Returns the number of rows removed.
    /// Used by retention policy and snapshot-based truncation.
    fn prune_aggregate(
        &self,
        aggregate_type: &str,
        aggregate_id: &str,
        before_sequence: u64,
    ) -> impl std::future::Future<Output = Result<u64, EventStoreError>> + Send;
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

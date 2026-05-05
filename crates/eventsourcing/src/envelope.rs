use crate::id::{EventId, TraceId};
use crate::outcome::Outcome;
use crate::snapshot::ContextSnapshot;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub id: EventId,
    pub aggregate_type: String,
    pub aggregate_id: String,
    /// Writer-assigned sequence within `(aggregate_type, aggregate_id)`.
    ///
    /// Set inside the writer thread's INSERT transaction. Fresh envelopes
    /// produced by [`EventEnvelope::new`] hold `0` until stamping. Read via
    /// [`EventEnvelope::sequence`].
    ///
    /// Private to this module: the only legitimate post-construction setter
    /// is [`EventEnvelope::set_sequence`], called from the store writer.
    /// See `docs/adr/0002-eventenvelope-sequence-stamping.md`.
    sequence: u64,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub trace_id: TraceId,
    pub outcome: Option<Outcome>,
    pub timestamp: DateTime<Utc>,
    pub context_snapshot: Option<ContextSnapshot>,
    pub metadata: HashMap<String, String>,
}

/// Field-bag for [`EventEnvelope::new`]. Mirrors the public fields of
/// `EventEnvelope` minus `sequence`, which is always stamped to `0` at
/// construction.
#[derive(Debug, Clone)]
pub struct NewEventEnvelope {
    pub id: EventId,
    pub aggregate_type: String,
    pub aggregate_id: String,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub trace_id: TraceId,
    pub outcome: Option<Outcome>,
    pub timestamp: DateTime<Utc>,
    pub context_snapshot: Option<ContextSnapshot>,
    pub metadata: HashMap<String, String>,
}

impl EventEnvelope {
    /// Construct a fresh envelope with `sequence = 0`. The store writer
    /// stamps the real sequence inside its INSERT transaction.
    #[must_use]
    pub fn new(args: NewEventEnvelope) -> Self {
        Self {
            id: args.id,
            aggregate_type: args.aggregate_type,
            aggregate_id: args.aggregate_id,
            sequence: 0,
            event_type: args.event_type,
            payload: args.payload,
            trace_id: args.trace_id,
            outcome: args.outcome,
            timestamp: args.timestamp,
            context_snapshot: args.context_snapshot,
            metadata: args.metadata,
        }
    }

    /// Returns the writer-stamped sequence, or `0` for a freshly constructed
    /// envelope that has not yet been appended.
    #[must_use]
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Set the writer-stamped sequence. Called by the store's writer thread
    /// inside its INSERT transaction. Code outside the writer should not
    /// invoke this; the value is normally produced by
    /// `MAX(sequence) + 1` per aggregate.
    pub fn set_sequence(&mut self, seq: u64) {
        self.sequence = seq;
    }
}

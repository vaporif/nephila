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
    /// CONTRACT (post slice-1b): callers MUST set this to `0` when constructing
    /// fresh envelopes. The store stamps the actual sequence inside the writer
    /// thread, after computing
    /// `MAX(sequence) WHERE aggregate_type=? AND aggregate_id=?`.
    /// On reads, this is the assigned sequence.
    ///
    /// In debug builds the writer asserts that the caller-supplied value is `0`;
    /// in release builds the writer overwrites unconditionally. See
    /// `docs/adr/0002-eventenvelope-sequence-stamping.md`.
    pub sequence: u64,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub trace_id: TraceId,
    pub outcome: Option<Outcome>,
    pub timestamp: DateTime<Utc>,
    pub context_snapshot: Option<ContextSnapshot>,
    pub metadata: HashMap<String, String>,
}

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
    pub sequence: u64,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub trace_id: TraceId,
    pub outcome: Option<Outcome>,
    pub timestamp: DateTime<Utc>,
    pub context_snapshot: Option<ContextSnapshot>,
    pub metadata: HashMap<String, String>,
}

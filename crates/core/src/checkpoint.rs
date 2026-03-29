use crate::id::{AgentId, CheckpointVersion, EntryId, ObjectiveId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct L0State {
    pub objectives: Vec<ObjectiveSnapshot>,
    pub next_steps: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectiveSnapshot {
    pub id: ObjectiveId,
    pub description: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct L2Chunk {
    pub id: EntryId,
    pub content: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub agent_id: AgentId,
    pub version: CheckpointVersion,
    pub l0: L0State,
    pub l1: String,
    pub timestamp: DateTime<Utc>,
}

/// Missing fields are filled in by the active Summarizer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointInput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub l0: Option<L0State>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub l1: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub l2: Option<Vec<L2Chunk>>,
}

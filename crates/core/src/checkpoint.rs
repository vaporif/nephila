use crate::id::{AgentId, CheckpointId, EntryId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointNode {
    pub id: CheckpointId,
    pub agent_id: AgentId,
    pub parent_id: Option<CheckpointId>,
    pub branch_label: Option<String>,
    pub channels: BTreeMap<String, ChannelEntry>,
    pub l2_namespace: String,
    pub interrupt: Option<InterruptSnapshot>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelEntry {
    pub reducer: ReducerKind,
    pub value: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReducerKind {
    Overwrite,
    Append,
    SetUnion,
    Sum,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterruptSnapshot {
    pub interrupt_type: InterruptType,
    pub payload: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterruptType {
    Drain,
    Hitl,
    Pause,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct L2Chunk {
    pub id: EntryId,
    pub content: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct L2SearchResult {
    pub chunk: L2Chunk,
    pub agent_id: AgentId,
    pub namespace: String,
    pub score: f32,
}

pub const REQUIRED_CHANNELS: &[&str] = &["objectives", "progress_summary", "decisions", "blockers"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkRequest {
    pub source_checkpoint_id: CheckpointId,
    pub branch_label: String,
    pub directive_override: Option<String>,
}

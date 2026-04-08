use crate::checkpoint::InterruptType;
use crate::id::{AgentId, CheckpointId, InterruptId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterruptStatus {
    Pending,
    Resolved,
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterruptRequest {
    pub id: InterruptId,
    pub agent_id: AgentId,
    pub checkpoint_id: CheckpointId,
    pub interrupt_type: InterruptType,
    pub payload: Option<serde_json::Value>,
    pub status: InterruptStatus,
    pub response: Option<serde_json::Value>,
    pub question_hash: Option<String>,
    pub ask_count: u32,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}

use crate::id::{AgentId, CheckpointVersion, ObjectiveId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display, strum::EnumString,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum AgentState {
    Starting,
    Active,
    Draining,
    Restoring,
    Exited,
    Completed,
    Failed,
    Paused,
}

/// Determines which MCP tools are exposed to the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentPhase {
    Restoring,
    Active,
    Draining,
}

impl AgentState {
    #[must_use]
    pub fn from_str_lossy(s: &str) -> Self {
        s.parse().unwrap_or(Self::Starting)
    }

    pub fn phase(self) -> Option<AgentPhase> {
        match self {
            Self::Restoring => Some(AgentPhase::Restoring),
            Self::Starting | Self::Active => Some(AgentPhase::Active),
            Self::Draining => Some(AgentPhase::Draining),
            Self::Exited | Self::Completed | Self::Failed | Self::Paused => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRecord {
    pub id: AgentId,
    pub state: AgentState,
    pub directory: PathBuf,
    pub objective_id: ObjectiveId,
    pub checkpoint_version: Option<CheckpointVersion>,
    pub spawned_by: Option<AgentId>,
    pub injected_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

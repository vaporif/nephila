use crate::id::{AgentId, ObjectiveId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display, strum::EnumString,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum ObjectiveStatus {
    Pending,
    InProgress,
    Done,
    Blocked,
}

impl ObjectiveStatus {
    #[must_use]
    pub fn from_str_lossy(s: &str) -> Self {
        s.parse().unwrap_or(Self::Pending)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectiveNode {
    pub id: ObjectiveId,
    pub parent_id: Option<ObjectiveId>,
    pub agent_id: Option<AgentId>,
    pub description: String,
    pub status: ObjectiveStatus,
    pub children: Vec<ObjectiveNode>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectiveTree {
    pub root: ObjectiveNode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewObjective {
    pub parent_id: Option<ObjectiveId>,
    pub agent_id: Option<AgentId>,
    pub description: String,
}

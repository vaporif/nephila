use crate::id::{AgentId, EntryId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub type Embedding = Vec<f32>;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display, strum::EnumString,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum LifecycleState {
    Generated,
    Active,
    Merged,
    Archived,
    Forgotten,
}

impl LifecycleState {
    #[must_use]
    pub fn from_str_lossy(s: &str) -> Self {
        s.parse().unwrap_or(Self::Generated)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: EntryId,
    pub agent_id: AgentId,
    pub content: String,
    pub embedding: Embedding,
    pub tags: Vec<String>,
    pub lifecycle_state: LifecycleState,
    pub importance: f32,
    pub access_count: u32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub entry: MemoryEntry,
    pub score: f32,
    pub linked: Vec<MemoryEntry>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Link {
    pub target_id: EntryId,
    pub similarity_score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchQuery {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<EntryId>,
    #[serde(default)]
    pub detail: bool,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    5
}

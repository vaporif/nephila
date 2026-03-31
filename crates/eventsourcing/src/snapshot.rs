use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSnapshot {
    pub memory_entries: Vec<MemoryEntrySnapshot>,
    pub l2_chunks: Vec<L2ChunkSnapshot>,
    pub objective_state: Vec<ObjectiveSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntrySnapshot {
    pub id: String,
    pub content: String,
    pub tags: Vec<String>,
    pub importance: f32,
    pub lifecycle_state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct L2ChunkSnapshot {
    pub id: String,
    pub content: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectiveSnapshot {
    pub id: String,
    pub description: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub aggregate_type: String,
    pub aggregate_id: String,
    pub sequence: u64,
    pub state: serde_json::Value,
    pub timestamp: DateTime<Utc>,
}

impl Snapshot {
    pub fn from_aggregate<S: Serialize>(
        agg_type: &str,
        agg_id: &str,
        seq: u64,
        state: &S,
    ) -> Result<Self, serde_json::Error> {
        Ok(Self {
            aggregate_type: agg_type.to_string(),
            aggregate_id: agg_id.to_string(),
            sequence: seq,
            state: serde_json::to_value(state)?,
            timestamp: Utc::now(),
        })
    }

    pub fn into_state<S: DeserializeOwned>(&self) -> Result<S, serde_json::Error> {
        serde_json::from_value(self.state.clone())
    }
}

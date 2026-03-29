use crate::id::{AgentId, EntryId, ObjectiveId};

#[derive(Debug, thiserror::Error)]
pub enum MeridianError {
    #[error("agent not found: {0}")]
    AgentNotFound(AgentId),

    #[error("entry not found: {0}")]
    EntryNotFound(EntryId),

    #[error("objective not found: {0}")]
    ObjectiveNotFound(ObjectiveId),

    #[error("checkpoint not found for agent {0}")]
    CheckpointNotFound(AgentId),

    #[error("invalid state transition: {from} -> {to}")]
    InvalidStateTransition { from: String, to: String },

    #[error("storage error: {0}")]
    Storage(String),

    #[error("embedding error: {0}")]
    Embedding(String),

    #[error("process error: {0}")]
    Process(String),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("config error: {0}")]
    Config(String),

    #[error("shutdown in progress")]
    Shutdown,
}

pub type Result<T> = std::result::Result<T, MeridianError>;

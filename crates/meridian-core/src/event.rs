use crate::id::{AgentId, ObjectiveId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    ToolCall,
    ToolResult,
    StateChange,
    DirectiveChange,
    CheckpointWrite,
    HitlRequest,
    HitlResponse,
    AgentSpawn,
    AgentExit,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpEvent {
    pub id: uuid::Uuid,
    pub agent_id: AgentId,
    pub event_type: EventType,
    pub timestamp: DateTime<Utc>,
    pub content: serde_json::Value,
    pub objective_id: Option<ObjectiveId>,
}

#[derive(Debug, Clone)]
pub enum BusEvent {
    Mcp(McpEvent),
    AgentStateChanged {
        agent_id: AgentId,
        old_state: crate::agent::AgentState,
        new_state: crate::agent::AgentState,
    },
    TokenReport {
        agent_id: AgentId,
        used: u64,
        remaining: u64,
    },
    HitlRequested {
        agent_id: AgentId,
        question: String,
        options: Vec<String>,
    },
    HitlResponded {
        agent_id: AgentId,
        response: String,
    },
    Shutdown,
}

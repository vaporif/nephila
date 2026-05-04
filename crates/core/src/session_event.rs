use crate::id::{AgentId, CheckpointId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

pub type SessionId = Uuid;
pub type TurnId = Uuid;
pub type MessageId = String;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum SessionEvent {
    SessionStarted {
        session_id: SessionId,
        agent_id: AgentId,
        model: Option<String>,
        working_dir: PathBuf,
        mcp_endpoint: String,
        resumed: bool,
        ts: DateTime<Utc>,
    },
    AgentPromptQueued {
        turn_id: TurnId,
        text: String,
        ts: DateTime<Utc>,
    },
    AgentPromptDelivered {
        turn_id: TurnId,
        ts: DateTime<Utc>,
    },
    HumanPromptQueued {
        turn_id: TurnId,
        text: String,
        ts: DateTime<Utc>,
    },
    HumanPromptDelivered {
        turn_id: TurnId,
        ts: DateTime<Utc>,
    },
    PromptDeliveryFailed {
        turn_id: TurnId,
        reason: String,
        ts: DateTime<Utc>,
    },
    AssistantMessage {
        message_id: MessageId,
        seq_in_message: u32,
        delta_text: String,
        is_final: bool,
        truncated: bool,
        ts: DateTime<Utc>,
    },
    ToolCall {
        tool_use_id: String,
        tool_name: String,
        input: serde_json::Value,
        ts: DateTime<Utc>,
    },
    ToolResult {
        tool_use_id: String,
        output: ToolResultPayload,
        is_error: bool,
        ts: DateTime<Utc>,
    },
    CheckpointReached {
        checkpoint_id: CheckpointId,
        interrupt: Option<InterruptSnapshot>,
        ts: DateTime<Utc>,
    },
    TurnCompleted {
        turn_id: TurnId,
        stop_reason: String,
        ts: DateTime<Utc>,
    },
    TurnAborted {
        turn_id: TurnId,
        reason: String,
        ts: DateTime<Utc>,
    },
    SessionCrashed {
        reason: String,
        exit_code: Option<i32>,
        ts: DateTime<Utc>,
    },
    SessionEnded {
        ts: DateTime<Utc>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum ToolResultPayload {
    /// Spilled MUST come before `Inline` so untagged deserialization picks the
    /// structurally-narrower variant first. `Inline(serde_json::Value)` would
    /// otherwise swallow any JSON object — including spilled-shape ones.
    Spilled {
        hash: String,
        original_len: u64,
        snippet: String,
    },
    Inline(serde_json::Value),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum InterruptSnapshot {
    Hitl {
        question: String,
        options: Vec<String>,
    },
    Pause,
    Drain,
}

impl SessionEvent {
    /// Stable kebab-case kind discriminator, used as the `EventEnvelope::event_type`
    /// when persisting `SessionEvent`s through `DomainEventStore`.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::SessionStarted { .. } => "session_started",
            Self::AgentPromptQueued { .. } => "agent_prompt_queued",
            Self::AgentPromptDelivered { .. } => "agent_prompt_delivered",
            Self::HumanPromptQueued { .. } => "human_prompt_queued",
            Self::HumanPromptDelivered { .. } => "human_prompt_delivered",
            Self::PromptDeliveryFailed { .. } => "prompt_delivery_failed",
            Self::AssistantMessage { .. } => "assistant_message",
            Self::ToolCall { .. } => "tool_call",
            Self::ToolResult { .. } => "tool_result",
            Self::CheckpointReached { .. } => "checkpoint_reached",
            Self::TurnCompleted { .. } => "turn_completed",
            Self::TurnAborted { .. } => "turn_aborted",
            Self::SessionCrashed { .. } => "session_crashed",
            Self::SessionEnded { .. } => "session_ended",
        }
    }
}

//! Provisional `SessionEventDraft` enum used by slice 1a.
//!
//! Slice 1b (Task 3 step 12) deletes this file and rewrites every callsite to
//! consume `nephila_core::SessionEvent` directly. The rollover is **not** a
//! `pub use` re-export: the draft uses `serde_json::Value` for tool payloads,
//! while the real event type uses a typed `ToolResultPayload`. Callers in 1a
//! must not depend on exact draft variant shapes beyond what step 12 provides.

use chrono::{DateTime, Utc};
use uuid::Uuid;

pub type TurnId = Uuid;
pub type CheckpointId = String;
pub type MessageId = String;

#[derive(Debug, Clone)]
pub enum SessionEventDraft {
    SessionStarted {
        resumed: bool,
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
    AgentPromptQueued {
        turn_id: TurnId,
        text: String,
        ts: DateTime<Utc>,
    },
    AgentPromptDelivered {
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
        output: serde_json::Value,
        is_error: bool,
        ts: DateTime<Utc>,
    },
    CheckpointReached {
        checkpoint_id: CheckpointId,
        interrupt: Option<serde_json::Value>,
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

impl SessionEventDraft {
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::SessionStarted { .. } => "SessionStarted",
            Self::HumanPromptQueued { .. } => "HumanPromptQueued",
            Self::HumanPromptDelivered { .. } => "HumanPromptDelivered",
            Self::AgentPromptQueued { .. } => "AgentPromptQueued",
            Self::AgentPromptDelivered { .. } => "AgentPromptDelivered",
            Self::PromptDeliveryFailed { .. } => "PromptDeliveryFailed",
            Self::AssistantMessage { .. } => "AssistantMessage",
            Self::ToolCall { .. } => "ToolCall",
            Self::ToolResult { .. } => "ToolResult",
            Self::CheckpointReached { .. } => "CheckpointReached",
            Self::TurnCompleted { .. } => "TurnCompleted",
            Self::TurnAborted { .. } => "TurnAborted",
            Self::SessionCrashed { .. } => "SessionCrashed",
            Self::SessionEnded { .. } => "SessionEnded",
        }
    }
}

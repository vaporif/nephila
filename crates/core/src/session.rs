use crate::id::{AgentId, CheckpointId};
use crate::session_event::{SessionEvent, SessionId, TurnId};
use nephila_eventsourcing::aggregate::EventSourced;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Claude Code emits `stop_reason: "interrupt"` on TurnCompleted when the
/// session was paused/interrupted mid-turn. Used to distinguish a clean
/// `end_turn` from an interrupt-driven stop.
const STOP_REASON_INTERRUPT: &str = "interrupt";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionPhase {
    Starting,
    Running,
    WaitingHitl,
    Paused,
    Crashed,
    Ended,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckpointRef {
    pub checkpoint_id: CheckpointId,
    pub at_sequence: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Session {
    pub id: SessionId,
    pub agent_id: AgentId,
    pub phase: SessionPhase,
    pub open_turn: Option<TurnId>,
    pub last_checkpoint: Option<CheckpointRef>,
    pub last_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionCommand {
    /// No commands today: the connector emits events directly via the
    /// store. `handle()` exists for symmetry with `Agent` and the future
    /// control protocol; new commands will land here.
    NoOp,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("session in terminal state — no further commands accepted")]
    Terminal,
    #[error("invariant violated: {0}")]
    Invariant(String),
}

impl Session {
    /// Replay/live-stream callers MUST use this — never call `apply` directly.
    /// Sets `last_seq` to the envelope's sequence (assigned by the store) AFTER
    /// applying the event. `CheckpointRef::at_sequence` and the prune-policy
    /// math (`last_seq - 200`) both depend on this field tracking the latest
    /// envelope sequence in the aggregate.
    pub fn apply_envelope(
        self,
        env: &nephila_eventsourcing::envelope::EventEnvelope,
    ) -> Result<Self, SessionError> {
        let event: SessionEvent = serde_json::from_value(env.payload.clone())
            .map_err(|e| SessionError::Invariant(format!("payload decode: {e}")))?;
        let mut next = self.apply(&event);
        next.last_seq = env.sequence();
        Ok(next)
    }
}

impl EventSourced for Session {
    type Event = SessionEvent;
    type Command = SessionCommand;
    type Error = SessionError;

    fn aggregate_type() -> &'static str {
        "session"
    }

    fn aggregate_id(&self) -> String {
        self.id.to_string()
    }

    fn default_state() -> Self {
        Self {
            id: Uuid::nil(),
            agent_id: AgentId(Uuid::nil()),
            phase: SessionPhase::Starting,
            open_turn: None,
            last_checkpoint: None,
            last_seq: 0,
        }
    }

    fn apply(mut self, event: &SessionEvent) -> Self {
        if matches!(self.phase, SessionPhase::Ended) {
            return self;
        }
        match event {
            SessionEvent::SessionStarted {
                session_id,
                agent_id,
                ..
            } => {
                self.id = *session_id;
                self.agent_id = *agent_id;
                self.phase = SessionPhase::Running;
            }
            SessionEvent::AgentPromptQueued { turn_id, .. }
            | SessionEvent::HumanPromptQueued { turn_id, .. } => {
                debug_assert!(
                    self.open_turn.is_none(),
                    "two open turns is an invariant violation"
                );
                self.open_turn = Some(*turn_id);
            }
            SessionEvent::AgentPromptDelivered { .. }
            | SessionEvent::HumanPromptDelivered { .. } => {}
            SessionEvent::PromptDeliveryFailed { turn_id, .. } => {
                if self.open_turn == Some(*turn_id) {
                    self.open_turn = None;
                }
            }
            SessionEvent::AssistantMessage { .. }
            | SessionEvent::ToolCall { .. }
            | SessionEvent::ToolResult { .. } => {}
            SessionEvent::CheckpointReached {
                checkpoint_id,
                interrupt,
                ..
            } => {
                self.last_checkpoint = Some(CheckpointRef {
                    checkpoint_id: *checkpoint_id,
                    at_sequence: self.last_seq,
                });
                match interrupt {
                    Some(crate::session_event::InterruptSnapshot::Hitl { .. }) => {
                        self.phase = SessionPhase::WaitingHitl;
                    }
                    Some(crate::session_event::InterruptSnapshot::Pause) => {
                        self.phase = SessionPhase::Paused;
                    }
                    Some(crate::session_event::InterruptSnapshot::Drain) | None => {}
                }
            }
            SessionEvent::TurnCompleted {
                turn_id,
                stop_reason,
                ..
            } => {
                if self.open_turn == Some(*turn_id) {
                    self.open_turn = None;
                }
                if stop_reason == STOP_REASON_INTERRUPT {
                    self.phase = SessionPhase::Paused;
                } else if matches!(self.phase, SessionPhase::WaitingHitl) {
                    // stays in WaitingHitl until operator answers
                } else {
                    self.phase = SessionPhase::Running;
                }
            }
            SessionEvent::TurnAborted { turn_id, .. } => {
                if self.open_turn == Some(*turn_id) {
                    self.open_turn = None;
                }
            }
            SessionEvent::SessionCrashed { .. } => {
                self.open_turn = None;
                self.phase = SessionPhase::Crashed;
            }
            SessionEvent::SessionEnded { .. } => {
                self.phase = SessionPhase::Ended;
            }
        }
        self
    }

    fn handle(&self, _cmd: SessionCommand) -> Result<Vec<SessionEvent>, SessionError> {
        if matches!(self.phase, SessionPhase::Ended) {
            return Err(SessionError::Terminal);
        }
        Ok(Vec::new())
    }
}

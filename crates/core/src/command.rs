use crate::directive::Directive;
use crate::id::{AgentId, CheckpointId, ObjectiveId};
use std::path::PathBuf;

pub enum OrchestratorCommand {
    Spawn {
        objective_id: ObjectiveId,
        content: String,
        dir: PathBuf,
    },
    SpawnAgent {
        objective_id: ObjectiveId,
        content: String,
        dir: PathBuf,
        spawned_by: AgentId,
    },
    Kill {
        agent_id: AgentId,
    },
    Pause {
        agent_id: AgentId,
    },
    Resume {
        agent_id: AgentId,
    },
    Suspend {
        agent_id: AgentId,
    },
    HitlRespond {
        agent_id: AgentId,
        response: String,
    },
    TokenThreshold {
        agent_id: AgentId,
        directive: Directive,
    },
    AgentExited {
        agent_id: AgentId,
        success: bool,
    },
    Respawn {
        objective_id: ObjectiveId,
        content: String,
        dir: PathBuf,
        restore_checkpoint_id: CheckpointId,
    },
}

// SECURITY (H-S2): do not use `..` in destructuring — every field must be listed
// so new sensitive fields are caught at review time. Adding a variant or a field
// to an existing variant fails compilation here, forcing an explicit redaction
// decision.
impl std::fmt::Debug for OrchestratorCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn {
                objective_id,
                content,
                dir,
            } => f
                .debug_struct("Spawn")
                .field("objective_id", objective_id)
                .field(
                    "content",
                    &format_args!("<redacted: {} bytes>", content.len()),
                )
                .field("dir", dir)
                .finish(),
            Self::SpawnAgent {
                objective_id,
                content,
                dir,
                spawned_by,
            } => f
                .debug_struct("SpawnAgent")
                .field("objective_id", objective_id)
                .field(
                    "content",
                    &format_args!("<redacted: {} bytes>", content.len()),
                )
                .field("dir", dir)
                .field("spawned_by", spawned_by)
                .finish(),
            Self::Kill { agent_id } => f.debug_struct("Kill").field("agent_id", agent_id).finish(),
            Self::Pause { agent_id } => {
                f.debug_struct("Pause").field("agent_id", agent_id).finish()
            }
            Self::Resume { agent_id } => f
                .debug_struct("Resume")
                .field("agent_id", agent_id)
                .finish(),
            Self::Suspend { agent_id } => f
                .debug_struct("Suspend")
                .field("agent_id", agent_id)
                .finish(),
            Self::HitlRespond { agent_id, response } => f
                .debug_struct("HitlRespond")
                .field("agent_id", agent_id)
                .field(
                    "response",
                    &format_args!("<redacted: {} bytes>", response.len()),
                )
                .finish(),
            Self::TokenThreshold {
                agent_id,
                directive,
            } => f
                .debug_struct("TokenThreshold")
                .field("agent_id", agent_id)
                .field("directive", directive)
                .finish(),
            Self::AgentExited { agent_id, success } => f
                .debug_struct("AgentExited")
                .field("agent_id", agent_id)
                .field("success", success)
                .finish(),
            Self::Respawn {
                objective_id,
                content,
                dir,
                restore_checkpoint_id,
            } => f
                .debug_struct("Respawn")
                .field("objective_id", objective_id)
                .field(
                    "content",
                    &format_args!("<redacted: {} bytes>", content.len()),
                )
                .field("dir", dir)
                .field("restore_checkpoint_id", restore_checkpoint_id)
                .finish(),
        }
    }
}

use crate::directive::Directive;
use crate::id::{AgentId, CheckpointVersion, ObjectiveId};
use std::path::PathBuf;

#[derive(Debug)]
pub enum OrchestratorCommand {
    Spawn {
        objective_id: ObjectiveId,
        content: String,
        dir: PathBuf,
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
    Rollback {
        agent_id: AgentId,
        version: CheckpointVersion,
    },
    ListCheckpoints {
        agent_id: AgentId,
    },
    HitlRespond {
        agent_id: AgentId,
        response: String,
    },
    RequestReset {
        agent_id: AgentId,
    },
    TokenThreshold {
        agent_id: AgentId,
        directive: Directive,
    },
}

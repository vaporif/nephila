use meridian_core::id::{AgentId, CheckpointVersion, ObjectiveId};
use std::path::PathBuf;

#[derive(Debug)]
pub enum TuiCommand {
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
    ListCheckpoints {
        agent_id: AgentId,
    },
    Rollback {
        agent_id: AgentId,
        version: CheckpointVersion,
    },
    HitlRespond {
        agent_id: AgentId,
        response: String,
    },
}

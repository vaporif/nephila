use std::borrow::Cow;

use rmcp::ErrorData;
use rmcp::handler::server::router::tool::{AsyncTool, ToolBase};
use rmcp::schemars;
use serde::{Deserialize, Serialize};

use crate::server::{NephilaMcpServer, nephila_err, parse_agent_id};
use nephila_core::agent::{Agent, SpawnOrigin};
use nephila_core::id::{AgentId, CheckpointId};
use nephila_core::objective::{NewObjective, ObjectiveStatus};
use nephila_core::store::{AgentStore, CheckpointStore, ObjectiveStore};

#[derive(Debug, Deserialize, schemars::JsonSchema, Default)]
pub struct ForkAgentParams {
    /// The agent ID initiating the fork.
    pub agent_id: String,
    /// The checkpoint ID to fork from.
    pub source_checkpoint_id: String,
    /// Human-readable branch label.
    pub branch_label: String,
    /// Optional directive override for the forked agent.
    pub directive_override: Option<String>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ForkAgentOutput {
    pub success: bool,
    pub forked_agent_id: Option<String>,
}

pub struct ForkAgentTool;

impl ToolBase for ForkAgentTool {
    type Parameter = ForkAgentParams;
    type Output = ForkAgentOutput;
    type Error = ErrorData;

    fn name() -> Cow<'static, str> {
        "fork_agent".into()
    }

    fn description() -> Option<Cow<'static, str>> {
        Some("Fork a new agent from a past checkpoint to retry with a different approach.".into())
    }
}

impl AsyncTool<NephilaMcpServer> for ForkAgentTool {
    async fn invoke(
        service: &NephilaMcpServer,
        params: Self::Parameter,
    ) -> Result<Self::Output, Self::Error> {
        let source_agent_id = parse_agent_id(&params.agent_id)?;
        let source_checkpoint_id: CheckpointId = params
            .source_checkpoint_id
            .parse::<uuid::Uuid>()
            .map(CheckpointId)
            .map_err(|e| {
                ErrorData::invalid_params(format!("invalid source_checkpoint_id: {e}"), None)
            })?;

        CheckpointStore::get(service.ferrex.as_ref(), source_checkpoint_id)
            .await
            .map_err(nephila_err)?
            .ok_or_else(|| {
                ErrorData::invalid_params("source checkpoint not found".to_string(), None)
            })?;

        let source_agent = service
            .sqlite
            .get(source_agent_id)
            .await
            .map_err(nephila_err)?
            .ok_or_else(|| ErrorData::invalid_params("source agent not found".to_string(), None))?;

        let sub_obj_id = service
            .sqlite
            .create(NewObjective {
                parent_id: Some(source_agent.objective_id),
                agent_id: None,
                description: format!("Fork: {}", params.branch_label),
            })
            .await
            .map_err(nephila_err)?;

        service
            .sqlite
            .update_status(sub_obj_id, ObjectiveStatus::InProgress)
            .await
            .map_err(nephila_err)?;

        let new_agent_id = AgentId::new();
        let mut new_agent = Agent::new(
            new_agent_id,
            sub_obj_id,
            source_agent.directory.clone(),
            SpawnOrigin::Fork {
                source_agent_id,
                source_checkpoint_id,
            },
            params.directive_override,
        );
        new_agent.restore_checkpoint_id = Some(source_checkpoint_id);

        service
            .sqlite
            .register(new_agent)
            .await
            .map_err(nephila_err)?;

        service
            .sqlite
            .assign_agent(sub_obj_id, new_agent_id)
            .await
            .map_err(nephila_err)?;

        let _ = service
            .cmd_tx
            .send(nephila_core::command::OrchestratorCommand::Spawn {
                objective_id: sub_obj_id,
                content: format!("Fork: {}", params.branch_label),
                dir: source_agent.directory,
                restore_checkpoint_id: None,
            })
            .await;

        Ok(ForkAgentOutput {
            success: true,
            forked_agent_id: Some(new_agent_id.0.to_string()),
        })
    }
}

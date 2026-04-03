use std::borrow::Cow;

use rmcp::ErrorData;
use rmcp::handler::server::router::tool::{AsyncTool, ToolBase};
use rmcp::schemars;
use serde::{Deserialize, Serialize};

use crate::server::{MeridianMcpServer, meridian_err, parse_agent_id};
use meridian_core::agent::{Agent, SpawnOrigin};
use meridian_core::embedding::EmbeddingProvider;
use meridian_core::id::{AgentId, CheckpointId};
use meridian_core::objective::{NewObjective, ObjectiveStatus};
use meridian_core::store::{
    AgentStore, CheckpointStore, InterruptStore, McpEventLog, MemoryStore, ObjectiveStore,
};

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

impl<S, E> AsyncTool<MeridianMcpServer<S, E>> for ForkAgentTool
where
    S: AgentStore
        + CheckpointStore
        + MemoryStore
        + ObjectiveStore
        + McpEventLog
        + InterruptStore
        + Send
        + Sync
        + 'static,
    E: EmbeddingProvider + 'static,
{
    async fn invoke(
        service: &MeridianMcpServer<S, E>,
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

        // Verify source checkpoint exists
        let _checkpoint = CheckpointStore::get(service.store.as_ref(), source_checkpoint_id)
            .await
            .map_err(meridian_err)?
            .ok_or_else(|| {
                ErrorData::invalid_params("source checkpoint not found".to_string(), None)
            })?;

        // Get source agent for objective and directory
        let source_agent = AgentStore::get(service.store.as_ref(), source_agent_id)
            .await
            .map_err(meridian_err)?
            .ok_or_else(|| ErrorData::invalid_params("source agent not found".to_string(), None))?;

        // Create sub-objective
        let sub_obj_id = ObjectiveStore::create(
            service.store.as_ref(),
            NewObjective {
                parent_id: Some(source_agent.objective_id),
                agent_id: None,
                description: format!("Fork: {}", params.branch_label),
            },
        )
        .await
        .map_err(meridian_err)?;

        ObjectiveStore::update_status(
            service.store.as_ref(),
            sub_obj_id,
            ObjectiveStatus::InProgress,
        )
        .await
        .map_err(meridian_err)?;

        // Create new agent with fork origin
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

        AgentStore::register(service.store.as_ref(), new_agent)
            .await
            .map_err(meridian_err)?;

        ObjectiveStore::assign_agent(service.store.as_ref(), sub_obj_id, new_agent_id)
            .await
            .map_err(meridian_err)?;

        // Send spawn command
        let _ = service
            .cmd_tx
            .send(meridian_core::command::OrchestratorCommand::Spawn {
                objective_id: sub_obj_id,
                content: format!("Fork: {}", params.branch_label),
                dir: source_agent.directory,
            })
            .await;

        Ok(ForkAgentOutput {
            success: true,
            forked_agent_id: Some(new_agent_id.0.to_string()),
        })
    }
}

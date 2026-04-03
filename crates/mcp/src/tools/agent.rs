use std::borrow::Cow;
use std::path::PathBuf;
use std::time::Duration;

use rmcp::ErrorData;
use rmcp::handler::server::router::tool::{AsyncTool, ToolBase};
use rmcp::schemars;
use serde::{Deserialize, Serialize};

use meridian_core::command::OrchestratorCommand;
use meridian_core::embedding::EmbeddingProvider;
use meridian_core::event::BusEvent;
use meridian_core::id::ObjectiveId;
use meridian_core::objective::NewObjective;
use meridian_core::store::{
    AgentStore, CheckpointStore, InterruptStore, McpEventLog, MemoryStore, ObjectiveStore,
};

use crate::server::{MeridianMcpServer, meridian_err, parse_agent_id};

// ── spawn_agent ──

#[derive(Debug, Deserialize, schemars::JsonSchema, Default)]
pub struct SpawnAgentParams {
    /// The agent ID of the requesting (parent) agent.
    pub requesting_agent_id: String,
    /// The objective description for the child agent.
    pub objective: String,
    /// Working directory for the child agent.
    pub directory: String,
    /// Optional parent objective ID to link the child's objective under.
    #[serde(default)]
    pub parent_objective_id: Option<String>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct SpawnAgentOutput {
    pub spawned: bool,
    pub agent_id: Option<String>,
    pub objective_id: Option<String>,
    pub error: Option<String>,
}

pub struct SpawnAgentTool;

impl ToolBase for SpawnAgentTool {
    type Parameter = SpawnAgentParams;
    type Output = SpawnAgentOutput;
    type Error = ErrorData;

    fn name() -> Cow<'static, str> {
        "spawn_agent".into()
    }

    fn description() -> Option<Cow<'static, str>> {
        Some(
            "Request Meridian to spawn a child agent with a given objective and working directory."
                .into(),
        )
    }
}

impl<S, E> AsyncTool<MeridianMcpServer<S, E>> for SpawnAgentTool
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
        let spawned_by = parse_agent_id(&params.requesting_agent_id)?;

        let parent_obj_id: Option<ObjectiveId> = params
            .parent_objective_id
            .as_deref()
            .map(|s| {
                s.parse::<uuid::Uuid>().map(ObjectiveId).map_err(|e| {
                    ErrorData::invalid_params(format!("invalid parent_objective_id: {e}"), None)
                })
            })
            .transpose()?;

        let objective_id = service
            .store
            .create(NewObjective {
                parent_id: parent_obj_id,
                agent_id: None,
                description: params.objective.clone(),
            })
            .await
            .map_err(meridian_err)?;

        let dir = PathBuf::from(&params.directory);

        // Subscribe before sending so we don't miss the response
        let mut event_rx = service.event_tx.subscribe();

        service
            .cmd_tx
            .send(OrchestratorCommand::SpawnAgent {
                objective_id,
                content: params.objective,
                dir,
                spawned_by,
            })
            .await
            .map_err(|e| {
                ErrorData::internal_error(format!("failed to send spawn command: {e}"), None)
            })?;

        let timeout = Duration::from_secs(10);
        let result = tokio::time::timeout(timeout, async {
            loop {
                match event_rx.recv().await {
                    Ok(BusEvent::AgentSessionReady { agent_id, .. }) => {
                        if let Ok(Some(agent)) =
                            AgentStore::get(service.store.as_ref(), agent_id).await
                            && agent.origin.spawned_by() == Some(spawned_by)
                            && agent.objective_id == objective_id
                        {
                            return Some(agent_id);
                        }
                    }
                    Err(_) => return None,
                    _ => continue,
                }
            }
        })
        .await;

        match result {
            Ok(Some(agent_id)) => Ok(SpawnAgentOutput {
                spawned: true,
                agent_id: Some(agent_id.0.to_string()),
                objective_id: Some(objective_id.0.to_string()),
                error: None,
            }),
            Ok(None) => Ok(SpawnAgentOutput {
                spawned: false,
                agent_id: None,
                objective_id: None,
                error: Some("event channel closed".into()),
            }),
            Err(_) => Ok(SpawnAgentOutput {
                spawned: false,
                agent_id: None,
                objective_id: Some(objective_id.0.to_string()),
                error: Some("spawn timed out or depth limit exceeded".into()),
            }),
        }
    }
}

// ── get_agent_status ──

#[derive(Debug, Deserialize, schemars::JsonSchema, Default)]
pub struct GetAgentStatusParams {
    /// The agent ID to check status for.
    pub agent_id: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct GetAgentStatusOutput {
    pub found: bool,
    pub agent_id: Option<String>,
    pub state: Option<String>,
    pub directive: Option<String>,
    pub objective_id: Option<String>,
    pub checkpoint_id: Option<String>,
    pub spawned_by: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

pub struct GetAgentStatusTool;

impl ToolBase for GetAgentStatusTool {
    type Parameter = GetAgentStatusParams;
    type Output = GetAgentStatusOutput;
    type Error = ErrorData;

    fn name() -> Cow<'static, str> {
        "get_agent_status".into()
    }

    fn description() -> Option<Cow<'static, str>> {
        Some("Check the current status and state of a spawned agent.".into())
    }
}

impl<S, E> AsyncTool<MeridianMcpServer<S, E>> for GetAgentStatusTool
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
        let agent_id = parse_agent_id(&params.agent_id)?;

        let agent = AgentStore::get(service.store.as_ref(), agent_id)
            .await
            .map_err(meridian_err)?;

        match agent {
            Some(a) => Ok(GetAgentStatusOutput {
                found: true,
                agent_id: Some(a.id.0.to_string()),
                state: Some(a.state.to_string()),
                directive: Some(a.directive.to_string()),
                objective_id: Some(a.objective_id.0.to_string()),
                checkpoint_id: a.checkpoint_id.map(|v| v.to_string()),
                spawned_by: a.origin.spawned_by().map(|id| id.0.to_string()),
                created_at: Some(a.created_at.to_rfc3339()),
                updated_at: Some(a.updated_at.to_rfc3339()),
            }),
            None => Ok(GetAgentStatusOutput {
                found: false,
                agent_id: None,
                state: None,
                directive: None,
                objective_id: None,
                checkpoint_id: None,
                spawned_by: None,
                created_at: None,
                updated_at: None,
            }),
        }
    }
}

// ── get_event_log ──

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetEventLogParams {
    /// The agent ID to retrieve events for.
    pub agent_id: String,
    /// Maximum number of events to return (default: 50).
    #[serde(default = "default_event_limit")]
    pub limit: usize,
}

fn default_event_limit() -> usize {
    50
}

impl Default for GetEventLogParams {
    fn default() -> Self {
        Self {
            agent_id: String::new(),
            limit: default_event_limit(),
        }
    }
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct GetEventLogOutput {
    pub events_json: String,
    pub count: usize,
}

pub struct GetEventLogTool;

impl ToolBase for GetEventLogTool {
    type Parameter = GetEventLogParams;
    type Output = GetEventLogOutput;
    type Error = ErrorData;

    fn name() -> Cow<'static, str> {
        "get_event_log".into()
    }

    fn description() -> Option<Cow<'static, str>> {
        Some("Retrieve recent MCP events for an agent.".into())
    }
}

impl<S, E> AsyncTool<MeridianMcpServer<S, E>> for GetEventLogTool
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
        let agent_id = parse_agent_id(&params.agent_id)?;

        let events = service
            .store
            .get_events(agent_id, None, params.limit)
            .await
            .map_err(meridian_err)?;

        let count = events.len();
        let events_json = serde_json::to_string(&events)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        Ok(GetEventLogOutput { events_json, count })
    }
}

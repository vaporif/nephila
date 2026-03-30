use std::borrow::Cow;

use rmcp::ErrorData;
use rmcp::handler::server::router::tool::{AsyncTool, ToolBase};
use rmcp::schemars;
use serde::{Deserialize, Serialize};

use crate::server::MeridianMcpServer;
use meridian_core::store::{
    AgentStore, CheckpointStore, EventStore, HitlStore, MemoryStore, ObjectiveStore,
};

#[derive(Debug, Deserialize, schemars::JsonSchema, Default)]
pub struct GetObjectiveTreeParams {
    /// The root objective ID to retrieve the tree for.
    pub root_id: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct GetObjectiveTreeOutput {
    /// The objective tree serialized as JSON.
    pub tree_json: String,
}

pub struct GetObjectiveTreeTool;

impl ToolBase for GetObjectiveTreeTool {
    type Parameter = GetObjectiveTreeParams;
    type Output = GetObjectiveTreeOutput;
    type Error = ErrorData;

    fn name() -> Cow<'static, str> {
        "get_objective_tree".into()
    }

    fn description() -> Option<Cow<'static, str>> {
        Some("Get the objective tree starting from the given root.".into())
    }
}

impl<S> AsyncTool<MeridianMcpServer<S>> for GetObjectiveTreeTool
where
    S: AgentStore
        + CheckpointStore
        + MemoryStore
        + ObjectiveStore
        + EventStore
        + HitlStore
        + Send
        + Sync
        + 'static,
{
    async fn invoke(
        _service: &MeridianMcpServer<S>,
        _params: Self::Parameter,
    ) -> Result<Self::Output, Self::Error> {
        Ok(GetObjectiveTreeOutput {
            tree_json: "{}".to_owned(),
        })
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema, Default)]
pub struct UpdateObjectiveParams {
    /// The objective ID to update.
    pub objective_id: String,
    /// New status: "pending", "in_progress", "done", or "blocked".
    pub status: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct UpdateObjectiveOutput {
    pub updated: bool,
}

pub struct UpdateObjectiveTool;

impl ToolBase for UpdateObjectiveTool {
    type Parameter = UpdateObjectiveParams;
    type Output = UpdateObjectiveOutput;
    type Error = ErrorData;

    fn name() -> Cow<'static, str> {
        "update_objective".into()
    }

    fn description() -> Option<Cow<'static, str>> {
        Some("Update an objective's status.".into())
    }
}

impl<S> AsyncTool<MeridianMcpServer<S>> for UpdateObjectiveTool
where
    S: AgentStore
        + CheckpointStore
        + MemoryStore
        + ObjectiveStore
        + EventStore
        + HitlStore
        + Send
        + Sync
        + 'static,
{
    async fn invoke(
        _service: &MeridianMcpServer<S>,
        _params: Self::Parameter,
    ) -> Result<Self::Output, Self::Error> {
        Ok(UpdateObjectiveOutput { updated: true })
    }
}

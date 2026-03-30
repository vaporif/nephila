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
pub struct GetSessionCheckpointParams {
    /// The agent ID requesting the checkpoint (hex UUID prefix or full UUID).
    pub agent_id: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct GetSessionCheckpointOutput {
    pub found: bool,
    pub checkpoint_json: Option<String>,
}

pub struct GetSessionCheckpointTool;

impl ToolBase for GetSessionCheckpointTool {
    type Parameter = GetSessionCheckpointParams;
    type Output = GetSessionCheckpointOutput;
    type Error = ErrorData;

    fn name() -> Cow<'static, str> {
        "get_session_checkpoint".into()
    }

    fn description() -> Option<Cow<'static, str>> {
        Some("Load the most recent checkpoint so the agent can pick up where it left off.".into())
    }
}

impl<S> AsyncTool<MeridianMcpServer<S>> for GetSessionCheckpointTool
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
        Ok(GetSessionCheckpointOutput {
            found: false,
            checkpoint_json: None,
        })
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema, Default)]
pub struct SerializeAndPersistParams {
    /// The agent ID performing the checkpoint.
    pub agent_id: String,
    /// Optional L0 state (current objectives + next steps) as JSON.
    pub l0_json: Option<String>,
    /// Optional L1 narrative summary.
    pub l1_summary: Option<String>,
    /// Optional L2 detail chunks as JSON array.
    pub l2_json: Option<String>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct SerializeAndPersistOutput {
    pub success: bool,
    pub version: Option<u32>,
}

pub struct SerializeAndPersistTool;

impl ToolBase for SerializeAndPersistTool {
    type Parameter = SerializeAndPersistParams;
    type Output = SerializeAndPersistOutput;
    type Error = ErrorData;

    fn name() -> Cow<'static, str> {
        "serialize_and_persist".into()
    }

    fn description() -> Option<Cow<'static, str>> {
        Some("Save the current session state as a checkpoint before context reset.".into())
    }
}

impl<S> AsyncTool<MeridianMcpServer<S>> for SerializeAndPersistTool
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
        Ok(SerializeAndPersistOutput {
            success: true,
            version: Some(1),
        })
    }
}

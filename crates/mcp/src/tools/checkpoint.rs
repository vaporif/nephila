use std::borrow::Cow;

use rmcp::ErrorData;
use rmcp::handler::server::router::tool::{AsyncTool, ToolBase};
use rmcp::schemars;
use serde::{Deserialize, Serialize};

use crate::server::{MeridianMcpServer, meridian_err, parse_agent_id};
use meridian_core::checkpoint::{L0State, L2Chunk};
use meridian_core::embedding::EmbeddingProvider;
use meridian_core::event::BusEvent;
use meridian_core::id::CheckpointVersion;
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

impl<S, E> AsyncTool<MeridianMcpServer<S, E>> for GetSessionCheckpointTool
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
    E: EmbeddingProvider + 'static,
{
    async fn invoke(
        service: &MeridianMcpServer<S, E>,
        params: Self::Parameter,
    ) -> Result<Self::Output, Self::Error> {
        let agent_id = parse_agent_id(&params.agent_id)?;
        match service.store.get_latest(agent_id).await.map_err(meridian_err)? {
            Some(cp) => {
                let json = serde_json::to_string(&cp)
                    .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
                Ok(GetSessionCheckpointOutput {
                    found: true,
                    checkpoint_json: Some(json),
                })
            }
            None => Ok(GetSessionCheckpointOutput {
                found: false,
                checkpoint_json: None,
            }),
        }
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

impl<S, E> AsyncTool<MeridianMcpServer<S, E>> for SerializeAndPersistTool
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
    E: EmbeddingProvider + 'static,
{
    async fn invoke(
        service: &MeridianMcpServer<S, E>,
        params: Self::Parameter,
    ) -> Result<Self::Output, Self::Error> {
        let agent_id = parse_agent_id(&params.agent_id)?;

        let l0: L0State = match params.l0_json {
            Some(json) => serde_json::from_str(&json)
                .map_err(|e| ErrorData::invalid_params(format!("invalid l0_json: {e}"), None))?,
            None => L0State {
                objectives: vec![],
                next_steps: vec![],
            },
        };

        let l1 = params.l1_summary.unwrap_or_default();

        let l2_chunks: Vec<L2Chunk> = match params.l2_json {
            Some(json) => serde_json::from_str(&json)
                .map_err(|e| ErrorData::invalid_params(format!("invalid l2_json: {e}"), None))?,
            None => vec![],
        };

        let versions = service.store.list_versions(agent_id).await.map_err(meridian_err)?;
        let version = versions
            .iter()
            .max()
            .map(|v| v.next())
            .unwrap_or(CheckpointVersion(1));

        let l2_embeddings = if l2_chunks.is_empty() {
            vec![]
        } else {
            let texts: Vec<&str> = l2_chunks.iter().map(|c| c.content.as_str()).collect();
            service.embedder.embed_batch(&texts).await.map_err(|e| {
                ErrorData::internal_error(format!("embedding failed: {e}"), None)
            })?
        };

        CheckpointStore::save(
            service.store.as_ref(),
            agent_id,
            version,
            &l0,
            &l1,
            &l2_chunks,
            &l2_embeddings,
        )
        .await
        .map_err(meridian_err)?;

        let _ = service.event_tx.send(BusEvent::CheckpointSaved {
            agent_id,
            version,
        });

        Ok(SerializeAndPersistOutput {
            success: true,
            version: Some(version.0),
        })
    }
}

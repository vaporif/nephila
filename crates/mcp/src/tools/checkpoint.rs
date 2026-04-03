use std::borrow::Cow;
use std::collections::BTreeMap;

use rmcp::ErrorData;
use rmcp::handler::server::router::tool::{AsyncTool, ToolBase};
use rmcp::schemars;
use serde::{Deserialize, Serialize};

use crate::server::{MeridianMcpServer, meridian_err, parse_agent_id};
use meridian_core::channel::{merge_channels, validate_channels};
use meridian_core::checkpoint::{ChannelEntry, CheckpointNode, L2Chunk};
use meridian_core::embedding::EmbeddingProvider;
use meridian_core::event::BusEvent;
use meridian_core::id::CheckpointId;
use meridian_core::store::{
    AgentStore, CheckpointStore, InterruptStore, McpEventLog, MemoryStore, ObjectiveStore,
};

#[derive(Debug, Deserialize, schemars::JsonSchema, Default)]
pub struct GetSessionCheckpointParams {
    /// The agent ID requesting the checkpoint (hex UUID prefix or full UUID).
    pub agent_id: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct GetSessionCheckpointOutput {
    pub found: bool,
    pub channels: Option<serde_json::Value>,
    pub interrupt: Option<serde_json::Value>,
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

        // Check if agent has a restore_checkpoint_id set (for forks or explicit restore)
        let agent = AgentStore::get(service.store.as_ref(), agent_id)
            .await
            .map_err(meridian_err)?;

        let checkpoint_id = match agent.and_then(|a| a.restore_checkpoint_id) {
            Some(id) => Some(id),
            None => CheckpointStore::get_latest(service.store.as_ref(), agent_id)
                .await
                .map_err(meridian_err)?
                .map(|n| n.id),
        };

        let checkpoint_id = match checkpoint_id {
            Some(id) => id,
            None => {
                return Ok(GetSessionCheckpointOutput {
                    found: false,
                    channels: None,
                    interrupt: None,
                });
            }
        };

        let ancestry = CheckpointStore::get_ancestry(service.store.as_ref(), checkpoint_id)
            .await
            .map_err(meridian_err)?;

        if ancestry.is_empty() {
            return Ok(GetSessionCheckpointOutput {
                found: false,
                channels: None,
                interrupt: None,
            });
        }

        let merged = merge_channels(&ancestry);
        let channels_value = serde_json::to_value(&merged)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        // Check for interrupt response
        let interrupt = if let Some(last) = ancestry.last() {
            if last.interrupt.is_some() {
                let pending = InterruptStore::get_pending(service.store.as_ref(), agent_id)
                    .await
                    .map_err(meridian_err)?;
                pending.map(|req| {
                    serde_json::json!({
                        "type": req.interrupt_type,
                        "payload": req.payload,
                        "response": req.response,
                    })
                })
            } else {
                None
            }
        } else {
            None
        };

        Ok(GetSessionCheckpointOutput {
            found: true,
            channels: Some(channels_value),
            interrupt,
        })
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema, Default)]
pub struct SerializeAndPersistParams {
    pub agent_id: String,
    /// Channel map as JSON: {"objectives": {"reducer": "overwrite", "value": [...]}, ...}
    pub channels: String,
    /// Optional L2 detail chunks as JSON array.
    pub l2_json: Option<String>,
    /// Optional branch label for this checkpoint.
    pub branch_label: Option<String>,
    /// Optional L2 namespace (defaults to "general").
    pub l2_namespace: Option<String>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct SerializeAndPersistOutput {
    pub success: bool,
    pub checkpoint_id: Option<String>,
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

        let channels: BTreeMap<String, ChannelEntry> = serde_json::from_str(&params.channels)
            .map_err(|e| ErrorData::invalid_params(format!("invalid channels JSON: {e}"), None))?;

        validate_channels(&channels).map_err(|e| {
            ErrorData::invalid_params(format!("channel validation failed: {e}"), None)
        })?;

        let l2_chunks: Vec<L2Chunk> = match params.l2_json {
            Some(json) => serde_json::from_str(&json)
                .map_err(|e| ErrorData::invalid_params(format!("invalid l2_json: {e}"), None))?,
            None => vec![],
        };

        let parent_id = CheckpointStore::get_latest(service.store.as_ref(), agent_id)
            .await
            .map_err(meridian_err)?
            .map(|n| n.id);

        let node = CheckpointNode {
            id: CheckpointId::new(),
            agent_id,
            parent_id,
            branch_label: params.branch_label,
            channels,
            l2_namespace: params.l2_namespace.unwrap_or_else(|| "general".into()),
            interrupt: None,
            created_at: chrono::Utc::now(),
        };

        let l2_embeddings =
            if l2_chunks.is_empty() {
                vec![]
            } else {
                let texts: Vec<&str> = l2_chunks.iter().map(|c| c.content.as_str()).collect();
                service.embedder.embed_batch(&texts).await.map_err(|e| {
                    ErrorData::internal_error(format!("embedding failed: {e}"), None)
                })?
            };

        let checkpoint_id = node.id;
        CheckpointStore::save(service.store.as_ref(), &node, &l2_chunks, &l2_embeddings)
            .await
            .map_err(meridian_err)?;

        AgentStore::set_checkpoint_id(service.store.as_ref(), agent_id, checkpoint_id)
            .await
            .map_err(meridian_err)?;

        // Clear restore_checkpoint_id if it was set
        let agent = AgentStore::get(service.store.as_ref(), agent_id)
            .await
            .map_err(meridian_err)?;
        if agent
            .map(|a| a.restore_checkpoint_id.is_some())
            .unwrap_or(false)
        {
            AgentStore::set_restore_checkpoint(service.store.as_ref(), agent_id, None)
                .await
                .map_err(meridian_err)?;
        }

        let _ = service.event_tx.send(BusEvent::CheckpointSaved {
            agent_id,
            checkpoint_id,
        });

        Ok(SerializeAndPersistOutput {
            success: true,
            checkpoint_id: Some(checkpoint_id.to_string()),
        })
    }
}

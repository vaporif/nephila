use std::borrow::Cow;
use std::collections::BTreeMap;

use rmcp::ErrorData;
use rmcp::handler::server::router::tool::{AsyncTool, ToolBase};
use rmcp::schemars;
use serde::{Deserialize, Serialize};

use crate::server::{NephilaMcpServer, nephila_err, parse_agent_id};
use nephila_core::channel::{merge_channels, validate_channels};
use nephila_core::checkpoint::{ChannelEntry, CheckpointNode, L2Chunk};
// `BusEvent` import removed in slice-1b step 18a — the connector reader is the
// sole producer of checkpoint events post-1b.
// use nephila_core::event::BusEvent;
use nephila_core::id::CheckpointId;
use nephila_core::store::{AgentStore, CheckpointStore, InterruptStore};

#[derive(Debug, Deserialize, schemars::JsonSchema, Default)]
pub struct GetSessionCheckpointParams {
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

impl AsyncTool<NephilaMcpServer> for GetSessionCheckpointTool {
    async fn invoke(
        service: &NephilaMcpServer,
        params: Self::Parameter,
    ) -> Result<Self::Output, Self::Error> {
        let agent_id = parse_agent_id(&params.agent_id)?;

        let agent = service.sqlite.get(agent_id).await.map_err(nephila_err)?;

        let checkpoint_id = match agent.and_then(|a| a.restore_checkpoint_id) {
            Some(id) => Some(id),
            None => service
                .ferrex
                .get_latest(agent_id)
                .await
                .map_err(nephila_err)?
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

        let ancestry = service
            .ferrex
            .get_ancestry(checkpoint_id)
            .await
            .map_err(nephila_err)?;

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

        let interrupt = if let Some(last) = ancestry.last() {
            if last.interrupt.is_some() {
                let pending = service
                    .sqlite
                    .get_pending(agent_id)
                    .await
                    .map_err(nephila_err)?;
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
    pub channels: String,
    pub l2_json: Option<String>,
    pub branch_label: Option<String>,
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

impl AsyncTool<NephilaMcpServer> for SerializeAndPersistTool {
    async fn invoke(
        service: &NephilaMcpServer,
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

        let parent_id = service
            .ferrex
            .get_latest(agent_id)
            .await
            .map_err(nephila_err)?
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

        let checkpoint_id = node.id;

        service
            .ferrex
            .save(&node, &l2_chunks)
            .await
            .map_err(nephila_err)?;

        service
            .sqlite
            .set_checkpoint_id(agent_id, checkpoint_id)
            .await
            .map_err(nephila_err)?;

        let agent = service.sqlite.get(agent_id).await.map_err(nephila_err)?;
        if agent
            .map(|a| a.restore_checkpoint_id.is_some())
            .unwrap_or(false)
        {
            service
                .sqlite
                .set_restore_checkpoint(agent_id, None)
                .await
                .map_err(nephila_err)?;
        }

        // Slice-1b (Task 3 step 18a): the connector reader is the sole
        // producer of `SessionEvent::CheckpointReached`. Emitting
        // `BusEvent::CheckpointSaved` here would make BOTH paths fire and
        // the TUI would render a double HITL modal. The bus arm itself stays
        // in place until slice 6 deletion; we just stop emitting.
        let _ = checkpoint_id; // keep the binding stable for future use
        let _ = agent_id;

        Ok(SerializeAndPersistOutput {
            success: true,
            checkpoint_id: Some(checkpoint_id.to_string()),
        })
    }
}

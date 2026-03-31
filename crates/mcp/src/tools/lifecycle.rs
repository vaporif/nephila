use std::borrow::Cow;

use rmcp::ErrorData;
use rmcp::handler::server::router::tool::{AsyncTool, ToolBase};
use rmcp::schemars;
use serde::{Deserialize, Serialize};

use crate::server::{MeridianMcpServer, meridian_err, parse_agent_id};
use meridian_core::command::OrchestratorCommand;
use meridian_core::directive::Directive;
use meridian_core::embedding::EmbeddingProvider;
use meridian_core::event::BusEvent;
use meridian_core::store::{
    AgentStore, CheckpointStore, HitlStore, McpEventLog, MemoryStore, ObjectiveStore,
};

#[derive(Debug, Deserialize, schemars::JsonSchema, Default)]
pub struct ReportTokenEstimateParams {
    /// The agent ID reporting its token usage.
    pub agent_id: String,
    /// Estimated tokens used so far.
    pub tokens_used: u64,
    /// Estimated tokens remaining in the context window.
    pub tokens_remaining: u64,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ReportTokenEstimateOutput {
    pub acknowledged: bool,
}

pub struct ReportTokenEstimateTool;

impl ToolBase for ReportTokenEstimateTool {
    type Parameter = ReportTokenEstimateParams;
    type Output = ReportTokenEstimateOutput;
    type Error = ErrorData;

    fn name() -> Cow<'static, str> {
        "report_token_estimate".into()
    }

    fn description() -> Option<Cow<'static, str>> {
        Some(
            "Report current token usage so the lifecycle manager knows when to trigger draining."
                .into(),
        )
    }
}

impl<S, E> AsyncTool<MeridianMcpServer<S, E>> for ReportTokenEstimateTool
where
    S: AgentStore
        + CheckpointStore
        + MemoryStore
        + ObjectiveStore
        + McpEventLog
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
        let total = params.tokens_used + params.tokens_remaining;
        let pct = if total > 0 {
            (params.tokens_used * 100 / total) as u8
        } else {
            0
        };

        if pct >= service.config.lifecycle.token_force_kill_pct {
            if let Err(e) = service
                .cmd_tx
                .send(OrchestratorCommand::TokenThreshold {
                    agent_id,
                    directive: Directive::Abort,
                })
                .await
            {
                tracing::warn!(%agent_id, %e, "failed to send token threshold abort command");
            }
        } else if pct >= service.config.lifecycle.context_threshold_pct
            && let Err(e) = service
                .cmd_tx
                .send(OrchestratorCommand::TokenThreshold {
                    agent_id,
                    directive: Directive::PrepareReset,
                })
                .await
        {
            tracing::warn!(%agent_id, %e, "failed to send token threshold reset command");
        }

        let _ = service.event_tx.send(BusEvent::TokenReport {
            agent_id,
            used: params.tokens_used,
            remaining: params.tokens_remaining,
        });

        Ok(ReportTokenEstimateOutput { acknowledged: true })
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema, Default)]
pub struct GetDirectiveParams {
    /// The agent ID requesting its current directive.
    pub agent_id: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct GetDirectiveOutput {
    /// One of: "continue", "prepare_reset", "pause", "abort".
    pub directive: String,
    /// Optional human-readable reason for the directive.
    pub reason: Option<String>,
    /// Optional injected message for the agent.
    pub injected_message: Option<String>,
}

pub struct GetDirectiveTool;

impl ToolBase for GetDirectiveTool {
    type Parameter = GetDirectiveParams;
    type Output = GetDirectiveOutput;
    type Error = ErrorData;

    fn name() -> Cow<'static, str> {
        "get_directive".into()
    }

    fn description() -> Option<Cow<'static, str>> {
        Some(
            "Check what the agent should do next: continue, prepare_reset, pause, or abort.".into(),
        )
    }
}

impl<S, E> AsyncTool<MeridianMcpServer<S, E>> for GetDirectiveTool
where
    S: AgentStore
        + CheckpointStore
        + MemoryStore
        + ObjectiveStore
        + McpEventLog
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
        let directive = service
            .store
            .get_directive(agent_id)
            .await
            .map_err(meridian_err)?;

        let reason = match directive {
            Directive::PrepareReset => Some("token threshold reached".to_owned()),
            Directive::Pause => Some("paused by operator".to_owned()),
            Directive::Abort => Some("force kill".to_owned()),
            Directive::Continue => None,
        };

        let injected_message = AgentStore::get(service.store.as_ref(), agent_id)
            .await
            .map_err(meridian_err)?
            .and_then(|a| a.injected_message.clone());

        if injected_message.is_some() {
            service
                .store
                .set_injected_message(agent_id, None)
                .await
                .map_err(meridian_err)?;
        }

        Ok(GetDirectiveOutput {
            directive: directive.to_string(),
            reason,
            injected_message,
        })
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema, Default)]
pub struct RequestContextResetParams {
    /// The agent ID requesting a context reset.
    pub agent_id: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct RequestContextResetOutput {
    pub accepted: bool,
}

pub struct RequestContextResetTool;

impl ToolBase for RequestContextResetTool {
    type Parameter = RequestContextResetParams;
    type Output = RequestContextResetOutput;
    type Error = ErrorData;

    fn name() -> Cow<'static, str> {
        "request_context_reset".into()
    }

    fn description() -> Option<Cow<'static, str>> {
        Some("Signal that the agent has saved its state and is ready for a context reset.".into())
    }
}

impl<S, E> AsyncTool<MeridianMcpServer<S, E>> for RequestContextResetTool
where
    S: AgentStore
        + CheckpointStore
        + MemoryStore
        + ObjectiveStore
        + McpEventLog
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
        if let Err(e) = service
            .cmd_tx
            .send(OrchestratorCommand::RequestReset { agent_id })
            .await
        {
            tracing::warn!(%agent_id, %e, "failed to send reset command");
            return Ok(RequestContextResetOutput { accepted: false });
        }
        Ok(RequestContextResetOutput { accepted: true })
    }
}

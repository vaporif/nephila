use std::borrow::Cow;

use rmcp::handler::server::router::tool::{AsyncTool, ToolBase};
use rmcp::schemars;
use rmcp::ErrorData;
use serde::{Deserialize, Serialize};

use crate::server::MeridianMcpServer;
use meridian_core::store::{AgentStore, CheckpointStore, EventStore, HitlStore, MemoryStore, ObjectiveStore};

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
        Some("Report current token usage so the lifecycle manager knows when to trigger draining.".into())
    }
}

impl<S> AsyncTool<MeridianMcpServer<S>> for ReportTokenEstimateTool
where
    S: AgentStore + CheckpointStore + MemoryStore + ObjectiveStore + EventStore + HitlStore + Send + Sync + 'static,
{
    async fn invoke(
        _service: &MeridianMcpServer<S>,
        _params: Self::Parameter,
    ) -> Result<Self::Output, Self::Error> {
        Ok(ReportTokenEstimateOutput {
            acknowledged: true,
        })
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
        Some("Check what the agent should do next: continue, prepare_reset, pause, or abort.".into())
    }
}

impl<S> AsyncTool<MeridianMcpServer<S>> for GetDirectiveTool
where
    S: AgentStore + CheckpointStore + MemoryStore + ObjectiveStore + EventStore + HitlStore + Send + Sync + 'static,
{
    async fn invoke(
        _service: &MeridianMcpServer<S>,
        _params: Self::Parameter,
    ) -> Result<Self::Output, Self::Error> {
        Ok(GetDirectiveOutput {
            directive: "continue".to_owned(),
            reason: None,
            injected_message: None,
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

impl<S> AsyncTool<MeridianMcpServer<S>> for RequestContextResetTool
where
    S: AgentStore + CheckpointStore + MemoryStore + ObjectiveStore + EventStore + HitlStore + Send + Sync + 'static,
{
    async fn invoke(
        _service: &MeridianMcpServer<S>,
        _params: Self::Parameter,
    ) -> Result<Self::Output, Self::Error> {
        Ok(RequestContextResetOutput { accepted: true })
    }
}

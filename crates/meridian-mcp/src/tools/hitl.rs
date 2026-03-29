use std::borrow::Cow;

use rmcp::handler::server::router::tool::{AsyncTool, ToolBase};
use rmcp::schemars;
use rmcp::ErrorData;
use serde::{Deserialize, Serialize};

use crate::server::MeridianMcpServer;
use meridian_core::store::{AgentStore, CheckpointStore, EventStore, HitlStore, MemoryStore, ObjectiveStore};

#[derive(Debug, Deserialize, schemars::JsonSchema, Default)]
pub struct RequestHumanInputParams {
    /// The agent ID making the request.
    pub agent_id: String,
    /// The question to present to the human operator.
    pub question: String,
    /// Optional list of predefined answer choices.
    #[serde(default)]
    pub options: Vec<String>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct RequestHumanInputOutput {
    /// The human operator's response, or a timeout message.
    pub response: String,
    /// Whether the response was received (false if timed out).
    pub received: bool,
}

pub struct RequestHumanInputTool;

impl ToolBase for RequestHumanInputTool {
    type Parameter = RequestHumanInputParams;
    type Output = RequestHumanInputOutput;
    type Error = ErrorData;

    fn name() -> Cow<'static, str> {
        "request_human_input".into()
    }

    fn description() -> Option<Cow<'static, str>> {
        Some("Ask the human operator a question and wait for their response.".into())
    }
}

impl<S> AsyncTool<MeridianMcpServer<S>> for RequestHumanInputTool
where
    S: AgentStore + CheckpointStore + MemoryStore + ObjectiveStore + EventStore + HitlStore + Send + Sync + 'static,
{
    async fn invoke(
        _service: &MeridianMcpServer<S>,
        _params: Self::Parameter,
    ) -> Result<Self::Output, Self::Error> {
        Ok(RequestHumanInputOutput {
            response: "No operator connected".to_owned(),
            received: false,
        })
    }
}

use std::borrow::Cow;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Duration;

use rmcp::ErrorData;
use rmcp::handler::server::router::tool::{AsyncTool, ToolBase};
use rmcp::schemars;
use serde::{Deserialize, Serialize};

use crate::server::{MeridianMcpServer, parse_agent_id};
use crate::state::HitlRequest;
use meridian_core::embedding::EmbeddingProvider;
use meridian_core::event::BusEvent;
use meridian_core::store::{
    AgentStore, CheckpointStore, EventStore, HitlStore, MemoryStore, ObjectiveStore,
};

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

fn hash_question(question: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    question.hash(&mut hasher);
    hasher.finish()
}

impl<S, E> AsyncTool<MeridianMcpServer<S, E>> for RequestHumanInputTool
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
        let question_hash = hash_question(&params.question);

        let ask_count = service
            .store
            .record_ask(agent_id, question_hash)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        if ask_count > 3 {
            return Err(ErrorData::invalid_request(
                "stuck loop detected: question asked more than 3 times".to_owned(),
                None,
            ));
        }

        let (tx, rx) = tokio::sync::oneshot::channel();

        {
            let mut requests = service.hitl_requests.write().await;
            if requests.contains_key(&agent_id) {
                return Err(ErrorData::invalid_request(
                    "another HITL request is already pending for this agent".to_owned(),
                    None,
                ));
            }
            requests.insert(
                agent_id,
                HitlRequest {
                    question: params.question.clone(),
                    options: params.options.clone(),
                    response_tx: tx,
                },
            );
        }

        let _ = service.event_tx.send(BusEvent::HitlRequested {
            agent_id,
            question: params.question,
            options: params.options,
        });

        let timeout = Duration::from_secs(service.config.lifecycle.hang_timeout_secs);
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(response)) => Ok(RequestHumanInputOutput {
                response,
                received: true,
            }),
            _ => {
                service.hitl_requests.write().await.remove(&agent_id);
                Ok(RequestHumanInputOutput {
                    response: "timeout".to_owned(),
                    received: false,
                })
            }
        }
    }
}

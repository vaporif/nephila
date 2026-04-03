use std::borrow::Cow;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use rmcp::ErrorData;
use rmcp::handler::server::router::tool::{AsyncTool, ToolBase};
use rmcp::schemars;
use serde::{Deserialize, Serialize};

use crate::server::{NephilaMcpServer, nephila_err, parse_agent_id};
use nephila_core::checkpoint::InterruptType;
use nephila_core::command::OrchestratorCommand;
use nephila_core::embedding::EmbeddingProvider;
use nephila_core::event::BusEvent;
use nephila_core::id::{CheckpointId, InterruptId};
use nephila_core::interrupt::{InterruptRequest, InterruptStatus};
use nephila_core::store::{
    AgentStore, CheckpointStore, InterruptStore, McpEventLog, MemoryStore, ObjectiveStore,
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
    pub message: String,
    pub suspending: bool,
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
        Some("Ask the human operator a question. The agent will be suspended and the answer arrives on the next generation via get_session_checkpoint.".into())
    }
}

fn hash_question(question: &str) -> String {
    let mut hasher = DefaultHasher::new();
    question.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

impl<S, E> AsyncTool<NephilaMcpServer<S, E>> for RequestHumanInputTool
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
        service: &NephilaMcpServer<S, E>,
        params: Self::Parameter,
    ) -> Result<Self::Output, Self::Error> {
        let agent_id = parse_agent_id(&params.agent_id)?;
        let question_hash = hash_question(&params.question);

        // Get the agent's current checkpoint to tie the interrupt to
        let checkpoint_id = AgentStore::get(service.store.as_ref(), agent_id)
            .await
            .map_err(nephila_err)?
            .and_then(|a| a.checkpoint_id)
            .unwrap_or_else(CheckpointId::new);

        let interrupt = InterruptRequest {
            id: InterruptId::new(),
            agent_id,
            checkpoint_id,
            interrupt_type: InterruptType::Hitl,
            payload: Some(serde_json::json!({
                "question": params.question,
                "options": params.options,
            })),
            status: InterruptStatus::Pending,
            response: None,
            question_hash: Some(question_hash),
            ask_count: 1,
            created_at: chrono::Utc::now(),
            resolved_at: None,
        };

        InterruptStore::save(service.store.as_ref(), &interrupt)
            .await
            .map_err(nephila_err)?;

        let _ = service.event_tx.send(BusEvent::HitlRequested {
            agent_id,
            question: params.question,
            options: params.options,
        });

        // Trigger suspension
        let _ = service
            .cmd_tx
            .send(OrchestratorCommand::Suspend { agent_id })
            .await;

        Ok(RequestHumanInputOutput {
            message: "Question recorded. Please checkpoint your state now — you will be suspended. The answer will be available when you are restored.".into(),
            suspending: true,
        })
    }
}

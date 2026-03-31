use meridian_core::id::AgentId;

use crate::anthropic_api::AnthropicApiConnector;
use crate::claude_code::ClaudeCodeConnector;
use crate::config::{RequestConfig, SpawnConfig};
use crate::error::ConnectorError;
use crate::message::MessageConnector;
use crate::openai_compatible::OpenAiCompatibleConnector;
use crate::task::{TaskConnector, TaskHandle, TaskResult, TaskStatus};
use crate::types::{Message, Response, ToolDefinition};

#[derive(Debug, Clone)]
pub enum TaskConnectorKind {
    ClaudeCode(ClaudeCodeConnector),
    AnthropicApi(AnthropicApiConnector),
}

impl TaskConnector for TaskConnectorKind {
    async fn spawn(
        &self,
        agent_id: AgentId,
        config: &SpawnConfig,
        prompt: &str,
        session_id: &str,
    ) -> Result<TaskHandle, ConnectorError> {
        match self {
            Self::ClaudeCode(c) => c.spawn(agent_id, config, prompt, session_id).await,
            Self::AnthropicApi(c) => c.spawn(agent_id, config, prompt, session_id).await,
        }
    }

    async fn status(&self, handle: &TaskHandle) -> Result<TaskStatus, ConnectorError> {
        match self {
            Self::ClaudeCode(c) => c.status(handle).await,
            Self::AnthropicApi(c) => c.status(handle).await,
        }
    }

    async fn kill(&self, handle: &TaskHandle) -> Result<(), ConnectorError> {
        match self {
            Self::ClaudeCode(c) => c.kill(handle).await,
            Self::AnthropicApi(c) => c.kill(handle).await,
        }
    }

    async fn wait(&self, handle: &TaskHandle) -> Result<TaskResult, ConnectorError> {
        match self {
            Self::ClaudeCode(c) => c.wait(handle).await,
            Self::AnthropicApi(c) => c.wait(handle).await,
        }
    }
}

#[derive(Debug, Clone)]
pub enum MessageConnectorKind {
    AnthropicApi(AnthropicApiConnector),
    OpenAiCompatible(OpenAiCompatibleConnector),
}

impl MessageConnector for MessageConnectorKind {
    async fn send(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        config: &RequestConfig,
    ) -> Result<Response, ConnectorError> {
        match self {
            Self::AnthropicApi(c) => c.send(messages, tools, config).await,
            Self::OpenAiCompatible(c) => c.send(messages, tools, config).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_connector_kind_claude_code_variant() {
        let connector = ClaudeCodeConnector::new("claude".into());
        let kind = TaskConnectorKind::ClaudeCode(connector);
        assert!(matches!(kind, TaskConnectorKind::ClaudeCode(_)));
    }

    #[tokio::test]
    async fn task_connector_kind_anthropic_stub_returns_error() {
        let connector = AnthropicApiConnector::new(
            "ANTHROPIC_API_KEY".into(),
            "claude-sonnet-4-6-20250514".into(),
        );
        let kind = TaskConnectorKind::AnthropicApi(connector);

        let spawn_config = SpawnConfig {
            directory: std::path::PathBuf::from("/tmp"),
            mcp_endpoint: "http://localhost:8080".into(),
            request_config: None,
        };
        let result = kind
            .spawn(AgentId::new(), &spawn_config, "test", "sess-1")
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn message_connector_kind_openai_stub_returns_error() {
        let connector =
            OpenAiCompatibleConnector::new("http://localhost:11434/v1".into(), "OPENAI_KEY".into());
        let kind = MessageConnectorKind::OpenAiCompatible(connector);

        let req_config = RequestConfig {
            model: "gpt-4".into(),
            max_tokens: 100,
            temperature: None,
            system: None,
        };
        let result = kind.send(&[], &[], &req_config).await;
        assert!(result.is_err());
    }
}

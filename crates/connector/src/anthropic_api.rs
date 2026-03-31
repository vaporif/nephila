use meridian_core::id::AgentId;

use crate::config::{RequestConfig, SpawnConfig};
use crate::error::ConnectorError;
use crate::message::MessageConnector;
use crate::task::{TaskConnector, TaskHandle, TaskResult, TaskStatus};
use crate::types::{Message, Response, ToolDefinition};

#[derive(Debug, Clone)]
pub struct AnthropicApiConnector {
    _api_key_env: String,
    _default_model: String,
}

impl AnthropicApiConnector {
    pub const fn new(api_key_env: String, default_model: String) -> Self {
        Self {
            _api_key_env: api_key_env,
            _default_model: default_model,
        }
    }
}

impl TaskConnector for AnthropicApiConnector {
    async fn spawn(
        &self,
        _agent_id: AgentId,
        _config: &SpawnConfig,
        _prompt: &str,
        _session_id: &str,
    ) -> Result<TaskHandle, ConnectorError> {
        Err(ConnectorError::Other(
            "AnthropicApiConnector task execution not yet implemented".into(),
        ))
    }

    async fn status(&self, _handle: &TaskHandle) -> Result<TaskStatus, ConnectorError> {
        Err(ConnectorError::Other(
            "AnthropicApiConnector status not yet implemented".into(),
        ))
    }

    async fn kill(&self, _handle: &TaskHandle) -> Result<(), ConnectorError> {
        Err(ConnectorError::Other(
            "AnthropicApiConnector kill not yet implemented".into(),
        ))
    }

    async fn wait(&self, _handle: &TaskHandle) -> Result<TaskResult, ConnectorError> {
        Err(ConnectorError::Other(
            "AnthropicApiConnector wait not yet implemented".into(),
        ))
    }
}

impl MessageConnector for AnthropicApiConnector {
    async fn send(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _config: &RequestConfig,
    ) -> Result<Response, ConnectorError> {
        Err(ConnectorError::Other(
            "AnthropicApiConnector send not yet implemented".into(),
        ))
    }
}

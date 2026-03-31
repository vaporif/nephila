use crate::config::RequestConfig;
use crate::error::ConnectorError;
use crate::message::MessageConnector;
use crate::types::{Message, Response, ToolDefinition};

#[derive(Debug, Clone)]
pub struct OpenAiCompatibleConnector {
    _base_url: String,
    _api_key_env: String,
}

impl OpenAiCompatibleConnector {
    pub const fn new(base_url: String, api_key_env: String) -> Self {
        Self {
            _base_url: base_url,
            _api_key_env: api_key_env,
        }
    }
}

impl MessageConnector for OpenAiCompatibleConnector {
    async fn send(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _config: &RequestConfig,
    ) -> Result<Response, ConnectorError> {
        Err(ConnectorError::Other(
            "OpenAiCompatibleConnector not yet implemented".into(),
        ))
    }
}

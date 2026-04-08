use crate::config::RequestConfig;
use crate::error::ConnectorError;
use crate::types::{Message, Response, ToolDefinition};

pub trait MessageConnector: Send + Sync {
    fn send(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        config: &RequestConfig,
    ) -> impl std::future::Future<Output = Result<Response, ConnectorError>> + Send;
}

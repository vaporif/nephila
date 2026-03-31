use std::path::PathBuf;

use meridian_core::id::AgentId;
use serde::{Deserialize, Serialize};

use crate::config::SpawnConfig;
use crate::error::ConnectorError;
use crate::types::Usage;

pub trait TaskConnector: Send + Sync {
    fn spawn(
        &self,
        agent_id: AgentId,
        config: &SpawnConfig,
        prompt: &str,
        session_id: &str,
    ) -> impl std::future::Future<Output = Result<TaskHandle, ConnectorError>> + Send;

    fn status(
        &self,
        handle: &TaskHandle,
    ) -> impl std::future::Future<Output = Result<TaskStatus, ConnectorError>> + Send;

    fn kill(
        &self,
        handle: &TaskHandle,
    ) -> impl std::future::Future<Output = Result<(), ConnectorError>> + Send;

    fn wait(
        &self,
        handle: &TaskHandle,
    ) -> impl std::future::Future<Output = Result<TaskResult, ConnectorError>> + Send;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TaskHandle {
    ClaudeCode {
        session_id: String,
        directory: PathBuf,
    },
    Api {
        conversation_id: String,
    },
}

#[derive(Debug, Clone)]
pub enum TaskStatus {
    Running,
    Completed(TaskResult),
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct TaskResult {
    pub output: String,
    pub usage: Option<Usage>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_handle_claude_code_serializes() {
        let handle = TaskHandle::ClaudeCode {
            session_id: "sess-123".into(),
            directory: PathBuf::from("/tmp/work"),
        };
        let json = serde_json::to_string(&handle).expect("serialize");
        let decoded: TaskHandle = serde_json::from_str(&json).expect("deserialize");
        assert!(matches!(decoded, TaskHandle::ClaudeCode { .. }));
    }

    #[test]
    fn task_handle_api_serializes() {
        let handle = TaskHandle::Api {
            conversation_id: "conv-456".into(),
        };
        let json = serde_json::to_string(&handle).expect("serialize");
        assert!(json.contains("conv-456"));
    }

    #[test]
    fn task_result_with_usage() {
        let result = TaskResult {
            output: "done".into(),
            usage: Some(Usage {
                input_tokens: 100,
                output_tokens: 50,
            }),
        };
        assert_eq!(result.output, "done");
        assert_eq!(result.usage.expect("has usage").input_tokens, 100);
    }

    #[test]
    fn task_result_without_usage() {
        let result = TaskResult {
            output: "done".into(),
            usage: None,
        };
        assert!(result.usage.is_none());
    }
}

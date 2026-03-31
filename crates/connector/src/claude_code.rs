use std::path::Path;

use meridian_core::id::AgentId;
use tokio::process::Command;

use crate::config::SpawnConfig;
use crate::error::ConnectorError;
use crate::task::{TaskConnector, TaskHandle, TaskResult, TaskStatus};

#[derive(Debug, Clone)]
pub struct ClaudeCodeConnector {
    pub claude_binary: String,
}

impl ClaudeCodeConnector {
    pub const fn new(claude_binary: String) -> Self {
        Self { claude_binary }
    }

    fn mcp_config_json(mcp_endpoint: &str) -> String {
        let config = serde_json::json!({
            "mcpServers": {
                "meridian": {
                    "type": "streamable-http",
                    "url": mcp_endpoint
                }
            }
        });
        serde_json::to_string_pretty(&config).expect("static json must serialize")
    }

    /// Opens an interactive Claude session by resuming an existing session.
    /// This takes over the terminal -- call after leaving the alternate screen.
    pub fn resume_interactive(
        &self,
        session_id: &str,
        directory: &Path,
    ) -> Result<std::process::ExitStatus, ConnectorError> {
        std::process::Command::new(&self.claude_binary)
            .arg("--resume")
            .arg(session_id)
            .arg("--dangerously-skip-permissions")
            .current_dir(directory)
            .status()
            .map_err(|e| ConnectorError::Process {
                exit_code: None,
                stderr: format!("failed to resume claude session: {e}"),
            })
    }
}

impl TaskConnector for ClaudeCodeConnector {
    async fn spawn(
        &self,
        agent_id: AgentId,
        config: &SpawnConfig,
        prompt: &str,
        session_id: &str,
    ) -> Result<TaskHandle, ConnectorError> {
        let mcp_config = Self::mcp_config_json(&config.mcp_endpoint);
        let mcp_path = config.directory.join(".mcp.json");

        tokio::fs::write(&mcp_path, &mcp_config)
            .await
            .map_err(|e| ConnectorError::Process {
                exit_code: None,
                stderr: format!("failed to write .mcp.json: {e}"),
            })?;

        let output = Command::new(&self.claude_binary)
            .arg("--dangerously-skip-permissions")
            .arg("-p")
            .arg(prompt)
            .arg("--session-id")
            .arg(session_id)
            .arg("--output-format")
            .arg("json")
            .arg("--verbose")
            .current_dir(&config.directory)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| ConnectorError::Process {
                exit_code: None,
                stderr: format!("failed to spawn claude for agent {agent_id}: {e}"),
            })?
            .wait_with_output()
            .await
            .map_err(|e| ConnectorError::Process {
                exit_code: None,
                stderr: format!("claude process failed for agent {agent_id}: {e}"),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ConnectorError::Process {
                exit_code: output.status.code(),
                stderr: stderr.chars().take(500).collect(),
            });
        }

        tracing::info!(%agent_id, session_id, "initial claude session created");

        Ok(TaskHandle::ClaudeCode {
            session_id: session_id.to_string(),
            directory: config.directory.clone(),
        })
    }

    async fn status(&self, _handle: &TaskHandle) -> Result<TaskStatus, ConnectorError> {
        Ok(TaskStatus::Running)
    }

    async fn kill(&self, _handle: &TaskHandle) -> Result<(), ConnectorError> {
        Ok(())
    }

    async fn wait(&self, _handle: &TaskHandle) -> Result<TaskResult, ConnectorError> {
        Ok(TaskResult {
            output: String::new(),
            usage: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_config_json_contains_endpoint() {
        let json = ClaudeCodeConnector::mcp_config_json("http://localhost:8080/mcp");
        assert!(json.contains("http://localhost:8080/mcp"));
        assert!(json.contains("streamable-http"));
        assert!(json.contains("meridian"));
    }

    #[test]
    fn mcp_config_json_is_valid_json() {
        let json = ClaudeCodeConnector::mcp_config_json("http://example.com/mcp");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        assert!(
            parsed["mcpServers"]["meridian"]["url"]
                .as_str()
                .expect("url string")
                .contains("example.com")
        );
    }

    #[test]
    fn connector_new_sets_binary() {
        let connector = ClaudeCodeConnector::new("/custom/claude".into());
        assert_eq!(connector.claude_binary, "/custom/claude");
    }
}

use std::path::PathBuf;

use meridian_core::id::AgentId;
use meridian_core::{MeridianError, Result};
use tokio::process::{Child, Command};

pub struct ClaudeProcess {
    pub agent_id: AgentId,
    pub child: Child,
    pub directory: PathBuf,
}

pub struct SpawnConfig {
    pub agent_id: AgentId,
    pub directory: PathBuf,
    pub claude_binary: String,
    pub system_prompt: String,
    pub mcp_endpoint: String,
}

impl ClaudeProcess {
    pub async fn spawn(config: SpawnConfig) -> Result<Self> {
        let mcp_config = serde_json::json!({
            "mcpServers": {
                "meridian": {
                    "type": "streamable-http",
                    "url": config.mcp_endpoint
                }
            }
        });

        let mcp_path = config.directory.join(".mcp.json");
        let config_str = serde_json::to_string_pretty(&mcp_config)
            .map_err(|e| MeridianError::Process(format!("failed to serialize .mcp.json: {e}")))?;
        tokio::fs::write(&mcp_path, config_str)
            .await
            .map_err(|e| MeridianError::Process(format!("failed to write .mcp.json: {e}")))?;

        let child = Command::new(&config.claude_binary)
            .arg("--dangerously-skip-permissions")
            .arg("-p")
            .arg(&config.system_prompt)
            .current_dir(&config.directory)
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                MeridianError::Process(format!(
                    "failed to spawn claude for agent {}: {e}",
                    config.agent_id
                ))
            })?;

        tracing::info!(agent_id = %config.agent_id, "spawned claude process");

        Ok(Self {
            agent_id: config.agent_id,
            child,
            directory: config.directory,
        })
    }

    pub fn id(&self) -> AgentId {
        self.agent_id
    }

    pub async fn wait(&mut self) -> Result<std::process::ExitStatus> {
        self.child
            .wait()
            .await
            .map_err(|e| MeridianError::Process(format!("wait failed for agent {}: {e}", self.agent_id)))
    }

    pub async fn kill(&mut self) -> Result<()> {
        self.child
            .kill()
            .await
            .map_err(|e| MeridianError::Process(format!("kill failed for agent {}: {e}", self.agent_id)))
    }
}

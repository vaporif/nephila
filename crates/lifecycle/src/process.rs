use std::path::{Path, PathBuf};

use meridian_core::id::AgentId;
use meridian_core::{MeridianError, Result};
use tokio::process::Command;

pub struct SpawnConfig {
    pub agent_id: AgentId,
    pub directory: PathBuf,
    pub claude_binary: String,
    pub mcp_endpoint: String,
}

/// Runs the initial prompt as a one-shot, creating a Claude session that can be resumed.
/// Returns the session_id on success.
pub async fn spawn_initial(config: &SpawnConfig, prompt: &str, session_id: &str) -> Result<()> {
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

    let output = Command::new(&config.claude_binary)
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
        .map_err(|e| {
            MeridianError::Process(format!(
                "failed to spawn claude for agent {}: {e}",
                config.agent_id
            ))
        })?
        .wait_with_output()
        .await
        .map_err(|e| {
            MeridianError::Process(format!(
                "claude process failed for agent {}: {e}",
                config.agent_id
            ))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(MeridianError::Process(format!(
            "claude exited with {}: {}",
            output.status,
            stderr.chars().take(500).collect::<String>()
        )));
    }

    tracing::info!(agent_id = %config.agent_id, session_id, "initial claude session created");
    Ok(())
}

/// Opens an interactive Claude session by resuming an existing session.
/// This takes over the terminal — call after leaving the alternate screen.
pub fn resume_interactive(
    claude_binary: &str,
    session_id: &str,
    directory: &Path,
) -> Result<std::process::ExitStatus> {
    std::process::Command::new(claude_binary)
        .arg("--resume")
        .arg(session_id)
        .arg("--dangerously-skip-permissions")
        .current_dir(directory)
        .status()
        .map_err(|e| MeridianError::Process(format!("failed to resume claude session: {e}")))
}

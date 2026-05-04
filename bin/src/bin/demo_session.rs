//! Slice-1a demo: spawns one `ClaudeCodeSession` against the fake-claude
//! binary, sends a single human turn, and prints the draft event stream
//! for 5 seconds.
//!
//! Usage:
//!   FAKE_CLAUDE_BIN=/path/to/fake_claude cargo run --bin demo_session
//!
//! Manual verification only — slice 5 deletes this file.

use std::path::PathBuf;
use std::time::Duration;

use nephila_connector::session::{ClaudeCodeSession, PromptSource, SessionConfig};
use nephila_core::id::AgentId;
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let fake = std::env::var("FAKE_CLAUDE_BIN").map_err(|_| {
        "FAKE_CLAUDE_BIN env var required (path to the fake_claude binary built by `cargo build -p nephila-connector --tests`)"
    })?;

    // Held until after `shutdown()` — `.keep()` would leak the dir.
    let workdir = tempfile::tempdir()?;
    let cfg = SessionConfig {
        claude_binary: PathBuf::from(fake),
        session_id: Uuid::new_v4(),
        agent_id: AgentId::new(),
        working_dir: workdir.path().to_path_buf(),
        mcp_endpoint: "http://stub".into(),
        permission_mode: "bypassPermissions".into(),
    };

    let session = ClaudeCodeSession::start(cfg).await?;
    let mut events = session.subscribe_drafts();

    let _ = session
        .send_turn(PromptSource::Human, "demo prompt: echo OK".into())
        .await?;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, events.recv()).await {
            Ok(Ok(ev)) => println!("{} {:?}", ev.kind(), ev),
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(n))) => {
                eprintln!("warn: demo lagged by {n} events");
            }
            Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) | Err(_) => break,
        }
    }

    session.shutdown().await?;
    drop(workdir);
    Ok(())
}

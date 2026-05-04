//! Slice-1b demo: spawns one `ClaudeCodeSession` against the fake-claude
//! binary, sends a single human turn, and prints the persisted event stream
//! for 5 seconds.
//!
//! Usage:
//!   FAKE_CLAUDE_BIN=/path/to/fake_claude cargo run --bin demo_session
//!
//! Manual verification only — slice 5 deletes this file.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use nephila_connector::session::{ClaudeCodeSession, PromptSource, SessionConfig};
use nephila_core::id::AgentId;
use nephila_core::session_event::SessionEvent;
use nephila_eventsourcing::store::DomainEventStore;
use nephila_store::SqliteStore;
use nephila_store::blob::SqliteBlobReader;
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let fake = std::env::var("FAKE_CLAUDE_BIN").map_err(|_| {
        "FAKE_CLAUDE_BIN env var required (path to the fake_claude binary built by `cargo build -p nephila-connector --tests`)"
    })?;

    // Held until after `shutdown()` — `.keep()` would leak the dir.
    let workdir = tempfile::tempdir()?;
    let session_id = Uuid::new_v4();
    let store = Arc::new(SqliteStore::open_in_memory(384)?);
    let blob_reader = Arc::new(SqliteBlobReader::new(store.read_pool()));
    let cfg = SessionConfig {
        claude_binary: PathBuf::from(fake),
        session_id,
        agent_id: AgentId::new(),
        working_dir: workdir.path().to_path_buf(),
        mcp_endpoint: "http://stub".into(),
        permission_mode: "bypassPermissions".into(),
        store: Arc::clone(&store),
        blob_reader,
    };

    let session = ClaudeCodeSession::start(cfg).await?;

    let mut stream = store
        .subscribe_after("session", &session.aggregate_id(), 0)
        .await?;

    let _ = session
        .send_turn(PromptSource::Human, "demo prompt: echo OK".into())
        .await?;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, stream.next()).await {
            Ok(Some(Ok(env))) => {
                match serde_json::from_value::<SessionEvent>(env.payload.clone()) {
                    Ok(ev) => println!("{} {:?}", ev.kind(), ev),
                    Err(e) => eprintln!("warn: decode payload: {e}"),
                }
            }
            Ok(Some(Err(e))) => eprintln!("warn: stream error: {e}"),
            Ok(None) | Err(_) => break,
        }
    }

    session.shutdown().await?;
    drop(workdir);
    Ok(())
}

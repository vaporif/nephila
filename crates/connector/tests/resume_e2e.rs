//! Integration tests for `ClaudeCodeSession::resume`.
//!
//! Two paths:
//!   1. `--resume <id>` succeeds — happy path through `wrap_with_scenario(happy)`.
//!   2. `--resume <id>` fails with "session not found" → fallback to
//!      `--session-id <id>` succeeds — `resume_not_found` scenario.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use nephila_connector::session::{ClaudeCodeSession, SessionConfig};
use nephila_core::session_event::SessionEvent;
use nephila_eventsourcing::store::DomainEventStore;
use nephila_store::SqliteStore;
use nephila_store::blob::SqliteBlobReader;
use uuid::Uuid;

fn fake_claude_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fake_claude"))
}

fn wrap_with_scenario(workdir: &std::path::Path, scenario: &str) -> PathBuf {
    let inner = fake_claude_path();
    let wrapper = workdir.join("fake_claude_wrapper.sh");
    let body = format!(
        "#!/bin/sh\nexec {} --scenario {} \"$@\"\n",
        inner.display(),
        scenario
    );
    std::fs::write(&wrapper, body).expect("write wrapper");
    let mut perm = std::fs::metadata(&wrapper).expect("meta").permissions();
    perm.set_mode(0o755);
    std::fs::set_permissions(&wrapper, perm).expect("chmod");
    wrapper
}

#[tokio::test]
async fn resume_succeeds_when_session_exists() {
    let workdir = tempfile::tempdir().expect("tempdir");
    let claude_binary = wrap_with_scenario(workdir.path(), "happy");
    let session_id = Uuid::new_v4();
    let store = Arc::new(SqliteStore::open_in_memory(384).expect("store"));
    let blob_reader = Arc::new(SqliteBlobReader::new(store.read_pool()));

    let cfg = SessionConfig {
        claude_binary,
        session_id,
        agent_id: nephila_core::id::AgentId::new(),
        working_dir: workdir.path().to_path_buf(),
        mcp_endpoint: "http://stub".into(),
        permission_mode: "bypassPermissions".into(),
        store: Arc::clone(&store),
        blob_reader,
        crash_fallback_tx: None,
    };

    let session = ClaudeCodeSession::resume(cfg, session_id)
        .await
        .expect("resume should succeed");

    let mut stream = store
        .subscribe_after("session", &session.aggregate_id(), 0)
        .await
        .expect("subscribe_after");

    let mut found_resumed = false;
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(item) = stream.next().await {
            if let Ok(env) = item
                && let Ok(SessionEvent::SessionStarted { resumed, .. }) =
                    serde_json::from_value::<SessionEvent>(env.payload.clone())
            {
                found_resumed = resumed;
                break;
            }
        }
    })
    .await;
    assert!(found_resumed, "expected SessionStarted with resumed=true");

    session.shutdown().await.expect("shutdown");
    drop(workdir);
}

#[tokio::test]
async fn resume_falls_back_when_session_not_found() {
    let workdir = tempfile::tempdir().expect("tempdir");
    // `resume_not_found`: --resume exits 1 with "Session not found"; --session-id works.
    let claude_binary = wrap_with_scenario(workdir.path(), "resume_not_found");
    let session_id = Uuid::new_v4();
    let store = Arc::new(SqliteStore::open_in_memory(384).expect("store"));
    let blob_reader = Arc::new(SqliteBlobReader::new(store.read_pool()));

    let cfg = SessionConfig {
        claude_binary,
        session_id,
        agent_id: nephila_core::id::AgentId::new(),
        working_dir: workdir.path().to_path_buf(),
        mcp_endpoint: "http://stub".into(),
        permission_mode: "bypassPermissions".into(),
        store: Arc::clone(&store),
        blob_reader,
        crash_fallback_tx: None,
    };

    let session = ClaudeCodeSession::resume(cfg, session_id)
        .await
        .expect("resume must succeed via fallback to --session-id");

    let mut stream = store
        .subscribe_after("session", &session.aggregate_id(), 0)
        .await
        .expect("subscribe_after");

    let mut saw_started = false;
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(item) = stream.next().await {
            if let Ok(env) = item
                && let Ok(SessionEvent::SessionStarted { resumed, .. }) =
                    serde_json::from_value::<SessionEvent>(env.payload.clone())
            {
                // Fallback: still emits SessionStarted; `resumed` reflects the
                // operator's intent (true), the metadata flag distinguishes
                // fallback. We assert SessionStarted is present at all.
                let _ = resumed;
                saw_started = true;
                break;
            }
        }
    })
    .await;
    assert!(
        saw_started,
        "expected SessionStarted after fallback to --session-id"
    );

    session.shutdown().await.expect("shutdown");
    drop(workdir);
}

//! Smoke test for `ClaudeCodeSession::pause` / `resume_paused`.
//!
//! Drives the `steady_drip` `fake_claude` scenario via a generated shell wrapper
//! (the production `SessionConfig` has no `extra_args`, so we wrap `fake_claude`
//! to inject `--scenario steady_drip`). The fixture emits one assistant frame
//! every 100ms after each stdin input.
//!
//! Flow:
//!   1. `send_turn` → fixture starts dripping frames
//!   2. observe at least one assistant event (proves stream alive)
//!   3. `pause()` → SIGSTOP halts the OS process
//!   4. assert no new frames arrive within ~600ms
//!   5. `resume_paused()` → SIGCONT
//!   6. observe further events flow

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use nephila_connector::session::{ClaudeCodeSession, PromptSource, SessionConfig};
use nephila_core::session_event::SessionEvent;
use nephila_eventsourcing::store::DomainEventStore;
use nephila_store::SqliteStore;
use nephila_store::blob::SqliteBlobReader;
use uuid::Uuid;

fn fake_claude_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fake_claude"))
}

/// Writes a shell wrapper that exec's `fake_claude` with `--scenario` prepended.
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
async fn pause_halts_event_stream_then_resume_continues() {
    let workdir = tempfile::tempdir().expect("tempdir");
    let claude_binary = wrap_with_scenario(workdir.path(), "steady_drip");
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
    };

    let session = ClaudeCodeSession::start(cfg).await.expect("start");

    // Subscribe before send so we capture every emitted event.
    let mut stream = store
        .subscribe_after("session", &session.aggregate_id(), 0)
        .await
        .expect("subscribe_after");

    let _turn_id = session
        .send_turn(PromptSource::Human, "drip".into())
        .await
        .expect("send_turn");

    // Wait for at least one assistant event to confirm the drip started.
    let mut events_before_pause: u32 = 0;
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(item) = stream.next().await {
            if let Ok(env) = item
                && let Ok(ev) = serde_json::from_value::<SessionEvent>(env.payload.clone())
                && matches!(ev, SessionEvent::AssistantMessage { .. })
            {
                events_before_pause += 1;
                if events_before_pause >= 1 {
                    break;
                }
            }
        }
    })
    .await;
    assert!(
        events_before_pause >= 1,
        "expected at least one event before pause"
    );

    let pid_before = session.pid().await.expect("pid present pre-pause");
    assert!(pid_before > 0, "pid is positive: {pid_before}");

    session.pause().await.expect("pause");

    // No new frames should arrive while paused. Wait a window > one drip
    // interval (~100ms) by a comfortable margin.
    let mut events_during_pause: u32 = 0;
    let _ = tokio::time::timeout(Duration::from_millis(600), async {
        while let Some(item) = stream.next().await {
            if let Ok(env) = item
                && let Ok(ev) = serde_json::from_value::<SessionEvent>(env.payload.clone())
                && matches!(ev, SessionEvent::AssistantMessage { .. })
            {
                events_during_pause += 1;
            }
        }
    })
    .await;
    assert_eq!(
        events_during_pause, 0,
        "events leaked through SIGSTOP: {events_during_pause}"
    );

    session.resume_paused().await.expect("resume");

    // After resume, events should flow again.
    let mut events_after_resume: u32 = 0;
    let _ = tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(item) = stream.next().await {
            if let Ok(env) = item
                && let Ok(ev) = serde_json::from_value::<SessionEvent>(env.payload.clone())
                && matches!(ev, SessionEvent::AssistantMessage { .. })
            {
                events_after_resume += 1;
                if events_after_resume >= 1 {
                    break;
                }
            }
        }
    })
    .await;
    assert!(
        events_after_resume >= 1,
        "no assistant event after SIGCONT (drip stopped)"
    );

    session.shutdown().await.expect("shutdown");
    drop(workdir);
}

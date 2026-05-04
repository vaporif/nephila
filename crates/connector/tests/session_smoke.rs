use std::path::PathBuf;
use std::time::Duration;

use nephila_connector::event_draft::SessionEventDraft;
use nephila_connector::session::{ClaudeCodeSession, PromptSource, SessionConfig};
use tokio::sync::broadcast::error::RecvError;
use uuid::Uuid;

fn fake_claude_path() -> PathBuf {
    // Set by cargo for any `[[bin]]` in the same package being tested.
    PathBuf::from(env!("CARGO_BIN_EXE_fake_claude"))
}

#[tokio::test]
async fn happy_turn_emits_assistant_then_result() {
    // Held until after `shutdown()` — `.keep()` would leak the dir.
    let workdir = tempfile::tempdir().expect("tempdir");
    let cfg = SessionConfig {
        claude_binary: fake_claude_path(),
        session_id: Uuid::new_v4(),
        agent_id: nephila_core::id::AgentId::new(),
        working_dir: workdir.path().to_path_buf(),
        mcp_endpoint: "http://stub".into(),
        permission_mode: "bypassPermissions".into(),
    };

    let session = ClaudeCodeSession::start(cfg).await.expect("start");
    let mut events = session.subscribe_drafts();

    let _turn_id = session
        .send_turn(PromptSource::Human, "echo OK".into())
        .await
        .expect("send_turn");

    let mut seen = Vec::new();
    while let Ok(maybe_ev) = tokio::time::timeout(Duration::from_secs(5), events.recv()).await {
        match maybe_ev {
            Ok(ev) => {
                let is_done = matches!(ev, SessionEventDraft::TurnCompleted { .. });
                seen.push(ev);
                if is_done {
                    break;
                }
            }
            Err(RecvError::Lagged(n)) => {
                eprintln!("warn: smoke test lagged by {n} events");
            }
            Err(RecvError::Closed) => break,
        }
    }

    session.shutdown().await.expect("shutdown");
    drop(workdir);

    let kinds: Vec<&'static str> = seen.iter().map(SessionEventDraft::kind).collect();
    assert!(kinds.contains(&"HumanPromptQueued"), "kinds = {kinds:?}");
    assert!(kinds.contains(&"HumanPromptDelivered"), "kinds = {kinds:?}");
    assert!(kinds.contains(&"AssistantMessage"), "kinds = {kinds:?}");
    assert!(kinds.contains(&"TurnCompleted"), "kinds = {kinds:?}");
    assert_eq!(
        seen.iter()
            .filter(|e| matches!(e, SessionEventDraft::TurnCompleted { .. }))
            .count(),
        1,
        "exactly one TurnCompleted; kinds = {kinds:?}",
    );
}

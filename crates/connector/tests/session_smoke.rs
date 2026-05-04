use std::path::PathBuf;
use std::time::Duration;

use nephila_connector::event_draft::SessionEventDraft;
use nephila_connector::session::{ClaudeCodeSession, PromptSource, SessionConfig};
use uuid::Uuid;

fn fake_claude_path() -> PathBuf {
    // Cargo guarantees this env var for `[[bin]]` entries when running tests for the same package.
    PathBuf::from(env!("CARGO_BIN_EXE_fake_claude"))
}

#[tokio::test]
async fn happy_turn_emits_assistant_then_result() {
    let cfg = SessionConfig {
        claude_binary: fake_claude_path(),
        session_id: Uuid::new_v4(),
        agent_id: nephila_core::id::AgentId::new(),
        working_dir: tempfile::tempdir().unwrap().keep(),
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
            Err(_) => break,
        }
    }

    session.shutdown().await.expect("shutdown");

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

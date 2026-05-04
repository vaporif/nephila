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
    // Set by cargo for any `[[bin]]` in the same package being tested.
    PathBuf::from(env!("CARGO_BIN_EXE_fake_claude"))
}

#[tokio::test]
async fn happy_turn_emits_assistant_then_result() {
    // Held until after `shutdown()` — `.keep()` would leak the dir.
    let workdir = tempfile::tempdir().expect("tempdir");
    let session_id = Uuid::new_v4();
    let store = Arc::new(SqliteStore::open_in_memory(384).expect("store"));
    let blob_reader = Arc::new(SqliteBlobReader::new(store.read_pool()));

    let cfg = SessionConfig {
        claude_binary: fake_claude_path(),
        session_id,
        agent_id: nephila_core::id::AgentId::new(),
        working_dir: workdir.path().to_path_buf(),
        mcp_endpoint: "http://stub".into(),
        permission_mode: "bypassPermissions".into(),
        store: Arc::clone(&store),
        blob_reader,
    };

    let session = ClaudeCodeSession::start(cfg).await.expect("start");

    // Subscribe before sending the turn so backfill picks up the SessionStarted
    // envelope plus everything appended by the writer/reader tasks.
    let mut stream = store
        .subscribe_after("session", &session.aggregate_id(), 0)
        .await
        .expect("subscribe_after");

    let _turn_id = session
        .send_turn(PromptSource::Human, "echo OK".into())
        .await
        .expect("send_turn");

    let mut seen: Vec<SessionEvent> = Vec::new();
    loop {
        match tokio::time::timeout(Duration::from_secs(5), stream.next()).await {
            Ok(Some(Ok(env))) => {
                let ev: SessionEvent = serde_json::from_value(env.payload.clone())
                    .expect("decode SessionEvent payload");
                let is_done = matches!(ev, SessionEvent::TurnCompleted { .. });
                seen.push(ev);
                if is_done {
                    break;
                }
            }
            Ok(Some(Err(e))) => {
                eprintln!("warn: smoke test stream error: {e}");
            }
            Ok(None) | Err(_) => break,
        }
    }

    session.shutdown().await.expect("shutdown");
    drop(workdir);

    let kinds: Vec<&'static str> = seen.iter().map(SessionEvent::kind).collect();
    assert!(kinds.contains(&"human_prompt_queued"), "kinds = {kinds:?}");
    assert!(
        kinds.contains(&"human_prompt_delivered"),
        "kinds = {kinds:?}"
    );
    assert!(kinds.contains(&"assistant_message"), "kinds = {kinds:?}");
    assert!(kinds.contains(&"turn_completed"), "kinds = {kinds:?}");
    assert_eq!(
        seen.iter()
            .filter(|e| matches!(e, SessionEvent::TurnCompleted { .. }))
            .count(),
        1,
        "exactly one TurnCompleted; kinds = {kinds:?}",
    );
}

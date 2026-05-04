//! Slice 2 e2e: typing in the SessionPane drives `send_turn` and the resulting
//! events flow back through `subscribe_after`. Asserts ordering of the first
//! four events: `[HumanPromptQueued, HumanPromptDelivered,
//! AssistantMessage{is_final:true}, TurnCompleted]`.
//!
//! Per Task 4 spec the test does NOT assert per-event timing — only the
//! overall 5s budget guards against hangs.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use futures::StreamExt;
use nephila_connector::session::{ClaudeCodeSession, SessionConfig};
use nephila_core::session_event::SessionEvent;
use nephila_eventsourcing::store::DomainEventStore;
use nephila_store::SqliteStore;
use nephila_store::blob::SqliteBlobReader;
use nephila_tui::panels::session_pane::input::{InputAction, InputState, PaneMode};
use nephila_tui::panels::session_pane::pump::{PumpChannels, spawn_pump};
use nephila_tui::panels::session_pane::{SessionPane, snapshot_text};
use uuid::Uuid;

/// Locate the `fake_claude` test binary. The TUI test crate doesn't get
/// `CARGO_BIN_EXE_fake_claude` (that's only set for the package owning the
/// `[[bin]]`), so we walk up from the running test executable to the workspace
/// `target/<profile>/` directory.
fn fake_claude_path() -> PathBuf {
    let exe = std::env::current_exe().expect("current_exe");
    // Layout: target/<profile>/deps/<test-name>-<hash>
    let profile_dir = exe
        .parent()
        .and_then(|deps| deps.parent())
        .expect("profile dir");
    let path = profile_dir.join("fake_claude");
    assert!(
        path.exists(),
        "fake_claude not found at {} — run `cargo build -p nephila-connector --bin fake_claude` first",
        path.display()
    );
    path
}

#[tokio::test]
async fn typing_in_pane_emits_human_prompt_events() {
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
        crash_fallback_tx: None,
    };

    let agent_id = nephila_core::id::AgentId::new();
    let session = Arc::new(ClaudeCodeSession::start(cfg).await.expect("start"));

    // Bind the pane to the live session and start a pump task feeding the
    // pane's buffer from `subscribe_after`. Slice 4 will move this into a
    // `SessionRegistry::on_session_started` hook; here we wire it manually.
    let mut pane = SessionPane::new();
    pane.bind_session(Arc::clone(&session));
    let pump = spawn_pump(
        Arc::clone(&store),
        agent_id,
        session.aggregate_id(),
        pane.buffer(),
        PumpChannels::default(),
    );

    // Drive the input/normal mode FSM: focus pane → enter input mode (i) →
    // type "hello" → submit (Ctrl+Enter). The submit returns the text via
    // `InputAction::Submit`, and the pane spawns a `send_turn` task in the
    // background so the TUI tick loop is not blocked.
    let mut input = InputState::new();
    assert!(matches!(input.mode(), PaneMode::Normal));
    let action = input.handle_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
    assert!(matches!(action, InputAction::None));
    assert!(matches!(input.mode(), PaneMode::Input));

    for ch in "hello".chars() {
        let _ = input.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    let action = input.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL));
    let text = match action {
        InputAction::Submit(t) => t,
        other => panic!("expected Submit, got {other:?}"),
    };
    assert_eq!(text, "hello");
    pane.submit_text(text);

    let mut stream = store
        .subscribe_after("session", &session.aggregate_id(), 0)
        .await
        .expect("subscribe_after");

    let result = tokio::time::timeout(Duration::from_secs(5), async {
        let mut events: Vec<SessionEvent> = Vec::new();
        // Skip non-asserted events (SessionStarted, AssistantMessage partials)
        // until we collect the four checkpoint events.
        let needed: [fn(&SessionEvent) -> bool; 4] = [
            |e| matches!(e, SessionEvent::HumanPromptQueued { .. }),
            |e| matches!(e, SessionEvent::HumanPromptDelivered { .. }),
            |e| matches!(e, SessionEvent::AssistantMessage { is_final: true, .. }),
            |e| matches!(e, SessionEvent::TurnCompleted { .. }),
        ];
        let mut idx = 0;
        while idx < needed.len() {
            let env = match stream.next().await {
                Some(Ok(env)) => env,
                Some(Err(e)) => panic!("stream error: {e}"),
                None => panic!("stream ended early at idx={idx}"),
            };
            let ev: SessionEvent =
                serde_json::from_value(env.payload.clone()).expect("decode SessionEvent payload");
            if needed[idx](&ev) {
                events.push(ev);
                idx += 1;
            }
        }
        events
    })
    .await;

    let events = result.expect("events did not arrive within 5s");
    assert!(matches!(events[0], SessionEvent::HumanPromptQueued { .. }));
    assert!(matches!(
        events[1],
        SessionEvent::HumanPromptDelivered { .. }
    ));
    assert!(matches!(
        events[2],
        SessionEvent::AssistantMessage { is_final: true, .. }
    ));
    assert!(matches!(events[3], SessionEvent::TurnCompleted { .. }));

    // The pump task is racing the assertion stream above, but both ultimately
    // converge on the same backfill+live source. Give the pump up to 2s to
    // catch up before checking the rendered buffer.
    let pane_text = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let text = snapshot_text(&pane.buffer);
            if text.contains("YOU →") && text.contains("hello") {
                return text;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("pane did not render YOU → hello within 2s");
    assert!(
        pane_text.contains("YOU →") && pane_text.contains("hello"),
        "pane text missing prompt; got: {pane_text}",
    );

    // Drop the pane's session handle before shutdown so we own the only Arc.
    pane.unbind_session();
    pump.abort();
    let session = Arc::try_unwrap(session).unwrap_or_else(|_| {
        panic!("pane still holds an Arc<ClaudeCodeSession>; unbind_session() should release it")
    });
    session.shutdown().await.expect("shutdown");
    drop(workdir);
}

//! Pump → modal HITL flow:
//!   1. `pump_forwards_hitl_request_on_checkpoint` confirms the pump task
//!      forwards a `HitlRequest` over the mpsc on `CheckpointReached(Hitl)`.
//!   2. `pane_renders_awaiting_marker_not_question_text` covers plan Step 8's
//!      explicit requirement that the pane shows the `"↳ awaiting human
//!      input"` marker but does NOT render the question inline (that lives in
//!      the modal).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use nephila_core::id::{AgentId, CheckpointId};
use nephila_core::session_event::{InterruptSnapshot, SessionEvent};
use nephila_eventsourcing::envelope::EventEnvelope;
use nephila_eventsourcing::id::{EventId, TraceId};
use nephila_eventsourcing::store::DomainEventStore;
use nephila_store::SqliteStore;
use nephila_tui::panels::session_pane::pump::{PumpChannels, spawn_pump};
use nephila_tui::panels::session_pane::{SessionPane, snapshot_text};
use tokio::sync::mpsc;
use uuid::Uuid;

#[tokio::test]
async fn pump_forwards_hitl_request_on_checkpoint() {
    let store = Arc::new(SqliteStore::open_in_memory(384).expect("store"));
    let session_id = Uuid::new_v4();
    let agent_id = AgentId::new();

    // Bootstrap a SessionStarted so the aggregate exists.
    let started = SessionEvent::SessionStarted {
        session_id,
        agent_id,
        model: None,
        working_dir: std::path::PathBuf::from("/tmp"),
        mcp_endpoint: "x".into(),
        resumed: false,
        ts: Utc::now(),
    };
    store
        .append_batch(vec![envelope(session_id, &started)])
        .await
        .expect("append start");

    let pane = SessionPane::new();
    let (hitl_tx, mut hitl_rx) = mpsc::channel(8);

    let pump = spawn_pump(
        Arc::clone(&store),
        agent_id,
        session_id.to_string(),
        pane.buffer(),
        PumpChannels {
            activity_tx: None,
            hitl_tx: Some(hitl_tx),
        },
    );

    let cp = SessionEvent::CheckpointReached {
        checkpoint_id: CheckpointId(Uuid::new_v4()),
        interrupt: Some(InterruptSnapshot::Hitl {
            question: "Continue?".into(),
            options: vec!["yes".into(), "no".into()],
        }),
        ts: Utc::now(),
    };
    store
        .append_batch(vec![envelope(session_id, &cp)])
        .await
        .expect("append cp");

    let req = tokio::time::timeout(Duration::from_secs(2), hitl_rx.recv())
        .await
        .expect("hitl request did not arrive within 2s")
        .expect("hitl_tx closed");
    assert_eq!(req.agent_id, agent_id);
    assert_eq!(req.question, "Continue?");
    assert_eq!(req.options, vec!["yes".to_string(), "no".to_string()]);

    pump.abort();
}

#[tokio::test]
async fn pane_renders_awaiting_marker_not_question_text() {
    let store = Arc::new(SqliteStore::open_in_memory(384).expect("store"));
    let session_id = Uuid::new_v4();
    let agent_id = AgentId::new();

    let started = SessionEvent::SessionStarted {
        session_id,
        agent_id,
        model: None,
        working_dir: std::path::PathBuf::from("/tmp"),
        mcp_endpoint: "x".into(),
        resumed: false,
        ts: Utc::now(),
    };
    store
        .append_batch(vec![envelope(session_id, &started)])
        .await
        .expect("append start");

    let pane = SessionPane::new();
    let buffer = pane.buffer();
    let (hitl_tx, mut hitl_rx) = mpsc::channel(8);

    let pump = spawn_pump(
        Arc::clone(&store),
        agent_id,
        session_id.to_string(),
        buffer.clone(),
        PumpChannels {
            activity_tx: None,
            hitl_tx: Some(hitl_tx),
        },
    );

    let question_text = "Approve destructive write to /etc/passwd?";
    let cp = SessionEvent::CheckpointReached {
        checkpoint_id: CheckpointId(Uuid::new_v4()),
        interrupt: Some(InterruptSnapshot::Hitl {
            question: question_text.into(),
            options: vec!["approve".into(), "deny".into()],
        }),
        ts: Utc::now(),
    };
    store
        .append_batch(vec![envelope(session_id, &cp)])
        .await
        .expect("append cp");

    // (b) Modal channel receives the full question + options.
    let req = tokio::time::timeout(Duration::from_secs(2), hitl_rx.recv())
        .await
        .expect("hitl request did not arrive within 2s")
        .expect("hitl_tx closed");
    assert_eq!(req.question, question_text);
    assert_eq!(req.options, vec!["approve".to_string(), "deny".to_string()]);

    // (a) Pane shows the marker but NOT the question text. Poll the buffer
    // until the pump appends the marker row (the marker is appended after
    // the hitl_tx send, so we may briefly observe an empty buffer).
    let pane_text = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let text = snapshot_text(&buffer);
            if text.contains("↳ awaiting") {
                return text;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("pane did not render awaiting marker within 2s");

    assert!(
        pane_text.contains("↳ awaiting") && pane_text.contains("human input"),
        "pane text missing awaiting marker; got: {pane_text}",
    );
    assert!(
        !pane_text.contains(question_text),
        "pane must not render question inline (modal owns it); got: {pane_text}",
    );
    assert!(
        !pane_text.contains("approve") && !pane_text.contains("deny"),
        "pane must not render options inline (modal owns them); got: {pane_text}",
    );

    pump.abort();
}

fn envelope(session_id: Uuid, ev: &SessionEvent) -> EventEnvelope {
    EventEnvelope::new(nephila_eventsourcing::envelope::NewEventEnvelope {
        id: EventId::new(),
        aggregate_type: "session".to_owned(),
        aggregate_id: session_id.to_string(),
        event_type: ev.kind().to_owned(),
        payload: serde_json::to_value(ev).expect("serialize SessionEvent"),
        trace_id: TraceId(session_id.to_string()),
        outcome: None,
        timestamp: Utc::now(),
        context_snapshot: None,
        metadata: HashMap::new(),
    })
}

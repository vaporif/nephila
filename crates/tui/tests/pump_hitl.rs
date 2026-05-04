//! Unit-ish test: drive a `CheckpointReached(Hitl{...})` envelope into the
//! store and assert the pump task forwards a `HitlRequest` to the App's
//! mpsc channel.

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
use nephila_tui::panels::session_pane::SessionPane;
use nephila_tui::panels::session_pane::pump::{PumpChannels, spawn_pump};
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

    // Now append a CheckpointReached(Hitl) — pump should receive it.
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

fn envelope(session_id: Uuid, ev: &SessionEvent) -> EventEnvelope {
    EventEnvelope {
        id: EventId::new(),
        aggregate_type: "session".to_owned(),
        aggregate_id: session_id.to_string(),
        sequence: 0,
        event_type: ev.kind().to_owned(),
        payload: serde_json::to_value(ev).expect("serialize SessionEvent"),
        trace_id: TraceId(session_id.to_string()),
        outcome: None,
        timestamp: Utc::now(),
        context_snapshot: None,
        metadata: HashMap::new(),
    }
}

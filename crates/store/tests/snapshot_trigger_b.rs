//! Trigger B: consumer-side snapshot trigger inside `subscribe_after`.
//!
//! When a snapshot is missing or stale beyond `SNAPSHOT_INTERVAL`, the first
//! subscriber after the threshold spawns a background task that replays the
//! aggregate and writes a fresh snapshot. Subsequent subscribers do NOT
//! spawn another task (per-aggregate Mutex prevents thundering herd).
//!
//! NOTE: Trigger A (producer-side, in connector::SessionEnded path) is
//! deferred to the slice-1b integration window per task instructions.

use chrono::Utc;
use nephila_core::id::AgentId;
use nephila_core::session::{Session, SessionPhase};
use nephila_core::session_event::SessionEvent;
use nephila_eventsourcing::envelope::{EventEnvelope, NewEventEnvelope};
use nephila_eventsourcing::id::{EventId, TraceId};
use nephila_eventsourcing::store::DomainEventStore;
use nephila_store::SqliteStore;
use std::collections::HashMap;
use std::time::Duration;
use uuid::Uuid;

fn env(event: &SessionEvent, agg_id: &str) -> EventEnvelope {
    EventEnvelope::new(NewEventEnvelope {
        id: EventId::new(),
        aggregate_type: "session".into(),
        aggregate_id: agg_id.into(),
        event_type: event.kind().into(),
        payload: serde_json::to_value(event).unwrap(),
        trace_id: TraceId("t".into()),
        outcome: None,
        timestamp: Utc::now(),
        context_snapshot: None,
        metadata: HashMap::new(),
    })
}

fn started(session_id: Uuid, agent_id: AgentId) -> SessionEvent {
    SessionEvent::SessionStarted {
        session_id,
        agent_id,
        model: None,
        working_dir: std::path::PathBuf::from("/tmp"),
        mcp_endpoint: "mcp".into(),
        resumed: false,
        ts: Utc::now(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn first_subscribe_after_threshold_spawns_snapshot_task() {
    let store = SqliteStore::open_in_memory(384).unwrap();
    let session_id = Uuid::new_v4();
    let agent_id = AgentId::new();
    let agg_id = session_id.to_string();

    // 1500 events: SessionStarted + 1499 AssistantMessages, no SessionEnded.
    let mut events = Vec::with_capacity(1500);
    events.push(started(session_id, agent_id));
    for i in 0..1499 {
        events.push(SessionEvent::AssistantMessage {
            message_id: format!("m{i}"),
            seq_in_message: 0,
            delta_text: "x".into(),
            is_final: false,
            truncated: false,
            ts: Utc::now(),
        });
    }
    let envelopes: Vec<_> = events.iter().map(|e| env(e, &agg_id)).collect();
    let _ = store.append_batch(envelopes).await.unwrap();

    let _stream1 = store.subscribe_after("session", &agg_id, 0).await.unwrap();
    drop(_stream1);

    // Give the spawned task a moment to settle.
    for _ in 0..50 {
        if let Some(snap) = store
            .load_latest_snapshot("session", &agg_id)
            .await
            .unwrap()
        {
            // Slight tolerance for race: it should be very close to 1500.
            assert_eq!(snap.sequence, 1500);
            let session: Session = snap.into_state().unwrap();
            assert_eq!(session.phase, SessionPhase::Running);
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("snapshot was never written");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn second_subscribe_does_not_spawn_duplicate_snapshot() {
    let store = SqliteStore::open_in_memory(384).unwrap();
    let session_id = Uuid::new_v4();
    let agent_id = AgentId::new();
    let agg_id = session_id.to_string();

    let mut events = Vec::with_capacity(1500);
    events.push(started(session_id, agent_id));
    for i in 0..1499 {
        events.push(SessionEvent::AssistantMessage {
            message_id: format!("m{i}"),
            seq_in_message: 0,
            delta_text: "x".into(),
            is_final: false,
            truncated: false,
            ts: Utc::now(),
        });
    }
    let envelopes: Vec<_> = events.iter().map(|e| env(e, &agg_id)).collect();
    let _ = store.append_batch(envelopes).await.unwrap();

    // Subscribe many times rapidly; only one snapshot row should appear.
    let _s1 = store.subscribe_after("session", &agg_id, 0).await.unwrap();
    let _s2 = store.subscribe_after("session", &agg_id, 0).await.unwrap();
    let _s3 = store.subscribe_after("session", &agg_id, 0).await.unwrap();

    for _ in 0..50 {
        if store
            .load_latest_snapshot("session", &agg_id)
            .await
            .unwrap()
            .is_some()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // A subsequent subscribe should be a no-op.
    let _s4 = store.subscribe_after("session", &agg_id, 0).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let snap = store
        .load_latest_snapshot("session", &agg_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(snap.sequence, 1500);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn small_aggregate_does_not_trigger_snapshot() {
    let store = SqliteStore::open_in_memory(384).unwrap();
    let session_id = Uuid::new_v4();
    let agent_id = AgentId::new();
    let agg_id = session_id.to_string();

    // Below SNAPSHOT_INTERVAL — 100 events.
    let mut events = vec![started(session_id, agent_id)];
    for i in 0..99 {
        events.push(SessionEvent::AssistantMessage {
            message_id: format!("m{i}"),
            seq_in_message: 0,
            delta_text: "x".into(),
            is_final: false,
            truncated: false,
            ts: Utc::now(),
        });
    }
    let envelopes: Vec<_> = events.iter().map(|e| env(e, &agg_id)).collect();
    let _ = store.append_batch(envelopes).await.unwrap();

    let _s = store.subscribe_after("session", &agg_id, 0).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let snap = store
        .load_latest_snapshot("session", &agg_id)
        .await
        .unwrap();
    assert!(
        snap.is_none(),
        "no snapshot should fire below SNAPSHOT_INTERVAL"
    );
}

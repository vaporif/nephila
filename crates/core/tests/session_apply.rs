//! Reducer invariants for the `Session` aggregate.
//!
//! Located in `crates/core/tests/` (rather than `crates/eventsourcing/tests/`) because
//! `Session` lives in `crates/core` and `core` already depends on `eventsourcing` —
//! the reverse path would create a dependency cycle.

use chrono::{TimeZone, Utc};
use nephila_core::id::{AgentId, CheckpointId};
use nephila_core::session::{Session, SessionPhase};
use nephila_core::session_event::{InterruptSnapshot, SessionEvent};
use nephila_eventsourcing::aggregate::EventSourced;
use nephila_eventsourcing::envelope::EventEnvelope;
use nephila_eventsourcing::id::{EventId, TraceId};
use std::collections::HashMap;
use uuid::Uuid;

fn ts() -> chrono::DateTime<chrono::Utc> {
    Utc.with_ymd_and_hms(2026, 5, 4, 12, 0, 0).unwrap()
}

fn envelope_for(event: &SessionEvent, sequence: u64, aggregate_id: &str) -> EventEnvelope {
    EventEnvelope {
        id: EventId::new(),
        aggregate_type: "session".into(),
        aggregate_id: aggregate_id.into(),
        sequence,
        event_type: event.kind().into(),
        payload: serde_json::to_value(event).unwrap(),
        trace_id: TraceId("trace-test".into()),
        outcome: None,
        timestamp: ts(),
        context_snapshot: None,
        metadata: HashMap::new(),
    }
}

fn started(session_id: Uuid, agent_id: AgentId) -> SessionEvent {
    SessionEvent::SessionStarted {
        session_id,
        agent_id,
        model: None,
        working_dir: std::path::PathBuf::from("/tmp"),
        mcp_endpoint: "mcp".into(),
        resumed: false,
        ts: ts(),
    }
}

#[test]
fn case_1_session_started_runs_and_sets_ids() {
    let s = Session::default_state();
    let session_id = Uuid::new_v4();
    let agent_id = AgentId::new();
    let s = s.apply(&started(session_id, agent_id));
    assert_eq!(s.id, session_id);
    assert_eq!(s.agent_id, agent_id);
    assert_eq!(s.phase, SessionPhase::Running);
    assert_eq!(s.last_seq, 0);
}

#[test]
fn case_2_prompt_queued_sets_open_turn() {
    let s = Session::default_state().apply(&started(Uuid::new_v4(), AgentId::new()));
    let turn = Uuid::new_v4();
    let s = s.apply(&SessionEvent::AgentPromptQueued {
        turn_id: turn,
        text: "hi".into(),
        ts: ts(),
    });
    assert_eq!(s.open_turn, Some(turn));
}

#[test]
fn case_3_turn_completed_clears_open_turn_and_handles_interrupt_stop() {
    let mut s = Session::default_state().apply(&started(Uuid::new_v4(), AgentId::new()));
    let turn = Uuid::new_v4();
    s = s.apply(&SessionEvent::AgentPromptQueued {
        turn_id: turn,
        text: "hi".into(),
        ts: ts(),
    });
    let s_done = s.clone().apply(&SessionEvent::TurnCompleted {
        turn_id: turn,
        stop_reason: "end_turn".into(),
        ts: ts(),
    });
    assert!(s_done.open_turn.is_none());
    assert_eq!(s_done.phase, SessionPhase::Running);

    let s_interrupt = s.apply(&SessionEvent::TurnCompleted {
        turn_id: turn,
        stop_reason: "interrupt".into(),
        ts: ts(),
    });
    assert!(s_interrupt.open_turn.is_none());
    assert_eq!(s_interrupt.phase, SessionPhase::Paused);
}

#[test]
fn case_4_checkpoint_with_hitl_transitions_to_waiting_hitl() {
    let s = Session::default_state().apply(&started(Uuid::new_v4(), AgentId::new()));
    let cp = CheckpointId::new();
    let s = s.apply(&SessionEvent::CheckpointReached {
        checkpoint_id: cp,
        interrupt: Some(InterruptSnapshot::Hitl {
            question: "?".into(),
            options: vec!["yes".into()],
        }),
        ts: ts(),
    });
    assert_eq!(s.phase, SessionPhase::WaitingHitl);
    let last = s.last_checkpoint.expect("checkpoint recorded");
    assert_eq!(last.checkpoint_id, cp);
}

#[test]
fn case_5_session_crashed_clears_open_turn_and_sets_phase() {
    let mut s = Session::default_state().apply(&started(Uuid::new_v4(), AgentId::new()));
    s = s.apply(&SessionEvent::AgentPromptQueued {
        turn_id: Uuid::new_v4(),
        text: "hi".into(),
        ts: ts(),
    });
    let s = s.apply(&SessionEvent::SessionCrashed {
        reason: "oom".into(),
        exit_code: Some(137),
        ts: ts(),
    });
    assert!(s.open_turn.is_none());
    assert_eq!(s.phase, SessionPhase::Crashed);
}

#[test]
fn case_6_after_session_ended_apply_is_noop() {
    let s = Session::default_state()
        .apply(&started(Uuid::new_v4(), AgentId::new()))
        .apply(&SessionEvent::SessionEnded { ts: ts() });
    assert_eq!(s.phase, SessionPhase::Ended);
    let s2 = s.clone().apply(&SessionEvent::AgentPromptQueued {
        turn_id: Uuid::new_v4(),
        text: "ignored".into(),
        ts: ts(),
    });
    assert_eq!(s2.phase, SessionPhase::Ended);
    assert_eq!(s2.open_turn, None);
}

#[test]
fn case_7_apply_envelope_updates_last_seq() {
    let mut s = Session::default_state();
    let session_id = Uuid::new_v4();
    let agent_id = AgentId::new();
    let agg_id = session_id.to_string();
    let events = [
        started(session_id, agent_id),
        SessionEvent::AgentPromptQueued {
            turn_id: Uuid::new_v4(),
            text: "a".into(),
            ts: ts(),
        },
        SessionEvent::AgentPromptDelivered {
            turn_id: Uuid::new_v4(),
            ts: ts(),
        },
    ];
    for (i, ev) in events.iter().enumerate() {
        let env = envelope_for(ev, (i + 1) as u64, &agg_id);
        s = s.apply_envelope(&env).expect("apply_envelope");
    }
    assert_eq!(s.last_seq, events.len() as u64);
}

#[test]
fn checkpoint_at_sequence_tracks_last_seq() {
    let session_id = Uuid::new_v4();
    let agent_id = AgentId::new();
    let agg_id = session_id.to_string();
    let mut s = Session::default_state();
    s = s
        .apply_envelope(&envelope_for(&started(session_id, agent_id), 1, &agg_id))
        .unwrap();
    s = s
        .apply_envelope(&envelope_for(
            &SessionEvent::AssistantMessage {
                message_id: "m".into(),
                seq_in_message: 0,
                delta_text: "x".into(),
                is_final: false,
                truncated: false,
                ts: ts(),
            },
            2,
            &agg_id,
        ))
        .unwrap();
    let cp = CheckpointId::new();
    s = s
        .apply_envelope(&envelope_for(
            &SessionEvent::CheckpointReached {
                checkpoint_id: cp,
                interrupt: None,
                ts: ts(),
            },
            3,
            &agg_id,
        ))
        .unwrap();
    let last = s.last_checkpoint.expect("recorded");
    // CheckpointRef::at_sequence is set BEFORE last_seq is bumped (in apply()),
    // so it equals the prior `last_seq` (= 2 here, before this envelope landed).
    assert_eq!(last.at_sequence, 2);
    assert_eq!(s.last_seq, 3);
}

//! Property test: replaying from any mid-point "snapshot" must produce the
//! same final state as replaying from `default_state`. Pins the replay
//! determinism invariant central to slice-1b's snapshot policy.
//!
//! The arbitrary strategy generates only VALID event sequences (so the
//! `debug_assert!("two open turns")` invariant in `Session::apply` is
//! never tripped). We model a small state machine: started → optional
//! turn cycles → optional crash/end.

use chrono::Utc;
use nephila_core::id::{AgentId, CheckpointId};
use nephila_core::session::Session;
use nephila_core::session_event::SessionEvent;
use nephila_eventsourcing::aggregate::EventSourced;
use nephila_eventsourcing::envelope::EventEnvelope;
use nephila_eventsourcing::id::{EventId, TraceId};
use proptest::prelude::*;
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone, Copy)]
enum Action {
    Queue,
    Deliver,
    AssistantMessage,
    Checkpoint,
    Complete,
    Abort,
    End,
}

fn arb_actions() -> impl Strategy<Value = Vec<Action>> {
    let action = prop_oneof![
        Just(Action::Queue),
        Just(Action::Deliver),
        Just(Action::AssistantMessage),
        Just(Action::Checkpoint),
        Just(Action::Complete),
        Just(Action::Abort),
        Just(Action::End),
    ];
    proptest::collection::vec(action, 1..40)
}

fn actions_to_events(actions: &[Action]) -> Vec<SessionEvent> {
    let ts = Utc::now();
    let session_id = Uuid::new_v4();
    let agent_id = AgentId::new();
    let mut events: Vec<SessionEvent> = vec![SessionEvent::SessionStarted {
        session_id,
        agent_id,
        model: None,
        working_dir: std::path::PathBuf::from("/"),
        mcp_endpoint: "m".into(),
        resumed: false,
        ts,
    }];
    let mut open_turn: Option<Uuid> = None;
    let mut ended = false;
    for a in actions {
        if ended {
            break;
        }
        match a {
            Action::Queue => {
                if open_turn.is_none() {
                    let t = Uuid::new_v4();
                    open_turn = Some(t);
                    events.push(SessionEvent::AgentPromptQueued {
                        turn_id: t,
                        text: "p".into(),
                        ts,
                    });
                }
            }
            Action::Deliver => {
                if let Some(t) = open_turn {
                    events.push(SessionEvent::AgentPromptDelivered { turn_id: t, ts });
                }
            }
            Action::AssistantMessage => {
                events.push(SessionEvent::AssistantMessage {
                    message_id: "m".into(),
                    seq_in_message: 0,
                    delta_text: "x".into(),
                    is_final: false,
                    truncated: false,
                    ts,
                });
            }
            Action::Checkpoint => {
                events.push(SessionEvent::CheckpointReached {
                    checkpoint_id: CheckpointId::new(),
                    interrupt: None,
                    ts,
                });
            }
            Action::Complete => {
                if let Some(t) = open_turn.take() {
                    events.push(SessionEvent::TurnCompleted {
                        turn_id: t,
                        stop_reason: "end_turn".into(),
                        ts,
                    });
                }
            }
            Action::Abort => {
                if let Some(t) = open_turn.take() {
                    events.push(SessionEvent::TurnAborted {
                        turn_id: t,
                        reason: "abort".into(),
                        ts,
                    });
                }
            }
            Action::End => {
                events.push(SessionEvent::SessionEnded { ts });
                ended = true;
            }
        }
    }
    events
}

fn envelope(event: &SessionEvent, sequence: u64, agg_id: &str) -> EventEnvelope {
    let mut env = EventEnvelope::new(nephila_eventsourcing::envelope::NewEventEnvelope {
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
    });
    env.set_sequence(sequence);
    env
}

fn fold_apply_envelope(start: Session, envs: &[EventEnvelope]) -> Session {
    envs.iter()
        .fold(start, |s, env| s.apply_envelope(env).unwrap())
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        ..ProptestConfig::default()
    })]
    #[test]
    fn replaying_from_mid_point_snapshot_yields_same_state(actions in arb_actions(), k_seed in any::<u8>()) {
        let events = actions_to_events(&actions);
        let agg = "s";
        let envs: Vec<_> = events
            .iter()
            .enumerate()
            .map(|(i, e)| envelope(e, (i + 1) as u64, agg))
            .collect();

        let s1 = fold_apply_envelope(Session::default_state(), &envs);

        let k = (k_seed as usize) % events.len().max(1);
        let mid = fold_apply_envelope(Session::default_state(), &envs[..k]);
        let s2 = fold_apply_envelope(mid, &envs[k..]);

        prop_assert_eq!(s1, s2);
    }
}

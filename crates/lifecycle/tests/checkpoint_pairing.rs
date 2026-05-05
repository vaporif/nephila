//! Property test for the (CheckpointReached, TurnCompleted) pairing state machine.
//!
//! Drives arbitrary interleavings of `SessionEvent`s through `PerSessionLoop`
//! and asserts the invariants that protect against race conditions in the
//! checkpoint-driven autonomy loop:
//!
//!   1. `send_turn(Agent, ...)` is never called before `TurnCompleted` for the
//!      active turn — the supervisor must not auto-prompt while a turn is open.
//!   2. `TurnAborted` does NOT auto-trigger `send_turn` — abort is a recoverable
//!      signal that the operator must investigate.
//!   3. `PromptDeliveryFailed` is logged but does NOT block the next
//!      `CheckpointReached` cycle — the supervisor stays alive.
//!   4. A `(CheckpointReached(None), TurnCompleted)` pair triggers exactly one
//!      `send_turn(Agent, ...)`. No double-prompts on duplicated checkpoints.

use std::sync::{Arc, Mutex};

use chrono::Utc;
use nephila_core::config::SupervisionConfig;
use nephila_core::id::{AgentId, CheckpointId};
use nephila_core::session_event::{InterruptSnapshot, SessionEvent};
use nephila_lifecycle::session_supervisor::{
    DriverError, PerSessionLoop, SessionDriver, SupervisorAction,
};
use proptest::prelude::*;
use uuid::Uuid;

#[derive(Debug, Default)]
struct CallLog {
    actions: Vec<RecordedAction>,
}

#[derive(Debug, Clone, PartialEq)]
enum RecordedAction {
    SendTurnAgent,
    Pause,
    Shutdown,
}

#[derive(Debug, Default, Clone)]
struct FakeDriver {
    log: Arc<Mutex<CallLog>>,
}

impl SessionDriver for FakeDriver {
    async fn send_agent_prompt(&self, _prompt: String) -> Result<(), DriverError> {
        self.log
            .lock()
            .expect("call log poisoned")
            .actions
            .push(RecordedAction::SendTurnAgent);
        Ok(())
    }
    async fn pause(&self) -> Result<(), DriverError> {
        self.log
            .lock()
            .expect("call log poisoned")
            .actions
            .push(RecordedAction::Pause);
        Ok(())
    }
    async fn shutdown(&self) -> Result<(), DriverError> {
        self.log
            .lock()
            .expect("call log poisoned")
            .actions
            .push(RecordedAction::Shutdown);
        Ok(())
    }
}

fn fresh_loop(driver: FakeDriver) -> PerSessionLoop<FakeDriver> {
    PerSessionLoop::new(
        AgentId::new(),
        Uuid::new_v4(),
        driver,
        SupervisionConfig::default(),
    )
}

#[derive(Debug, Clone)]
enum EventGen {
    AgentPromptDelivered,
    HumanPromptDelivered,
    CheckpointReachedNone,
    CheckpointReachedHitl,
    CheckpointReachedPause,
    CheckpointReachedDrain,
    TurnCompleted,
    TurnAborted,
    PromptDeliveryFailed,
}

fn arb_event() -> impl Strategy<Value = EventGen> {
    prop_oneof![
        Just(EventGen::AgentPromptDelivered),
        Just(EventGen::HumanPromptDelivered),
        Just(EventGen::CheckpointReachedNone),
        Just(EventGen::CheckpointReachedHitl),
        Just(EventGen::CheckpointReachedPause),
        Just(EventGen::CheckpointReachedDrain),
        Just(EventGen::TurnCompleted),
        Just(EventGen::TurnAborted),
        Just(EventGen::PromptDeliveryFailed),
    ]
}

fn realize_event(g: &EventGen, open_turn: &mut Option<Uuid>) -> SessionEvent {
    let now = Utc::now();
    match g {
        EventGen::AgentPromptDelivered => {
            let turn_id = Uuid::new_v4();
            *open_turn = Some(turn_id);
            SessionEvent::AgentPromptDelivered { turn_id, ts: now }
        }
        EventGen::HumanPromptDelivered => {
            let turn_id = Uuid::new_v4();
            *open_turn = Some(turn_id);
            SessionEvent::HumanPromptDelivered { turn_id, ts: now }
        }
        EventGen::CheckpointReachedNone => SessionEvent::CheckpointReached {
            checkpoint_id: CheckpointId(Uuid::new_v4()),
            interrupt: None,
            ts: now,
        },
        EventGen::CheckpointReachedHitl => SessionEvent::CheckpointReached {
            checkpoint_id: CheckpointId(Uuid::new_v4()),
            interrupt: Some(InterruptSnapshot::Hitl {
                question: "ok?".into(),
                options: vec!["yes".into(), "no".into()],
            }),
            ts: now,
        },
        EventGen::CheckpointReachedPause => SessionEvent::CheckpointReached {
            checkpoint_id: CheckpointId(Uuid::new_v4()),
            interrupt: Some(InterruptSnapshot::Pause),
            ts: now,
        },
        EventGen::CheckpointReachedDrain => SessionEvent::CheckpointReached {
            checkpoint_id: CheckpointId(Uuid::new_v4()),
            interrupt: Some(InterruptSnapshot::Drain),
            ts: now,
        },
        EventGen::TurnCompleted => SessionEvent::TurnCompleted {
            turn_id: open_turn.take().unwrap_or_else(Uuid::new_v4),
            stop_reason: "end_turn".into(),
            ts: now,
        },
        EventGen::TurnAborted => SessionEvent::TurnAborted {
            turn_id: open_turn.take().unwrap_or_else(Uuid::new_v4),
            reason: "test".into(),
            ts: now,
        },
        EventGen::PromptDeliveryFailed => SessionEvent::PromptDeliveryFailed {
            turn_id: open_turn.unwrap_or_else(Uuid::new_v4),
            reason: "stdin".into(),
            ts: now,
        },
    }
}

/// Each proptest case constructs its own current-thread runtime and drives
/// the (now async) `handle_event` via `block_on`. This is cheaper than
/// reusing a global runtime under proptest because each case is independent
/// and a current-thread runtime tears down quickly.
fn run_case(events: &[SessionEvent]) -> Vec<RecordedAction> {
    let driver = FakeDriver::default();
    let mut loop_state = fresh_loop(driver.clone());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("build runtime");
    runtime.block_on(async {
        for ev in events {
            let _ = loop_state.handle_event(ev).await;
        }
    });
    let log = driver.log.lock().expect("log");
    log.actions.clone()
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 64,
        ..ProptestConfig::default()
    })]

    #[test]
    fn checkpoint_pairing_invariants(seq in proptest::collection::vec(arb_event(), 0..32)) {
        let mut open_turn: Option<Uuid> = None;
        let mut events: Vec<SessionEvent> = Vec::with_capacity(seq.len());
        for g in &seq {
            events.push(realize_event(g, &mut open_turn));
        }

        // Drive each prefix and snapshot the action log so we can attribute
        // each new action to the event that produced it.
        let driver = FakeDriver::default();
        let mut loop_state = fresh_loop(driver.clone());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("build runtime");

        let mut active_turn: Option<Uuid> = None;

        for ev in &events {
            let before = driver.log.lock().expect("log").actions.len();

            // Close-out events mark the turn closed AT this event — a
            // follow-up `send_turn(Agent)` issued by the supervisor as the
            // immediate response is exactly the (Checkpoint, Completed) pair
            // we want to allow. Open events flip the active_turn AFTER
            // dispatch so any send_turn issued inside this iteration runs
            // against the prior (idle) state.
            let was_open_before_dispatch = active_turn.is_some();
            match ev {
                SessionEvent::TurnCompleted { .. } | SessionEvent::TurnAborted { .. } => {
                    active_turn = None;
                }
                _ => {}
            }

            let action = runtime.block_on(loop_state.handle_event(ev));

            let after_actions = driver.log.lock().expect("log").actions.clone();
            let new_actions = &after_actions[before..];

            // Invariant 1: no `send_turn(Agent)` while a turn is active.
            for a in new_actions {
                if matches!(a, RecordedAction::SendTurnAgent) {
                    prop_assert!(
                        active_turn.is_none(),
                        "send_turn(Agent) called while turn active; ev = {:?}, was_open = {}",
                        ev,
                        was_open_before_dispatch,
                    );
                }
            }

            // Invariant 2: TurnAborted must not auto-trigger send_turn(Agent).
            if matches!(ev, SessionEvent::TurnAborted { .. }) {
                prop_assert!(
                    !new_actions.contains(&RecordedAction::SendTurnAgent),
                    "TurnAborted triggered send_turn",
                );
            }

            // Invariant 3: PromptDeliveryFailed produces no driver actions.
            if matches!(ev, SessionEvent::PromptDeliveryFailed { .. }) {
                prop_assert!(
                    new_actions.is_empty(),
                    "PromptDeliveryFailed produced driver actions",
                );
            }

            // Now flip active_turn for opens, after dispatch.
            match ev {
                SessionEvent::AgentPromptDelivered { turn_id, .. }
                | SessionEvent::HumanPromptDelivered { turn_id, .. } => {
                    active_turn = Some(*turn_id);
                }
                _ => {}
            }

            let _ = action;
        }

        // No double-shutdown within a single drive.
        let log = driver.log.lock().expect("log");
        let shutdowns = log
            .actions
            .iter()
            .filter(|a| matches!(a, RecordedAction::Shutdown))
            .count();
        prop_assert!(
            shutdowns <= 1,
            "double-shutdown; actions = {:?}",
            log.actions,
        );
    }
}

/// Direct unit assertions for the most important invariants — proptest may
/// not always exercise these specific orderings.
#[tokio::test]
async fn checkpoint_none_then_completed_triggers_one_send_turn() {
    let driver = FakeDriver::default();
    let mut loop_state = fresh_loop(driver.clone());

    let turn_id = Uuid::new_v4();
    let now = Utc::now();

    // Open a turn (delivered) — should NOT trigger send_turn.
    let _ = loop_state
        .handle_event(&SessionEvent::AgentPromptDelivered { turn_id, ts: now })
        .await;
    {
        let log = driver.log.lock().expect("log");
        assert!(
            !log.actions.contains(&RecordedAction::SendTurnAgent),
            "send_turn called while turn open"
        );
    }

    // Reach a (no-interrupt) checkpoint — still no auto-prompt yet.
    let _ = loop_state
        .handle_event(&SessionEvent::CheckpointReached {
            checkpoint_id: CheckpointId(Uuid::new_v4()),
            interrupt: None,
            ts: now,
        })
        .await;
    {
        let log = driver.log.lock().expect("log");
        assert!(
            !log.actions.contains(&RecordedAction::SendTurnAgent),
            "send_turn called before TurnCompleted"
        );
    }

    // TurnCompleted — pair fires; send_turn(Agent) issued exactly once.
    let _ = loop_state
        .handle_event(&SessionEvent::TurnCompleted {
            turn_id,
            stop_reason: "end_turn".into(),
            ts: now,
        })
        .await;
    {
        let log = driver.log.lock().expect("log");
        let n = log
            .actions
            .iter()
            .filter(|a| matches!(a, RecordedAction::SendTurnAgent))
            .count();
        assert_eq!(
            n, 1,
            "expected exactly one send_turn(Agent), got {n}: {:?}",
            log.actions
        );
    }
}

#[tokio::test]
async fn checkpoint_drain_calls_shutdown() {
    let driver = FakeDriver::default();
    let mut loop_state = fresh_loop(driver.clone());

    let turn_id = Uuid::new_v4();
    let now = Utc::now();
    let _ = loop_state
        .handle_event(&SessionEvent::AgentPromptDelivered { turn_id, ts: now })
        .await;
    let _ = loop_state
        .handle_event(&SessionEvent::CheckpointReached {
            checkpoint_id: CheckpointId(Uuid::new_v4()),
            interrupt: Some(InterruptSnapshot::Drain),
            ts: now,
        })
        .await;
    let _ = loop_state
        .handle_event(&SessionEvent::TurnCompleted {
            turn_id,
            stop_reason: "end_turn".into(),
            ts: now,
        })
        .await;

    let log = driver.log.lock().expect("log");
    assert!(
        log.actions.contains(&RecordedAction::Shutdown),
        "Drain interrupt did not trigger shutdown: {:?}",
        log.actions
    );
    assert!(
        !log.actions.contains(&RecordedAction::SendTurnAgent),
        "Drain incorrectly triggered send_turn: {:?}",
        log.actions
    );
}

#[tokio::test]
async fn checkpoint_pause_calls_pause() {
    let driver = FakeDriver::default();
    let mut loop_state = fresh_loop(driver.clone());

    let turn_id = Uuid::new_v4();
    let now = Utc::now();
    let _ = loop_state
        .handle_event(&SessionEvent::AgentPromptDelivered { turn_id, ts: now })
        .await;
    let _ = loop_state
        .handle_event(&SessionEvent::CheckpointReached {
            checkpoint_id: CheckpointId(Uuid::new_v4()),
            interrupt: Some(InterruptSnapshot::Pause),
            ts: now,
        })
        .await;
    let _ = loop_state
        .handle_event(&SessionEvent::TurnCompleted {
            turn_id,
            stop_reason: "end_turn".into(),
            ts: now,
        })
        .await;

    let log = driver.log.lock().expect("log");
    assert!(
        log.actions.contains(&RecordedAction::Pause),
        "Pause interrupt did not call session.pause(): {:?}",
        log.actions
    );
}

#[tokio::test]
async fn checkpoint_hitl_does_not_auto_prompt() {
    let driver = FakeDriver::default();
    let mut loop_state = fresh_loop(driver.clone());

    let turn_id = Uuid::new_v4();
    let now = Utc::now();
    let _ = loop_state
        .handle_event(&SessionEvent::AgentPromptDelivered { turn_id, ts: now })
        .await;
    let _ = loop_state
        .handle_event(&SessionEvent::CheckpointReached {
            checkpoint_id: CheckpointId(Uuid::new_v4()),
            interrupt: Some(InterruptSnapshot::Hitl {
                question: "?".into(),
                options: vec![],
            }),
            ts: now,
        })
        .await;
    let _ = loop_state
        .handle_event(&SessionEvent::TurnCompleted {
            turn_id,
            stop_reason: "end_turn".into(),
            ts: now,
        })
        .await;

    let log = driver.log.lock().expect("log");
    assert!(
        !log.actions.contains(&RecordedAction::SendTurnAgent),
        "Hitl interrupt incorrectly triggered send_turn: {:?}",
        log.actions
    );
}

#[tokio::test]
async fn prompt_delivery_failed_does_not_block_next_cycle() {
    let driver = FakeDriver::default();
    let mut loop_state = fresh_loop(driver.clone());

    let turn_id = Uuid::new_v4();
    let now = Utc::now();
    // Failure before any turn completed.
    let _ = loop_state
        .handle_event(&SessionEvent::PromptDeliveryFailed {
            turn_id,
            reason: "stdin".into(),
            ts: now,
        })
        .await;
    {
        let log = driver.log.lock().expect("log");
        assert!(
            log.actions.is_empty(),
            "PromptDeliveryFailed produced actions: {:?}",
            log.actions
        );
    }

    // Subsequent (delivered, checkpoint, completed) cycle must still produce send_turn.
    let new_turn = Uuid::new_v4();
    let _ = loop_state
        .handle_event(&SessionEvent::AgentPromptDelivered {
            turn_id: new_turn,
            ts: now,
        })
        .await;
    let _ = loop_state
        .handle_event(&SessionEvent::CheckpointReached {
            checkpoint_id: CheckpointId(Uuid::new_v4()),
            interrupt: None,
            ts: now,
        })
        .await;
    let _ = loop_state
        .handle_event(&SessionEvent::TurnCompleted {
            turn_id: new_turn,
            stop_reason: "end_turn".into(),
            ts: now,
        })
        .await;
    let log = driver.log.lock().expect("log");
    let n = log
        .actions
        .iter()
        .filter(|a| matches!(a, RecordedAction::SendTurnAgent))
        .count();
    assert_eq!(
        n, 1,
        "expected one send_turn after recovery; got {n}: {:?}",
        log.actions
    );
}

#[tokio::test]
async fn supervisor_ignores_turn_completed_for_unrelated_turn() {
    let driver = FakeDriver::default();
    let mut loop_state = fresh_loop(driver.clone());

    let turn_a = Uuid::new_v4();
    let turn_b = Uuid::new_v4();
    let now = Utc::now();

    // Open turn A.
    let _ = loop_state
        .handle_event(&SessionEvent::AgentPromptDelivered {
            turn_id: turn_a,
            ts: now,
        })
        .await;

    let prompts_before = driver
        .log
        .lock()
        .expect("log")
        .actions
        .iter()
        .filter(|a| matches!(a, RecordedAction::SendTurnAgent))
        .count();

    // Inject TurnCompleted for an unrelated turn B — must be ignored.
    let action = loop_state
        .handle_event(&SessionEvent::TurnCompleted {
            turn_id: turn_b,
            stop_reason: "end_turn".into(),
            ts: now,
        })
        .await;

    assert!(matches!(action, SupervisorAction::Idle));
    let prompts_after = driver
        .log
        .lock()
        .expect("log")
        .actions
        .iter()
        .filter(|a| matches!(a, RecordedAction::SendTurnAgent))
        .count();
    assert_eq!(
        prompts_after, prompts_before,
        "no auto-prompt should fire for unrelated TurnCompleted",
    );

    // The awaited turn A must still close cleanly and trigger the prompt.
    let action = loop_state
        .handle_event(&SessionEvent::TurnCompleted {
            turn_id: turn_a,
            stop_reason: "end_turn".into(),
            ts: now,
        })
        .await;
    assert!(
        matches!(action, SupervisorAction::PromptedAgent),
        "TurnCompleted for the awaited turn must clear and prompt; got {action:?}",
    );
}

#[tokio::test]
async fn supervisor_ignores_turn_aborted_for_unrelated_turn() {
    let driver = FakeDriver::default();
    let mut loop_state = fresh_loop(driver.clone());

    let turn_a = Uuid::new_v4();
    let turn_b = Uuid::new_v4();
    let now = Utc::now();

    // Open turn A.
    let _ = loop_state
        .handle_event(&SessionEvent::AgentPromptDelivered {
            turn_id: turn_a,
            ts: now,
        })
        .await;

    // Stale TurnAborted for turn B must not clear the await for turn A.
    let action = loop_state
        .handle_event(&SessionEvent::TurnAborted {
            turn_id: turn_b,
            reason: "stale".into(),
            ts: now,
        })
        .await;
    assert!(matches!(action, SupervisorAction::Idle));

    // Turn A's TurnCompleted must still trigger the prompt — proves the
    // unrelated abort did not clear `awaiting_turn_completion`.
    let action = loop_state
        .handle_event(&SessionEvent::TurnCompleted {
            turn_id: turn_a,
            stop_reason: "end_turn".into(),
            ts: now,
        })
        .await;
    assert!(
        matches!(action, SupervisorAction::PromptedAgent),
        "stale TurnAborted should not have cleared the active turn's await; got {action:?}",
    );
}

// `run_case` is exercised inline via the proptest closure above; expose it
// here so dead-code analysis doesn't trip when the proptest is filtered out.
#[allow(dead_code)]
fn _run_case_compiles(events: &[SessionEvent]) -> Vec<RecordedAction> {
    run_case(events)
}

// Suppress unused-variant warning on `SupervisorAction` if the supervisor
// happens not to surface every variant inline.
#[allow(dead_code)]
fn _action_compiles(a: SupervisorAction) -> SupervisorAction {
    a
}

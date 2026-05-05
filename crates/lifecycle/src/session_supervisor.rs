//! `SessionSupervisor` — checkpoint-driven autonomy loop.
//!
//! Slice 3: replaces today's `BusEvent`-driven `LifecycleSupervisor` for the
//! per-session control flow with a state machine that consumes `SessionEvent`s
//! produced by the connector. The pair `(CheckpointReached, TurnCompleted)`
//! is the trigger for the next autonomy step:
//!
//!   - `interrupt = None`     → compose next prompt and `send_turn(Agent, ..)`
//!   - `interrupt = Some(Hitl)`  → stay quiet; the operator drives next via TUI
//!   - `interrupt = Some(Pause)` → SIGSTOP the child via `session.pause()`
//!   - `interrupt = Some(Drain)` → graceful `session.shutdown()`
//!
//! `TurnAborted` is a recoverable signal that does NOT auto-prompt — the
//! operator decides whether to continue. `PromptDeliveryFailed` is logged but
//! does not block the next checkpoint cycle.
//!
//! The supervisor uses a `SessionDriver` trait so tests can substitute a
//! call-recording fake. In production the only impl wraps `Arc<ClaudeCodeSession>`
//! in a thin shim that translates the trait calls into `send_turn` / `pause` /
//! `shutdown`.
//!
//! Slice 3 ships the state machine, the `SessionDriver` trait,
//! [`run_per_session`], and the registry stub. The bridge from
//! `SessionRegistry` → `run_per_session` lands in slice 4 (Task 6 step 5a)
//! once `Agent::session_id` exists. Until then the supervisor is exercised
//! only by `crates/lifecycle/tests/checkpoint_pairing.rs`'s proptest.

use std::collections::HashMap;
use std::sync::Arc;

use futures::StreamExt;
use nephila_core::config::SupervisionConfig;
use nephila_core::id::AgentId;
use nephila_core::session_event::{InterruptSnapshot, SessionEvent, SessionId, TurnId};
use nephila_store::SqliteStore;
use nephila_store::resilient_subscribe::resilient_subscribe;
use tokio::sync::Mutex;

use crate::supervisor::RestartTracker;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionPhase {
    Running,
    WaitingHitl,
    Paused,
    Crashed,
    Ended,
}

impl SessionPhase {
    const fn is_terminal(self) -> bool {
        matches!(self, Self::Ended | Self::Crashed)
    }
}

/// Per-session state tracked by the supervisor.
#[derive(Debug)]
struct PerSession {
    agent_id: AgentId,
    /// The most recent `CheckpointReached` envelope for which we have not yet
    /// observed the matching `TurnCompleted`. Cleared on `TurnCompleted` (and
    /// driven by the (checkpoint, completed) pair).
    last_checkpoint: Option<InterruptDecision>,
    /// `Some(turn_id)` while a `*PromptDelivered` has fired and no
    /// `TurnCompleted`/`TurnAborted`/`PromptDeliveryFailed` has closed it.
    awaiting_turn_completion: Option<TurnId>,
    phase: SessionPhase,
    driver: Arc<dyn SessionDriver>,
    /// Crash budget — incremented on `SessionCrashed`, cleared on
    /// `SessionEnded`. Slice 4's registry consults
    /// `restart_tracker.record_restart()` before respawning a crashed session.
    restart_tracker: RestartTracker,
}

#[derive(Debug, Clone)]
struct InterruptDecision {
    interrupt: Option<InterruptSnapshot>,
}

/// Abstracts the per-session control surface so the supervisor can be unit
/// tested without spawning a real claude child process.
///
/// All methods are sync and fire-and-forget: the production impl forwards
/// each call into a `tokio::spawn` to avoid blocking the supervisor's loop on
/// awaits across many sessions.
pub trait SessionDriver: Send + Sync + std::fmt::Debug {
    /// Issue an autonomy prompt as `PromptSource::Agent`.
    fn send_agent_prompt(&self, prompt: String);
    /// SIGSTOP the underlying claude process.
    fn pause(&self);
    /// Graceful shutdown — drain reader/writer, persist `SessionEnded`.
    fn shutdown(&self);
}

/// Surfaced action for tracing / observability. Not used by the production
/// loop directly — callers can choose to log it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupervisorAction {
    Idle,
    PromptedAgent,
    Paused,
    ShutdownRequested,
    HitlAwaiting,
    Logged,
    Crashed,
    Ended,
}

pub struct SessionSupervisor {
    sessions: HashMap<SessionId, PerSession>,
    supervision_config: SupervisionConfig,
    store: Arc<SqliteStore>,
}

impl std::fmt::Debug for SessionSupervisor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionSupervisor")
            .field("session_count", &self.sessions.len())
            .field("supervision_config", &self.supervision_config)
            .finish_non_exhaustive()
    }
}

fn default_supervision_config() -> SupervisionConfig {
    SupervisionConfig {
        default_strategy: "one_for_one".into(),
        max_restarts: 5,
        restart_window_secs: 600,
        max_agent_depth: 3,
    }
}

impl SessionSupervisor {
    #[must_use]
    pub fn new(store: Arc<SqliteStore>, supervision_config: SupervisionConfig) -> Self {
        Self {
            sessions: HashMap::new(),
            supervision_config,
            store,
        }
    }

    /// Accessor for the store used by [`run_per_session`] when re-anchoring
    /// the per-session subscription.
    #[must_use]
    pub fn store(&self) -> Arc<SqliteStore> {
        Arc::clone(&self.store)
    }

    /// Test-only constructor. Spins up an in-memory store so the proptest
    /// harness can exercise the state machine without a tempdir or real
    /// `subscribe_after` plumbing.
    #[must_use]
    pub fn new_for_test() -> Self {
        let store = Arc::new(SqliteStore::open_in_memory(384).expect("test store opens in-memory"));
        Self::new(store, default_supervision_config())
    }

    /// Test seam: register a session with a fake driver before driving events.
    /// The production registry calls `attach_session` (no `_for_test` suffix)
    /// — see `LifecycleSupervisor` once the registry lands in slice 4.
    pub fn attach_session_for_test<D: SessionDriver + 'static>(
        &mut self,
        agent_id: AgentId,
        session_id: SessionId,
        driver: D,
    ) {
        self.attach_session(agent_id, session_id, Arc::new(driver));
    }

    /// Register a session with the supervisor.
    pub fn attach_session(
        &mut self,
        agent_id: AgentId,
        session_id: SessionId,
        driver: Arc<dyn SessionDriver>,
    ) {
        self.sessions.insert(
            session_id,
            PerSession {
                agent_id,
                last_checkpoint: None,
                awaiting_turn_completion: None,
                phase: SessionPhase::Running,
                driver,
                restart_tracker: RestartTracker::new(self.supervision_config.clone()),
            },
        );
    }

    /// Test-only event handler. Production loop reads events through
    /// `resilient_subscribe` and routes them through the same state machine.
    pub fn handle_event_for_test(
        &mut self,
        session_id: SessionId,
        event: &SessionEvent,
    ) -> SupervisorAction {
        self.handle_event(session_id, event)
    }

    /// Drive the per-session state machine on an incoming `SessionEvent`.
    ///
    /// Returns the action taken (for tracing/test assertions). The action's
    /// side effect (e.g. a `send_agent_prompt` call into the `SessionDriver`)
    /// has already been issued by the time this returns.
    pub fn handle_event(
        &mut self,
        session_id: SessionId,
        event: &SessionEvent,
    ) -> SupervisorAction {
        let Some(state) = self.sessions.get_mut(&session_id) else {
            tracing::debug!(%session_id, "event for unattached session — ignoring");
            return SupervisorAction::Idle;
        };

        if state.phase.is_terminal() {
            tracing::debug!(%session_id, ?event, "event after terminal phase — ignoring");
            return SupervisorAction::Idle;
        }

        match event {
            SessionEvent::AgentPromptDelivered { turn_id, .. }
            | SessionEvent::HumanPromptDelivered { turn_id, .. } => {
                state.awaiting_turn_completion = Some(*turn_id);
                SupervisorAction::Idle
            }
            SessionEvent::CheckpointReached { interrupt, .. } => {
                state.last_checkpoint = Some(InterruptDecision {
                    interrupt: interrupt.clone(),
                });
                SupervisorAction::Idle
            }
            SessionEvent::TurnCompleted { turn_id, .. } => {
                if state.awaiting_turn_completion != Some(*turn_id) {
                    tracing::debug!(
                        %session_id,
                        %turn_id,
                        awaiting = ?state.awaiting_turn_completion,
                        "TurnCompleted for unrelated turn; ignoring",
                    );
                    return SupervisorAction::Idle;
                }
                state.awaiting_turn_completion = None;
                let cp = state.last_checkpoint.take();
                let agent_id = state.agent_id;
                match cp {
                    Some(InterruptDecision { interrupt }) => match interrupt {
                        None => {
                            let prompt = compose_continuation_prompt(agent_id);
                            state.driver.send_agent_prompt(prompt);
                            SupervisorAction::PromptedAgent
                        }
                        Some(InterruptSnapshot::Hitl { .. }) => {
                            state.phase = SessionPhase::WaitingHitl;
                            SupervisorAction::HitlAwaiting
                        }
                        Some(InterruptSnapshot::Pause) => {
                            state.phase = SessionPhase::Paused;
                            state.driver.pause();
                            SupervisorAction::Paused
                        }
                        Some(InterruptSnapshot::Drain) => {
                            state.phase = SessionPhase::Ended;
                            state.driver.shutdown();
                            SupervisorAction::ShutdownRequested
                        }
                    },
                    None => {
                        // Steady-state autonomy — turn closed without an
                        // explicit checkpoint in this loop iteration. Compose
                        // next prompt regardless.
                        let prompt = compose_continuation_prompt(agent_id);
                        state.driver.send_agent_prompt(prompt);
                        SupervisorAction::PromptedAgent
                    }
                }
            }
            SessionEvent::TurnAborted {
                turn_id, reason, ..
            } => {
                if state.awaiting_turn_completion != Some(*turn_id) {
                    tracing::debug!(
                        %session_id,
                        %turn_id,
                        awaiting = ?state.awaiting_turn_completion,
                        "TurnAborted for unrelated turn; ignoring",
                    );
                    return SupervisorAction::Idle;
                }
                state.awaiting_turn_completion = None;
                state.last_checkpoint = None;
                tracing::warn!(
                    %session_id,
                    %turn_id,
                    %reason,
                    "TurnAborted — recoverable; not auto-prompting",
                );
                SupervisorAction::Logged
            }
            SessionEvent::PromptDeliveryFailed {
                turn_id, reason, ..
            } => {
                tracing::warn!(
                    %session_id,
                    %turn_id,
                    %reason,
                    "PromptDeliveryFailed — next CheckpointReached cycle will retry",
                );
                SupervisorAction::Logged
            }
            SessionEvent::SessionCrashed {
                reason, exit_code, ..
            } => {
                state.phase = SessionPhase::Crashed;
                let allowed = state.restart_tracker.record_restart();
                tracing::error!(
                    %session_id,
                    %reason,
                    ?exit_code,
                    restart_allowed = allowed,
                    restart_count = state.restart_tracker.restart_count(),
                    "session crashed",
                );
                SupervisorAction::Crashed
            }
            SessionEvent::SessionEnded { .. } => {
                state.phase = SessionPhase::Ended;
                state.restart_tracker.reset();
                SupervisorAction::Ended
            }
            // The remaining events are observability-only as far as autonomy
            // is concerned (assistant deltas, tool calls, etc.).
            _ => SupervisorAction::Idle,
        }
    }

    pub fn forget_session(&mut self, session_id: SessionId) {
        self.sessions.remove(&session_id);
    }

    #[must_use]
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }
}

/// Drive a single session's event stream into a shared `SessionSupervisor`.
///
/// Subscribes to `("session", session_id, since=0)` via
/// [`resilient_subscribe`] and routes each `SessionEvent` envelope through
/// [`SessionSupervisor::handle_event`]. Returns when the upstream stream
/// closes (clean session shutdown) or yields a non-recoverable error.
///
/// The supervisor is shared across many such tasks via `Arc<Mutex<_>>`; each
/// task locks briefly per event to dispatch and never holds the lock across
/// `.await` boundaries (the `subscribe` `.next()` call happens BEFORE the
/// lock is acquired). The store handle is read once at task entry under a
/// short-lived lock and reused for the lifetime of the subscription.
#[tracing::instrument(level = "debug", skip(supervisor), fields(session_id = %session_id))]
pub async fn run_per_session(supervisor: Arc<Mutex<SessionSupervisor>>, session_id: SessionId) {
    let store = supervisor.lock().await.store();
    let mut stream = resilient_subscribe(store, "session".into(), session_id.to_string(), 0);
    while let Some(item) = stream.next().await {
        match item {
            Ok(env) => match serde_json::from_value::<SessionEvent>(env.payload.clone()) {
                Ok(ev) => {
                    let mut guard = supervisor.lock().await;
                    let _ = guard.handle_event(session_id, &ev);
                }
                Err(e) => {
                    tracing::warn!(%session_id, error = %e, "decode SessionEvent failed");
                }
            },
            Err(e) => {
                tracing::warn!(%session_id, error = %e, "subscribe stream error; ending loop");
                return;
            }
        }
    }
}

/// Compose the next per-turn continuation prompt for an autonomous agent.
///
/// Today this is a static "continue" string — the spawn-time prompt
/// (system prompt + objective) lives in
/// [`crate::lifecycle_supervisor::compose_next_prompt`]. Slice 5+ may inject
/// richer state-aware context (memory recap, recent activity).
#[must_use]
pub fn compose_continuation_prompt(agent_id: AgentId) -> String {
    format!(
        "Continue with your objective. (agent_id={agent_id})\n\
         If your work is complete, call `serialize_and_persist` with `final=true`.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use nephila_core::id::CheckpointId;
    use std::sync::Mutex;
    use uuid::Uuid;

    #[derive(Debug, Default, Clone)]
    struct RecorderDriver {
        prompts: Arc<Mutex<Vec<String>>>,
        pauses: Arc<Mutex<u32>>,
        shutdowns: Arc<Mutex<u32>>,
    }

    impl SessionDriver for RecorderDriver {
        fn send_agent_prompt(&self, prompt: String) {
            self.prompts.lock().unwrap().push(prompt);
        }
        fn pause(&self) {
            *self.pauses.lock().unwrap() += 1;
        }
        fn shutdown(&self) {
            *self.shutdowns.lock().unwrap() += 1;
        }
    }

    #[test]
    fn unattached_session_is_idle() {
        let mut sup = SessionSupervisor::new_for_test();
        let session_id = Uuid::new_v4();
        let action = sup.handle_event_for_test(
            session_id,
            &SessionEvent::TurnCompleted {
                turn_id: Uuid::new_v4(),
                stop_reason: "end_turn".into(),
                ts: Utc::now(),
            },
        );
        assert_eq!(action, SupervisorAction::Idle);
    }

    #[test]
    fn checkpoint_none_then_completed_prompts_agent() {
        let driver = RecorderDriver::default();
        let mut sup = SessionSupervisor::new_for_test();
        let agent_id = AgentId::new();
        let session_id = Uuid::new_v4();
        sup.attach_session_for_test(agent_id, session_id, driver.clone());

        let turn_id = Uuid::new_v4();
        sup.handle_event_for_test(
            session_id,
            &SessionEvent::AgentPromptDelivered {
                turn_id,
                ts: Utc::now(),
            },
        );
        sup.handle_event_for_test(
            session_id,
            &SessionEvent::CheckpointReached {
                checkpoint_id: CheckpointId(Uuid::new_v4()),
                interrupt: None,
                ts: Utc::now(),
            },
        );
        sup.handle_event_for_test(
            session_id,
            &SessionEvent::TurnCompleted {
                turn_id,
                stop_reason: "end_turn".into(),
                ts: Utc::now(),
            },
        );
        assert_eq!(driver.prompts.lock().unwrap().len(), 1);
    }

    #[test]
    fn turn_aborted_does_not_prompt() {
        let driver = RecorderDriver::default();
        let mut sup = SessionSupervisor::new_for_test();
        let session_id = Uuid::new_v4();
        sup.attach_session_for_test(AgentId::new(), session_id, driver.clone());

        let turn_id = Uuid::new_v4();
        sup.handle_event_for_test(
            session_id,
            &SessionEvent::AgentPromptDelivered {
                turn_id,
                ts: Utc::now(),
            },
        );
        sup.handle_event_for_test(
            session_id,
            &SessionEvent::TurnAborted {
                turn_id,
                reason: "x".into(),
                ts: Utc::now(),
            },
        );
        assert!(driver.prompts.lock().unwrap().is_empty());
    }

    #[test]
    fn drain_interrupt_calls_shutdown() {
        let driver = RecorderDriver::default();
        let mut sup = SessionSupervisor::new_for_test();
        let session_id = Uuid::new_v4();
        sup.attach_session_for_test(AgentId::new(), session_id, driver.clone());

        let turn_id = Uuid::new_v4();
        sup.handle_event_for_test(
            session_id,
            &SessionEvent::AgentPromptDelivered {
                turn_id,
                ts: Utc::now(),
            },
        );
        sup.handle_event_for_test(
            session_id,
            &SessionEvent::CheckpointReached {
                checkpoint_id: CheckpointId(Uuid::new_v4()),
                interrupt: Some(InterruptSnapshot::Drain),
                ts: Utc::now(),
            },
        );
        sup.handle_event_for_test(
            session_id,
            &SessionEvent::TurnCompleted {
                turn_id,
                stop_reason: "end_turn".into(),
                ts: Utc::now(),
            },
        );
        assert_eq!(*driver.shutdowns.lock().unwrap(), 1);
        assert!(driver.prompts.lock().unwrap().is_empty());
    }

    #[test]
    fn pause_interrupt_calls_pause() {
        let driver = RecorderDriver::default();
        let mut sup = SessionSupervisor::new_for_test();
        let session_id = Uuid::new_v4();
        sup.attach_session_for_test(AgentId::new(), session_id, driver.clone());

        let turn_id = Uuid::new_v4();
        sup.handle_event_for_test(
            session_id,
            &SessionEvent::AgentPromptDelivered {
                turn_id,
                ts: Utc::now(),
            },
        );
        sup.handle_event_for_test(
            session_id,
            &SessionEvent::CheckpointReached {
                checkpoint_id: CheckpointId(Uuid::new_v4()),
                interrupt: Some(InterruptSnapshot::Pause),
                ts: Utc::now(),
            },
        );
        sup.handle_event_for_test(
            session_id,
            &SessionEvent::TurnCompleted {
                turn_id,
                stop_reason: "end_turn".into(),
                ts: Utc::now(),
            },
        );
        assert_eq!(*driver.pauses.lock().unwrap(), 1);
    }
}

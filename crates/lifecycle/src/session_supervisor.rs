//! `SessionSupervisor` — checkpoint-driven autonomy loop.
//!
//! Drives per-session control flow off `SessionEvent`s produced by the
//! connector. The pair `(CheckpointReached, TurnCompleted)` triggers the next
//! autonomy step:
//!
//!   - `interrupt = None`        → compose next prompt and `send_turn(Agent, ..)`
//!   - `interrupt = Some(Hitl)`  → stay quiet; the operator drives next via TUI
//!   - `interrupt = Some(Pause)` → SIGSTOP the child via `session.pause()`
//!   - `interrupt = Some(Drain)` → graceful `session.shutdown()`
//!
//! `TurnAborted` is recoverable and does not auto-prompt — the operator
//! decides whether to continue. `PromptDeliveryFailed` is logged but does
//! not block the next checkpoint cycle.
//!
//! The supervisor is sharded: each attached session owns its own
//! [`PerSessionLoop`] task; there is no shared map of per-session state. The
//! outer [`SessionSupervisor`] is a thin registry of `JoinHandle`s used for
//! shutdown.
//!
//! The supervisor uses a [`SessionDriver`] trait so tests can substitute a
//! call-recording fake. The production impl
//! ([`crate::claude_code_driver::ClaudeCodeDriver`]) wraps
//! `Arc<ClaudeCodeSession>` and translates the trait calls into
//! `send_turn` / `pause` / `shutdown_in_place`.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

use futures::StreamExt;
use nephila_core::config::SupervisionConfig;
use nephila_core::id::AgentId;
use nephila_core::session_event::{InterruptSnapshot, SessionEvent, SessionId, TurnId};
use nephila_eventsourcing::store::EventStoreError;
use nephila_store::SqliteStore;
use nephila_store::resilient_subscribe::resilient_subscribe;
use tokio::task::JoinHandle;

use crate::supervisor::RestartTracker;

/// Errors surfaced by [`SessionDriver`] implementations. The supervisor
/// transitions the session to [`SessionPhase::Crashed`] on any error and
/// stops dispatching further driver calls for that session.
#[derive(Debug, thiserror::Error)]
pub enum DriverError {
    /// The underlying claude child is no longer reachable (writer task closed,
    /// pid gone, etc.).
    #[error("process gone")]
    ProcessGone,
    /// IO failure during process control (signal delivery, fs writes).
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Persisting the post-shutdown `SessionEnded` envelope failed.
    #[error("persist: {0}")]
    Persist(#[from] EventStoreError),
}

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

#[derive(Debug, Clone)]
struct InterruptDecision {
    interrupt: Option<InterruptSnapshot>,
}

/// Abstracts the per-session control surface so the supervisor can be unit
/// tested without spawning a real claude child process.
///
/// Each method is `async` and returns a `Result<(), DriverError>`. Errors
/// transition the session to [`SessionPhase::Crashed`]; the per-session loop
/// then drains its subscription without dispatching further driver calls.
///
/// The trait uses RPITIT (return-position `impl Future`) rather than
/// `async fn`, matching the existing `TaskConnector` pattern. Dispatch is
/// monomorphic — each [`PerSessionLoop`] is generic over its `D: SessionDriver`,
/// avoiding `Arc<dyn _>` and the dyn-compatibility limitations of async-fn-
/// in-trait.
pub trait SessionDriver: Send + Sync + std::fmt::Debug + 'static {
    /// Issue an autonomy prompt as `PromptSource::Agent`.
    fn send_agent_prompt(
        &self,
        prompt: String,
    ) -> impl Future<Output = Result<(), DriverError>> + Send;
    /// SIGSTOP the underlying claude process.
    fn pause(&self) -> impl Future<Output = Result<(), DriverError>> + Send;
    /// Graceful shutdown — drain reader/writer, persist `SessionEnded`.
    fn shutdown(&self) -> impl Future<Output = Result<(), DriverError>> + Send;
}

/// Surfaced action for tracing / observability. Returned from
/// [`PerSessionLoop::handle_event`] so tests can assert what the state machine
/// chose to do without inspecting driver call logs.
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

/// Owns per-session state and runs the resilient-subscribe + event-routing
/// loop for ONE session. Each spawned via [`SessionSupervisor::attach_session`]
/// — there is no cross-session shared state.
pub struct PerSessionLoop<D: SessionDriver> {
    agent_id: AgentId,
    session_id: SessionId,
    /// The most recent `CheckpointReached` envelope for which we have not yet
    /// observed the matching `TurnCompleted`. Cleared on `TurnCompleted`.
    last_checkpoint: Option<InterruptDecision>,
    /// `Some(turn_id)` while a `*PromptDelivered` has fired and no
    /// `TurnCompleted`/`TurnAborted`/`PromptDeliveryFailed` has closed it.
    awaiting_turn_completion: Option<TurnId>,
    phase: SessionPhase,
    driver: D,
    /// Crash budget: incremented on `SessionCrashed`, cleared on
    /// `SessionEnded`.
    restart_tracker: RestartTracker,
}

impl<D: SessionDriver> std::fmt::Debug for PerSessionLoop<D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PerSessionLoop")
            .field("agent_id", &self.agent_id)
            .field("session_id", &self.session_id)
            .field("phase", &self.phase)
            .field("awaiting_turn_completion", &self.awaiting_turn_completion)
            .finish_non_exhaustive()
    }
}

impl<D: SessionDriver> PerSessionLoop<D> {
    #[must_use]
    pub fn new(
        agent_id: AgentId,
        session_id: SessionId,
        driver: D,
        supervision_config: SupervisionConfig,
    ) -> Self {
        Self {
            agent_id,
            session_id,
            last_checkpoint: None,
            awaiting_turn_completion: None,
            phase: SessionPhase::Running,
            driver,
            restart_tracker: RestartTracker::new(supervision_config),
        }
    }

    /// Drive the per-session state machine on an incoming `SessionEvent`.
    ///
    /// On a state transition that requires a driver call (auto-prompt, pause,
    /// shutdown), this awaits the call and routes the result. A
    /// [`DriverError`] transitions the session to [`SessionPhase::Crashed`]
    /// and is logged via `tracing::error!`.
    ///
    /// Returns the action taken (for tracing/test assertions).
    pub async fn handle_event(&mut self, event: &SessionEvent) -> SupervisorAction {
        if self.phase.is_terminal() {
            tracing::debug!(session_id = %self.session_id, ?event, "event after terminal phase — ignoring");
            return SupervisorAction::Idle;
        }

        match event {
            SessionEvent::AgentPromptDelivered { turn_id, .. }
            | SessionEvent::HumanPromptDelivered { turn_id, .. } => {
                self.awaiting_turn_completion = Some(*turn_id);
                SupervisorAction::Idle
            }
            SessionEvent::CheckpointReached { interrupt, .. } => {
                self.last_checkpoint = Some(InterruptDecision {
                    interrupt: interrupt.clone(),
                });
                SupervisorAction::Idle
            }
            SessionEvent::TurnCompleted { turn_id, .. } => {
                if self.awaiting_turn_completion != Some(*turn_id) {
                    tracing::debug!(
                        session_id = %self.session_id,
                        %turn_id,
                        awaiting = ?self.awaiting_turn_completion,
                        "TurnCompleted for unrelated turn; ignoring",
                    );
                    return SupervisorAction::Idle;
                }
                self.awaiting_turn_completion = None;
                let cp = self.last_checkpoint.take();
                match cp {
                    Some(InterruptDecision { interrupt }) => match interrupt {
                        None => self.dispatch_continuation_prompt().await,
                        Some(InterruptSnapshot::Hitl { .. }) => {
                            self.phase = SessionPhase::WaitingHitl;
                            SupervisorAction::HitlAwaiting
                        }
                        Some(InterruptSnapshot::Pause) => self.dispatch_pause().await,
                        Some(InterruptSnapshot::Drain) => self.dispatch_shutdown().await,
                    },
                    None => {
                        // Steady-state autonomy — turn closed without an
                        // explicit checkpoint in this loop iteration. Compose
                        // next prompt regardless.
                        self.dispatch_continuation_prompt().await
                    }
                }
            }
            SessionEvent::TurnAborted {
                turn_id, reason, ..
            } => {
                if self.awaiting_turn_completion != Some(*turn_id) {
                    tracing::debug!(
                        session_id = %self.session_id,
                        %turn_id,
                        awaiting = ?self.awaiting_turn_completion,
                        "TurnAborted for unrelated turn; ignoring",
                    );
                    return SupervisorAction::Idle;
                }
                self.awaiting_turn_completion = None;
                self.last_checkpoint = None;
                tracing::warn!(
                    session_id = %self.session_id,
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
                    session_id = %self.session_id,
                    %turn_id,
                    %reason,
                    "PromptDeliveryFailed — next CheckpointReached cycle will retry",
                );
                SupervisorAction::Logged
            }
            SessionEvent::SessionCrashed {
                reason, exit_code, ..
            } => {
                self.phase = SessionPhase::Crashed;
                let allowed = self.restart_tracker.record_restart();
                tracing::error!(
                    session_id = %self.session_id,
                    %reason,
                    ?exit_code,
                    restart_allowed = allowed,
                    restart_count = self.restart_tracker.restart_count(),
                    "session crashed",
                );
                SupervisorAction::Crashed
            }
            SessionEvent::SessionEnded { .. } => {
                self.phase = SessionPhase::Ended;
                self.restart_tracker.reset();
                SupervisorAction::Ended
            }
            // The remaining events are observability-only as far as autonomy
            // is concerned (assistant deltas, tool calls, etc.).
            _ => SupervisorAction::Idle,
        }
    }

    async fn dispatch_continuation_prompt(&mut self) -> SupervisorAction {
        let prompt = compose_continuation_prompt(self.agent_id);
        match self.driver.send_agent_prompt(prompt).await {
            Ok(()) => SupervisorAction::PromptedAgent,
            Err(e) => {
                tracing::error!(
                    session_id = %self.session_id,
                    error = %e,
                    "send_agent_prompt failed; transitioning to Crashed",
                );
                self.phase = SessionPhase::Crashed;
                SupervisorAction::Crashed
            }
        }
    }

    async fn dispatch_pause(&mut self) -> SupervisorAction {
        match self.driver.pause().await {
            Ok(()) => {
                self.phase = SessionPhase::Paused;
                SupervisorAction::Paused
            }
            Err(e) => {
                tracing::error!(
                    session_id = %self.session_id,
                    error = %e,
                    "pause failed; transitioning to Crashed",
                );
                self.phase = SessionPhase::Crashed;
                SupervisorAction::Crashed
            }
        }
    }

    async fn dispatch_shutdown(&mut self) -> SupervisorAction {
        match self.driver.shutdown().await {
            Ok(()) => {
                self.phase = SessionPhase::Ended;
                SupervisorAction::ShutdownRequested
            }
            Err(e) => {
                tracing::error!(
                    session_id = %self.session_id,
                    error = %e,
                    "shutdown failed; transitioning to Crashed",
                );
                self.phase = SessionPhase::Crashed;
                SupervisorAction::Crashed
            }
        }
    }

    /// Drive a single session's event stream into this loop.
    ///
    /// Subscribes to `("session", session_id, since=0)` via
    /// [`resilient_subscribe`] and routes each `SessionEvent` envelope
    /// through [`Self::handle_event`]. Returns when:
    ///   - the upstream stream closes (clean session shutdown), OR
    ///   - the per-session loop transitions to a terminal phase
    ///     ([`SessionPhase::Ended`] or [`SessionPhase::Crashed`]), OR
    ///   - a non-recoverable subscribe error (`PersistentLag`, etc.).
    #[tracing::instrument(level = "debug", skip(self, store), fields(session_id = %self.session_id))]
    pub async fn run(&mut self, store: Arc<SqliteStore>) {
        let mut stream = Box::pin(resilient_subscribe(
            store,
            "session".into(),
            self.session_id.to_string(),
            0,
        ));
        while let Some(item) = stream.next().await {
            match item {
                Ok(env) => match serde_json::from_value::<SessionEvent>(env.payload.clone()) {
                    Ok(ev) => {
                        let _ = self.handle_event(&ev).await;
                        if self.phase.is_terminal() {
                            tracing::debug!(
                                session_id = %self.session_id,
                                phase = ?self.phase,
                                "per-session loop reached terminal phase; exiting",
                            );
                            return;
                        }
                    }
                    Err(e) => {
                        tracing::warn!(session_id = %self.session_id, error = %e, "decode SessionEvent failed");
                    }
                },
                Err(e) => {
                    tracing::warn!(session_id = %self.session_id, error = %e, "subscribe stream error; ending loop");
                    return;
                }
            }
        }
    }

    #[cfg(test)]
    pub fn phase(&self) -> SessionPhase {
        self.phase
    }
}

/// Thin registry of per-session task handles. There is no shared per-session
/// state — each call to [`Self::attach_session`] spawns a [`PerSessionLoop`]
/// task that owns its own state.
pub struct SessionSupervisor {
    handles: HashMap<SessionId, JoinHandle<()>>,
    supervision_config: SupervisionConfig,
    store: Arc<SqliteStore>,
}

impl std::fmt::Debug for SessionSupervisor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionSupervisor")
            .field("session_count", &self.handles.len())
            .field("supervision_config", &self.supervision_config)
            .finish_non_exhaustive()
    }
}

impl SessionSupervisor {
    #[must_use]
    pub fn new(store: Arc<SqliteStore>, supervision_config: SupervisionConfig) -> Self {
        Self {
            handles: HashMap::new(),
            supervision_config,
            store,
        }
    }

    /// Accessor for the store (used by the autonomy task entry point in
    /// `bin/src/main.rs`).
    #[must_use]
    pub fn store(&self) -> Arc<SqliteStore> {
        Arc::clone(&self.store)
    }

    /// Spawn a per-session task that drives the session-event stream through
    /// a fresh [`PerSessionLoop`]. The driver is moved into the spawned task —
    /// no `Arc<dyn _>` indirection.
    ///
    /// If a task is already attached for `session_id`, it is replaced (the
    /// old `JoinHandle` is dropped, which detaches but does not abort it —
    /// callers expecting respawn semantics should call [`Self::detach_session`]
    /// first).
    pub fn attach_session<D: SessionDriver>(
        &mut self,
        agent_id: AgentId,
        session_id: SessionId,
        driver: D,
    ) {
        let store = Arc::clone(&self.store);
        let cfg = self.supervision_config.clone();
        let handle = tokio::spawn(async move {
            let mut loop_state = PerSessionLoop::new(agent_id, session_id, driver, cfg);
            loop_state.run(store).await;
        });
        self.handles.insert(session_id, handle);
    }

    /// Abort and forget the per-session task for `session_id`, if any.
    pub fn detach_session(&mut self, session_id: SessionId) {
        if let Some(handle) = self.handles.remove(&session_id) {
            handle.abort();
        }
    }

    /// Forget the per-session task entry without aborting.
    pub fn forget_session(&mut self, session_id: SessionId) {
        self.handles.remove(&session_id);
    }

    #[must_use]
    pub fn session_count(&self) -> usize {
        self.handles.len()
    }

    /// Abort all per-session tasks and drop their handles. Idempotent.
    pub async fn shutdown_all(&mut self) {
        let handles: Vec<_> = self.handles.drain().map(|(_, h)| h).collect();
        for h in &handles {
            h.abort();
        }
        for h in handles {
            let _ = h.await;
        }
    }
}

/// Compose the next per-turn continuation prompt for an autonomous agent.
///
/// Currently a static "continue" string. The spawn-time prompt (system
/// prompt + objective) lives in
/// [`crate::lifecycle_supervisor::compose_next_prompt`].
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
        async fn send_agent_prompt(&self, prompt: String) -> Result<(), DriverError> {
            self.prompts.lock().unwrap().push(prompt);
            Ok(())
        }
        async fn pause(&self) -> Result<(), DriverError> {
            *self.pauses.lock().unwrap() += 1;
            Ok(())
        }
        async fn shutdown(&self) -> Result<(), DriverError> {
            *self.shutdowns.lock().unwrap() += 1;
            Ok(())
        }
    }

    fn fresh_loop(driver: RecorderDriver) -> PerSessionLoop<RecorderDriver> {
        PerSessionLoop::new(
            AgentId::new(),
            Uuid::new_v4(),
            driver,
            SupervisionConfig::default(),
        )
    }

    #[tokio::test]
    async fn checkpoint_none_then_completed_prompts_agent() {
        let driver = RecorderDriver::default();
        let mut loop_state = fresh_loop(driver.clone());

        let turn_id = Uuid::new_v4();
        loop_state
            .handle_event(&SessionEvent::AgentPromptDelivered {
                turn_id,
                ts: Utc::now(),
            })
            .await;
        loop_state
            .handle_event(&SessionEvent::CheckpointReached {
                checkpoint_id: CheckpointId(Uuid::new_v4()),
                interrupt: None,
                ts: Utc::now(),
            })
            .await;
        loop_state
            .handle_event(&SessionEvent::TurnCompleted {
                turn_id,
                stop_reason: "end_turn".into(),
                ts: Utc::now(),
            })
            .await;
        assert_eq!(driver.prompts.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn turn_aborted_does_not_prompt() {
        let driver = RecorderDriver::default();
        let mut loop_state = fresh_loop(driver.clone());

        let turn_id = Uuid::new_v4();
        loop_state
            .handle_event(&SessionEvent::AgentPromptDelivered {
                turn_id,
                ts: Utc::now(),
            })
            .await;
        loop_state
            .handle_event(&SessionEvent::TurnAborted {
                turn_id,
                reason: "x".into(),
                ts: Utc::now(),
            })
            .await;
        assert!(driver.prompts.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn drain_interrupt_calls_shutdown() {
        let driver = RecorderDriver::default();
        let mut loop_state = fresh_loop(driver.clone());

        let turn_id = Uuid::new_v4();
        loop_state
            .handle_event(&SessionEvent::AgentPromptDelivered {
                turn_id,
                ts: Utc::now(),
            })
            .await;
        loop_state
            .handle_event(&SessionEvent::CheckpointReached {
                checkpoint_id: CheckpointId(Uuid::new_v4()),
                interrupt: Some(InterruptSnapshot::Drain),
                ts: Utc::now(),
            })
            .await;
        loop_state
            .handle_event(&SessionEvent::TurnCompleted {
                turn_id,
                stop_reason: "end_turn".into(),
                ts: Utc::now(),
            })
            .await;
        assert_eq!(*driver.shutdowns.lock().unwrap(), 1);
        assert!(driver.prompts.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn pause_interrupt_calls_pause() {
        let driver = RecorderDriver::default();
        let mut loop_state = fresh_loop(driver.clone());

        let turn_id = Uuid::new_v4();
        loop_state
            .handle_event(&SessionEvent::AgentPromptDelivered {
                turn_id,
                ts: Utc::now(),
            })
            .await;
        loop_state
            .handle_event(&SessionEvent::CheckpointReached {
                checkpoint_id: CheckpointId(Uuid::new_v4()),
                interrupt: Some(InterruptSnapshot::Pause),
                ts: Utc::now(),
            })
            .await;
        loop_state
            .handle_event(&SessionEvent::TurnCompleted {
                turn_id,
                stop_reason: "end_turn".into(),
                ts: Utc::now(),
            })
            .await;
        assert_eq!(*driver.pauses.lock().unwrap(), 1);
    }

    /// Driver error during auto-prompt transitions the session to `Crashed`
    /// and stops further dispatch.
    #[tokio::test]
    async fn driver_error_transitions_to_crashed() {
        #[derive(Debug, Default)]
        struct FailingDriver;
        impl SessionDriver for FailingDriver {
            async fn send_agent_prompt(&self, _prompt: String) -> Result<(), DriverError> {
                Err(DriverError::ProcessGone)
            }
            async fn pause(&self) -> Result<(), DriverError> {
                Err(DriverError::ProcessGone)
            }
            async fn shutdown(&self) -> Result<(), DriverError> {
                Err(DriverError::ProcessGone)
            }
        }

        let mut loop_state = PerSessionLoop::new(
            AgentId::new(),
            Uuid::new_v4(),
            FailingDriver,
            SupervisionConfig::default(),
        );

        let turn_id = Uuid::new_v4();
        loop_state
            .handle_event(&SessionEvent::AgentPromptDelivered {
                turn_id,
                ts: Utc::now(),
            })
            .await;
        loop_state
            .handle_event(&SessionEvent::CheckpointReached {
                checkpoint_id: CheckpointId(Uuid::new_v4()),
                interrupt: None,
                ts: Utc::now(),
            })
            .await;
        let action = loop_state
            .handle_event(&SessionEvent::TurnCompleted {
                turn_id,
                stop_reason: "end_turn".into(),
                ts: Utc::now(),
            })
            .await;
        assert_eq!(action, SupervisorAction::Crashed);
        assert_eq!(loop_state.phase(), SessionPhase::Crashed);
    }
}

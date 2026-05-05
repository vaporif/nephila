//! `SessionRegistry` — owns one `ClaudeCodeSession` per agent and respawns it
//! after a crash via `ClaudeCodeSession::resume`.
//!
//! Surface:
//!   - `ensure_session(agent)` — start the session, append
//!     `AgentSessionAssigned` to the agent aggregate, install the per-session
//!     crash-watch task, fire `started_tx`.
//!   - `on_crash(agent_id, crash_seq)` — guarded by a per-agent
//!     `Mutex<RespawnState>`; drops the old handle and resumes via
//!     `ClaudeCodeSession::resume`.
//!   - `on_startup()` — scans `list_agents_in_active_phase()` and resumes
//!     each one. Failures transition the agent to `Failed`.
//!
//! Termination contract: the per-session crash-watch task must exit on
//! `SessionCrashed` or `SessionEnded`, or we leak one task per ever-spawned
//! session.

// The orchestrator-driven `spawn` path that calls `ensure_session` is wired in
// from a separate plan; some methods compile but are not yet called from
// production code paths.
#![allow(dead_code)]

use std::sync::Arc;

use chrono::Utc;
use dashmap::DashMap;
use futures::StreamExt;
use nephila_connector::ConnectorError;
use nephila_connector::session::{ClaudeCodeSession, SessionConfig};
use nephila_core::agent::{Agent, AgentConfigSnapshot, AgentEvent};
use nephila_core::id::AgentId;
use nephila_core::session_event::SessionEvent;
#[cfg(test)]
use nephila_core::session_event::SessionId;
use nephila_eventsourcing::envelope::EventEnvelope;
use nephila_eventsourcing::id::{EventId, TraceId};
use nephila_eventsourcing::store::DomainEventStore;
use nephila_store::SqliteStore;
use nephila_store::blob::SqliteBlobReader;
use nephila_store::resilient_subscribe::resilient_subscribe;
use tokio::sync::{Mutex, broadcast, mpsc};
use uuid::Uuid;

const NEW_SESSION_CHANNEL_BOUND: usize = 64;
const CRASH_FALLBACK_CHANNEL_BOUND: usize = 64;

/// Defaults used by `cfg_from(agent)` when an agent predates the
/// `AgentConfigSnapshotted` event (legacy installs).
#[derive(Debug, Clone)]
pub struct RegistryDefaults {
    pub claude_binary: std::path::PathBuf,
    pub mcp_endpoint: String,
    pub permission_mode: String,
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("connector: {0}")]
    Connector(#[from] ConnectorError),
    #[error("store: {0}")]
    Store(#[from] nephila_eventsourcing::store::EventStoreError),
    #[error("store: {0}")]
    StoreOther(String),
}

struct SessionHandle {
    session: Arc<ClaudeCodeSession>,
    /// Per-session crash-watch task. Must exit on `SessionCrashed` /
    /// `SessionEnded`. Aborted unconditionally on respawn / shutdown.
    pump: tokio::task::AbortHandle,
}

/// Crashes that arrive via the fallback channel carry no sequence number
/// (the connector sends only the `AgentId`). To dedupe a connector that
/// sends twice in a row — or a fallback that races a delayed store-path
/// crash — collapse fallback respawns that land within this window.
const FALLBACK_DEDUP_WINDOW: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Default)]
struct RespawnState {
    /// Sequence number of the last crash event handled for this agent.
    /// Duplicates (e.g. from a Lagged-recovery re-subscription) with
    /// `sequence <= last_handled_crash_seq` are dropped without spawning.
    last_handled_crash_seq: Option<u64>,
    /// True between Phase 1 (decision) and Phase 3 (commit) of `on_crash`.
    /// Concurrent callers that observe `true` skip — the in-flight respawn
    /// will install the new handle.
    respawn_in_flight: bool,
    /// Wall-clock instant at which the last respawn attempt was committed
    /// (success or failure). Used to dedupe `crash_seq == 0` (fallback)
    /// replays within `FALLBACK_DEDUP_WINDOW`.
    last_respawn_attempt_at: Option<std::time::Instant>,
    #[cfg(test)]
    respawn_count: u64,
}

pub struct SessionRegistry {
    started_tx: broadcast::Sender<AgentId>,
    sessions: DashMap<AgentId, SessionHandle>,
    store: Arc<SqliteStore>,
    blob_reader: Arc<SqliteBlobReader>,
    /// Per-agent respawn lock. `on_crash` acquires the mutex before
    /// checking-and-spawning, serializing concurrent crash deliveries.
    respawn_states: DashMap<AgentId, Arc<Mutex<RespawnState>>>,
    /// Out-of-band crash-fallback receiver. The connector's reader uses the
    /// `Sender` half (passed via `SessionConfig::crash_fallback_tx`) to signal
    /// crashes when its `[TurnAborted, SessionCrashed]` `append_batch` itself
    /// fails. The receiver is consumed by a long-lived task installed by
    /// `start_crash_fallback_listener`.
    crash_fallback_tx: mpsc::Sender<AgentId>,
    crash_fallback_rx: Mutex<Option<mpsc::Receiver<AgentId>>>,
    /// Defaults used when an agent's `last_config_snapshot` is `None`.
    defaults: RegistryDefaults,
    /// Tracks per-agent UUID for the running session — used to map
    /// `AgentId → SessionId` for resume.
    session_ids: DashMap<AgentId, Uuid>,
    /// Test-only: signals when `on_crash` has dropped the old handle but
    /// has NOT yet inserted the new one. Used by step-6 tests to verify
    /// orphan recovery via `on_startup`.
    #[cfg(test)]
    abort_after_drop_old: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl SessionRegistry {
    #[must_use]
    pub fn new(
        store: Arc<SqliteStore>,
        blob_reader: Arc<SqliteBlobReader>,
        defaults: RegistryDefaults,
    ) -> Self {
        let (started_tx, _) = broadcast::channel(NEW_SESSION_CHANNEL_BOUND);
        let (crash_fallback_tx, crash_fallback_rx) = mpsc::channel(CRASH_FALLBACK_CHANNEL_BOUND);
        Self {
            started_tx,
            sessions: DashMap::new(),
            store,
            blob_reader,
            respawn_states: DashMap::new(),
            crash_fallback_tx,
            crash_fallback_rx: Mutex::new(Some(crash_fallback_rx)),
            defaults,
            session_ids: DashMap::new(),
            #[cfg(test)]
            abort_after_drop_old: Mutex::new(None),
        }
    }

    /// Subscribe to "a new session has started" notifications.
    pub fn subscribe_session_started(&self) -> broadcast::Receiver<AgentId> {
        self.started_tx.subscribe()
    }

    /// Test-only: fire `started_tx` without spawning a session. Production
    /// code goes through `ensure_session`, which fires the broadcast itself.
    #[cfg(test)]
    pub fn fire_started(&self, agent_id: AgentId) {
        let _ = self.started_tx.send(agent_id);
    }

    /// Hand the registry's crash-fallback receiver to a background listener
    /// that re-routes channel sends into `on_crash` calls. Idempotent: only
    /// the first call wires anything; subsequent calls are no-ops.
    ///
    /// Returns the `JoinHandle` of the listener task (so callers can `abort`
    /// it on shutdown).
    pub async fn start_crash_fallback_listener(
        self: Arc<Self>,
    ) -> Option<tokio::task::JoinHandle<()>> {
        let mut guard = self.crash_fallback_rx.lock().await;
        let mut rx = guard.take()?;
        let me = Arc::clone(&self);
        Some(tokio::spawn(async move {
            while let Some(agent_id) = rx.recv().await {
                // Fallback path has no sequence number; pass 0 as a sentinel.
                // `on_crash` recognises `crash_seq == 0` and falls back to a
                // wall-clock dedup window instead of the per-sequence guard.
                me.on_crash(agent_id, 0).await;
            }
        }))
    }

    /// Ensure a session exists for `agent`. Returns the existing `Arc` if one
    /// is already running; otherwise spawns a fresh session via
    /// `ClaudeCodeSession::start`, appends `AgentSessionAssigned` and
    /// `AgentConfigSnapshotted` to the agent aggregate, fires `started_tx`,
    /// and installs the per-session crash-watch task.
    #[tracing::instrument(level = "debug", skip(self, agent), fields(agent_id = %agent.id))]
    pub async fn ensure_session(
        self: &Arc<Self>,
        agent: &Agent,
    ) -> Result<Arc<ClaudeCodeSession>, RegistryError> {
        if let Some(existing) = self.sessions.get(&agent.id) {
            return Ok(Arc::clone(&existing.session));
        }

        let cfg = self.cfg_from(agent);
        let session_id = cfg.session_id;
        let session = ClaudeCodeSession::start(cfg).await?;
        let session_arc = Arc::new(session);

        self.session_ids.insert(agent.id, session_id);

        // Append `AgentSessionAssigned` + `AgentConfigSnapshotted` to the
        // agent aggregate so post-restart resume can find this binding.
        let snap = AgentConfigSnapshot {
            working_dir: agent.directory.clone(),
            mcp_endpoint: self.defaults.mcp_endpoint.clone(),
            permission_mode: self.defaults.permission_mode.clone(),
            claude_binary: self.defaults.claude_binary.clone(),
        };
        let now = Utc::now();
        let assigned = AgentEvent::AgentSessionAssigned {
            agent_id: agent.id,
            session_id,
            ts: now,
        };
        let snapshotted = AgentEvent::AgentConfigSnapshotted {
            agent_id: agent.id,
            snapshot: snap,
            ts: now,
        };
        if let Err(e) = self
            .append_agent_events(agent.id, &[assigned, snapshotted])
            .await
        {
            tracing::warn!(%agent.id, %e, "failed to persist agent assignment events");
        }

        let pump = self.spawn_crash_watch(agent.id, session_id);
        self.sessions.insert(
            agent.id,
            SessionHandle {
                session: Arc::clone(&session_arc),
                pump,
            },
        );

        let _ = self.started_tx.send(agent.id);
        Ok(session_arc)
    }

    /// Build a `SessionConfig` from `agent.last_config_snapshot`, falling back
    /// to registry defaults (with a warn log) when the snapshot is absent.
    fn cfg_from(&self, agent: &Agent) -> SessionConfig {
        let (claude_binary, mcp_endpoint, permission_mode, working_dir) =
            if let Some(snap) = &agent.last_config_snapshot {
                (
                    snap.claude_binary.clone(),
                    snap.mcp_endpoint.clone(),
                    snap.permission_mode.clone(),
                    snap.working_dir.clone(),
                )
            } else {
                tracing::warn!(
                    %agent.id,
                    "agent has no last_config_snapshot; falling back to registry defaults",
                );
                (
                    self.defaults.claude_binary.clone(),
                    self.defaults.mcp_endpoint.clone(),
                    self.defaults.permission_mode.clone(),
                    agent.directory.clone(),
                )
            };

        let session_id = agent
            .session_id
            .as_deref()
            .and_then(|s| Uuid::parse_str(s).ok())
            .unwrap_or_else(Uuid::new_v4);

        SessionConfig {
            claude_binary,
            session_id,
            agent_id: agent.id,
            working_dir,
            mcp_endpoint,
            permission_mode,
            store: Arc::clone(&self.store),
            blob_reader: Arc::clone(&self.blob_reader),
            crash_fallback_tx: Some(self.crash_fallback_tx.clone()),
        }
    }

    /// Per-session crash-watch task. Subscribes to the session aggregate from
    /// sequence 0, breaks on `SessionCrashed` (calling `on_crash`) or
    /// `SessionEnded` (removing the entry).
    pub(crate) fn spawn_crash_watch(
        self: &Arc<Self>,
        agent_id: AgentId,
        session_id: Uuid,
    ) -> tokio::task::AbortHandle {
        let me = Arc::clone(self);
        let agg_id = session_id.to_string();
        let handle = tokio::spawn(async move {
            let mut stream = Box::pin(resilient_subscribe(
                Arc::clone(&me.store),
                "session".to_owned(),
                agg_id,
                0,
            ));

            while let Some(item) = stream.next().await {
                let env = match item {
                    Ok(e) => e,
                    Err(nephila_eventsourcing::store::EventStoreError::PersistentLag(n)) => {
                        tracing::error!(
                            %agent_id, %session_id, retries = n,
                            "crash-watch persistent lag; relying on connector fallback",
                        );
                        return;
                    }
                    Err(e) => {
                        tracing::warn!(%agent_id, %session_id, %e, "crash-watch stream error");
                        continue;
                    }
                };
                let ev: SessionEvent = match serde_json::from_value(env.payload.clone()) {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                match ev {
                    SessionEvent::SessionCrashed { .. } => {
                        me.on_crash(agent_id, env.sequence()).await;
                        break;
                    }
                    SessionEvent::SessionEnded { .. } => {
                        me.sessions.remove(&agent_id);
                        me.session_ids.remove(&agent_id);
                        break;
                    }
                    _ => {}
                }
            }
        });
        handle.abort_handle()
    }

    /// Handle a crash — drop the old handle, resume via
    /// `ClaudeCodeSession::resume`, install the new handle.
    ///
    /// Dedup runs in three phases so the per-agent `RespawnState` mutex is
    /// NOT held across `resume().await`. Two concurrent callers (e.g. the
    /// store-path watcher and the fallback channel listener) racing on this
    /// agent will both reach Phase 1, but only the first observes
    /// `respawn_in_flight = false` and proceeds; the second skips.
    #[tracing::instrument(level = "debug", skip(self), fields(%agent_id, crash_seq))]
    pub async fn on_crash(self: &Arc<Self>, agent_id: AgentId, crash_seq: u64) {
        let lock = self
            .respawn_states
            .entry(agent_id)
            .or_insert_with(|| Arc::new(Mutex::new(RespawnState::default())))
            .clone();

        // Phase 1: dedup decision — atomic with setting `respawn_in_flight`.
        let mut state = lock.lock().await;
        if state.respawn_in_flight {
            tracing::debug!(%agent_id, crash_seq, "respawn already in flight; skipping");
            return;
        }
        if crash_seq != 0
            && let Some(prev) = state.last_handled_crash_seq
            && crash_seq <= prev
        {
            tracing::debug!(%agent_id, crash_seq, prev, "duplicate crash sequence; skipping");
            return;
        }
        if crash_seq == 0
            && let Some(last) = state.last_respawn_attempt_at
            && last.elapsed() < FALLBACK_DEDUP_WINDOW
        {
            tracing::debug!(
                %agent_id,
                last_respawn_ago_ms = ?last.elapsed(),
                "fallback within dedup window; skipping",
            );
            return;
        }
        state.respawn_in_flight = true;
        #[cfg(test)]
        {
            state.respawn_count += 1;
        }
        drop(state);

        // Phase 2: do the work without holding the lock.
        let result = self.do_respawn_work(agent_id).await;

        // Phase 3: commit + clear in-flight, regardless of result.
        // `last_respawn_attempt_at` is set on every attempt — the fallback
        // dedup window also gates failed attempts so a flapping respawn
        // doesn't spin. `last_handled_crash_seq` only advances on success,
        // so a failed store-path respawn can be retried at the next sequence.
        let mut state = lock.lock().await;
        state.respawn_in_flight = false;
        state.last_respawn_attempt_at = Some(std::time::Instant::now());
        match result {
            Ok(()) => {
                state.last_handled_crash_seq = Some(crash_seq);
            }
            Err(e) => {
                tracing::error!(%agent_id, %e, "respawn failed");
            }
        }
    }

    /// Inner respawn work — drop the old handle, resume, install the new
    /// handle. Holds NO `RespawnState` lock; the caller manages the
    /// in-flight flag.
    async fn do_respawn_work(self: &Arc<Self>, agent_id: AgentId) -> Result<(), RegistryError> {
        if let Some((_id, old)) = self.sessions.remove(&agent_id) {
            old.pump.abort();
            drop(old.session);
        }

        #[cfg(test)]
        {
            let mut tx_guard = self.abort_after_drop_old.lock().await;
            if let Some(tx) = tx_guard.take() {
                drop(tx_guard);
                // Signal the test that the drop happened, then park forever —
                // simulating a process abort between drop and insert.
                let _ = tx.send(());
                let (_never_tx, never_rx) = tokio::sync::oneshot::channel::<()>();
                let _ = never_rx.await;
                return Err(RegistryError::StoreOther("test-only abort path".into()));
            }
        }

        let session_id = self
            .session_ids
            .get(&agent_id)
            .map(|r| *r.value())
            .ok_or_else(|| RegistryError::StoreOther("no session_id known".into()))?;

        let agent = match nephila_core::store::AgentStore::get(&*self.store, agent_id).await {
            Ok(Some(a)) => a,
            Ok(None) => return Err(RegistryError::StoreOther("agent not found".into())),
            Err(e) => return Err(RegistryError::StoreOther(e.to_string())),
        };

        let cfg = self.cfg_from(&agent);
        let session = ClaudeCodeSession::resume(cfg, session_id).await?;
        let session_arc = Arc::new(session);
        let pump = self.spawn_crash_watch(agent_id, session_id);
        self.sessions.insert(
            agent_id,
            SessionHandle {
                session: Arc::clone(&session_arc),
                pump,
            },
        );
        nephila_store::metrics::record_session_respawn(&session_id.to_string());
        tracing::info!(%agent_id, %session_id, "session respawned after crash");
        let _ = self.started_tx.send(agent_id);
        Ok(())
    }

    /// Scan `list_agents_in_active_phase` and resume each agent that has a
    /// known `session_id`. Failures transition the agent to `Failed` via the
    /// agent reducer.
    #[tracing::instrument(level = "debug", skip(self))]
    pub async fn on_startup(self: &Arc<Self>) -> Result<(), RegistryError> {
        let agents = self
            .store
            .list_agents_in_active_phase()
            .await
            .map_err(|e| RegistryError::StoreOther(e.to_string()))?;

        for agent in agents {
            let Some(session_id_str) = agent.session_id.as_deref() else {
                tracing::debug!(%agent.id, "active agent has no session_id; skipping resume");
                continue;
            };
            let Ok(session_id) = Uuid::parse_str(session_id_str) else {
                tracing::warn!(%agent.id, %session_id_str, "session_id is not a uuid; skipping");
                continue;
            };

            self.session_ids.insert(agent.id, session_id);
            let cfg = self.cfg_from(&agent);
            match ClaudeCodeSession::resume(cfg, session_id).await {
                Ok(session) => {
                    let session_arc = Arc::new(session);
                    let pump = self.spawn_crash_watch(agent.id, session_id);
                    self.sessions.insert(
                        agent.id,
                        SessionHandle {
                            session: Arc::clone(&session_arc),
                            pump,
                        },
                    );
                    let _ = self.started_tx.send(agent.id);
                }
                Err(e) => {
                    tracing::error!(%agent.id, %session_id, %e, "on_startup resume failed; marking agent failed");
                    if let Err(e2) = self
                        .mark_agent_failed(&agent, "session resume failed during on_startup")
                        .await
                    {
                        tracing::error!(%agent.id, %e2, "failed to mark agent as Failed");
                    }
                }
            }
        }

        Ok(())
    }

    /// Transition an agent to `Failed` through the event-sourced path:
    /// `AgentCommand::Fail` produces `AgentEvent::StateChanged` (and a
    /// `DirectiveChanged` companion in some arms); we append those events to
    /// the event log AND persist the resulting projection via `AgentStore::save`
    /// so the SQL row and event log stay in sync.
    async fn mark_agent_failed(&self, agent: &Agent, reason: &str) -> Result<(), RegistryError> {
        let events = agent
            .handle(nephila_core::agent::AgentCommand::Fail {
                reason: reason.to_owned(),
            })
            .map_err(|e| RegistryError::StoreOther(format!("agent.handle(Fail): {e}")))?;
        self.append_agent_events(agent.id, &events).await?;
        let projected = events.iter().fold(agent.clone(), Agent::apply_event);
        nephila_core::store::AgentStore::save(&*self.store, &projected)
            .await
            .map_err(|e| RegistryError::StoreOther(format!("AgentStore::save: {e}")))?;
        Ok(())
    }

    /// Append `AgentEvent`s to the agent aggregate's event stream.
    async fn append_agent_events(
        &self,
        agent_id: AgentId,
        events: &[AgentEvent],
    ) -> Result<(), nephila_eventsourcing::store::EventStoreError> {
        let envelopes: Vec<EventEnvelope> = events
            .iter()
            .map(|e| {
                let payload =
                    serde_json::to_value(e).expect("AgentEvent serializes to JSON infallibly");
                EventEnvelope::new(nephila_eventsourcing::envelope::NewEventEnvelope {
                    id: EventId::new(),
                    aggregate_type: "agent".to_owned(),
                    aggregate_id: agent_id.to_string(),
                    event_type: e.kind().to_owned(),
                    payload,
                    trace_id: TraceId(agent_id.to_string()),
                    outcome: None,
                    timestamp: Utc::now(),
                    context_snapshot: None,
                    metadata: std::collections::HashMap::new(),
                })
            })
            .collect();
        self.store.append_batch(envelopes).await.map(|_| ())
    }

    #[cfg(test)]
    pub async fn install_abort_after_drop_old(&self, tx: tokio::sync::oneshot::Sender<()>) {
        *self.abort_after_drop_old.lock().await = Some(tx);
    }

    /// Test-only accessor for the session count.
    #[must_use]
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Test-only accessor for whether a given agent has a live handle.
    #[must_use]
    pub fn has_session(&self, agent_id: AgentId) -> bool {
        self.sessions.contains_key(&agent_id)
    }

    /// Test seam: bind a session_id for an agent without spawning a process.
    #[cfg(test)]
    pub fn bind_session_id_for_test(&self, agent_id: AgentId, session_id: SessionId) {
        self.session_ids.insert(agent_id, session_id);
    }

    /// Test seam: pre-create the per-agent `RespawnState` entry so that
    /// concurrent `on_crash` callers race the same mutex.
    #[cfg(test)]
    pub fn materialize_respawn_state_for_test(self: &Arc<Self>, agent_id: AgentId) {
        let _ = self
            .respawn_states
            .entry(agent_id)
            .or_insert_with(|| Arc::new(Mutex::new(RespawnState::default())))
            .clone();
    }

    /// Test seam: read the count of respawn decisions made for an agent.
    #[cfg(test)]
    pub async fn respawn_count_for_test(&self, agent_id: AgentId) -> u64 {
        let Some(lock) = self.respawn_states.get(&agent_id).map(|r| r.clone()) else {
            return 0;
        };
        lock.lock().await.respawn_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> RegistryDefaults {
        RegistryDefaults {
            claude_binary: std::path::PathBuf::from("/usr/bin/false"),
            mcp_endpoint: "http://stub".into(),
            permission_mode: "bypassPermissions".into(),
        }
    }

    #[tokio::test]
    async fn fire_then_recv_returns_agent_id() {
        let store = Arc::new(SqliteStore::open_in_memory(384).unwrap());
        let blob = Arc::new(SqliteBlobReader::new(store.read_pool()));
        let reg = SessionRegistry::new(store, blob, defaults());
        let mut rx = reg.subscribe_session_started();
        let agent = AgentId::new();
        reg.fire_started(agent);
        assert_eq!(rx.recv().await.expect("recv"), agent);
    }

    #[tokio::test]
    async fn registry_starts_empty() {
        let store = Arc::new(SqliteStore::open_in_memory(384).unwrap());
        let blob = Arc::new(SqliteBlobReader::new(store.read_pool()));
        let reg = SessionRegistry::new(store, blob, defaults());
        assert_eq!(reg.session_count(), 0);
    }

    /// `mark_agent_failed` writes through events: both the agent event log
    /// and the SQL projection reflect `Failed`. Verifies `on_startup`'s
    /// failure branch is event-sourced.
    #[tokio::test]
    async fn mark_agent_failed_appends_event_and_updates_projection() {
        use nephila_core::ObjectiveId;
        use nephila_core::agent::{AgentCommand, AgentState, SpawnOrigin};
        use nephila_core::store::AgentStore;

        let store = Arc::new(SqliteStore::open_in_memory(384).unwrap());
        let blob = Arc::new(SqliteBlobReader::new(store.read_pool()));
        let reg = SessionRegistry::new(store.clone(), blob, defaults());

        // Build an Active agent (agent must not be terminal for `Fail` to apply).
        let mut agent = Agent::new(
            AgentId::new(),
            ObjectiveId::new(),
            std::path::PathBuf::from("/tmp"),
            SpawnOrigin::Operator,
            None,
        );
        let activate_events = agent.handle(AgentCommand::Activate).unwrap();
        agent = activate_events.iter().fold(agent, Agent::apply_event);
        store.register(agent.clone()).await.unwrap();
        AgentStore::update_state(&*store, agent.id, AgentState::Active)
            .await
            .unwrap();

        reg.mark_agent_failed(&agent, "test")
            .await
            .expect("mark_agent_failed");

        // Projection reflects Failed.
        let after = store.get(agent.id).await.unwrap().unwrap();
        assert_eq!(after.state, AgentState::Failed);

        // Event log contains a StateChanged event with new_state = Failed.
        let events = store
            .load_events("agent", &agent.id.to_string(), 0)
            .await
            .unwrap();
        let saw_failed = events.iter().any(|env| {
            serde_json::from_value::<AgentEvent>(env.payload.clone())
                .ok()
                .is_some_and(|ev| {
                    matches!(
                        ev,
                        AgentEvent::StateChanged {
                            new_state: AgentState::Failed,
                            ..
                        }
                    )
                })
        });
        assert!(
            saw_failed,
            "expected StateChanged(new_state=Failed) in event log; got {events:?}",
        );
    }

    /// Termination contract: the per-session crash-watch task MUST exit when
    /// it observes `SessionEnded`. Without this, the registry leaks one task
    /// per ever-spawned session.
    #[tokio::test]
    async fn crash_watch_terminates_on_session_ended() {
        use nephila_eventsourcing::envelope::EventEnvelope;
        use nephila_eventsourcing::id::{EventId, TraceId};

        let store = Arc::new(SqliteStore::open_in_memory(384).unwrap());
        let blob = Arc::new(SqliteBlobReader::new(store.read_pool()));
        let reg = Arc::new(SessionRegistry::new(store.clone(), blob, defaults()));

        let agent_id = AgentId::new();
        let session_id = Uuid::new_v4();

        let abort = reg.spawn_crash_watch(agent_id, session_id);

        // Append SessionEnded; the watcher must break out.
        let ev = SessionEvent::SessionEnded {
            ts: chrono::Utc::now(),
        };
        let env = EventEnvelope::new(nephila_eventsourcing::envelope::NewEventEnvelope {
            id: EventId::new(),
            aggregate_type: "session".to_owned(),
            aggregate_id: session_id.to_string(),
            event_type: "session_ended".to_owned(),
            payload: serde_json::to_value(&ev).unwrap(),
            trace_id: TraceId(session_id.to_string()),
            outcome: None,
            timestamp: chrono::Utc::now(),
            context_snapshot: None,
            metadata: std::collections::HashMap::new(),
        });
        store.append_batch(vec![env]).await.unwrap();

        // Wait up to 2s for the watcher to exit.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            if abort.is_finished() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("crash-watch task did not terminate on SessionEnded");
    }

    /// Termination contract: the per-session crash-watch task MUST exit when
    /// it observes `SessionCrashed`.
    #[tokio::test]
    async fn crash_watch_terminates_on_session_crashed() {
        use nephila_eventsourcing::envelope::EventEnvelope;
        use nephila_eventsourcing::id::{EventId, TraceId};

        let store = Arc::new(SqliteStore::open_in_memory(384).unwrap());
        let blob = Arc::new(SqliteBlobReader::new(store.read_pool()));
        let reg = Arc::new(SessionRegistry::new(store.clone(), blob, defaults()));

        let agent_id = AgentId::new();
        let session_id = Uuid::new_v4();
        // Bind so on_crash has a session_id, but agent isn't in store — so
        // the on_crash respawn attempt will give up early.
        reg.bind_session_id_for_test(agent_id, session_id);

        let abort = reg.spawn_crash_watch(agent_id, session_id);

        let ev = SessionEvent::SessionCrashed {
            reason: "test".into(),
            exit_code: Some(1),
            ts: chrono::Utc::now(),
        };
        let env = EventEnvelope::new(nephila_eventsourcing::envelope::NewEventEnvelope {
            id: EventId::new(),
            aggregate_type: "session".to_owned(),
            aggregate_id: session_id.to_string(),
            event_type: "session_crashed".to_owned(),
            payload: serde_json::to_value(&ev).unwrap(),
            trace_id: TraceId(session_id.to_string()),
            outcome: None,
            timestamp: chrono::Utc::now(),
            context_snapshot: None,
            metadata: std::collections::HashMap::new(),
        });
        store.append_batch(vec![env]).await.unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            if abort.is_finished() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("crash-watch task did not terminate on SessionCrashed");
    }
}

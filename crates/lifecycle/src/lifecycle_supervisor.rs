use std::collections::HashMap;
use std::sync::Arc;

use nephila_core::agent::AgentState;
use nephila_core::command::OrchestratorCommand;
use nephila_core::config::{LifecycleConfig, SupervisionConfig};
use nephila_core::directive::Directive;
use nephila_core::event::BusEvent;
use nephila_core::id::{AgentId, ObjectiveId};
use nephila_core::store::AgentStore;
use nephila_store::SqliteStore;
use tokio::sync::{broadcast, mpsc};

use crate::supervisor::RestartTracker;
use crate::token_tracker::TokenTracker;

/// Compose the spawn-time prompt for an agent.
///
/// Substitutes the placeholders `{{agent_id}}`, `{{objective_id}}`, and
/// `{{mcp_endpoint}}` in `template`, then appends a `# Your Task` section
/// containing `objective_content`. Lives here (rather than in
/// `bin/src/orchestrator.rs`) so the supervisor can re-use it when seeding
/// a respawned session.
#[must_use]
pub fn compose_next_prompt(
    template: &str,
    agent_id: AgentId,
    objective_id: ObjectiveId,
    mcp_endpoint: &str,
    objective_content: &str,
) -> String {
    let protocol = template
        .replace("{{agent_id}}", &agent_id.to_string())
        .replace("{{objective_id}}", &objective_id.to_string())
        .replace("{{mcp_endpoint}}", mcp_endpoint);

    format!("{protocol}\n\n---\n\n# Your Task\n\n{objective_content}")
}

pub struct LifecycleSupervisor {
    token_trackers: HashMap<AgentId, TokenTracker>,
    restart_trackers: HashMap<ObjectiveId, RestartTracker>,
    store: Arc<SqliteStore>,
    event_rx: broadcast::Receiver<BusEvent>,
    cmd_tx: mpsc::Sender<OrchestratorCommand>,
    lifecycle_config: LifecycleConfig,
    supervision_config: SupervisionConfig,
}

impl LifecycleSupervisor {
    pub fn new(
        event_rx: broadcast::Receiver<BusEvent>,
        cmd_tx: mpsc::Sender<OrchestratorCommand>,
        store: Arc<SqliteStore>,
        lifecycle_config: LifecycleConfig,
        supervision_config: SupervisionConfig,
    ) -> Self {
        Self {
            token_trackers: HashMap::new(),
            restart_trackers: HashMap::new(),
            store,
            event_rx,
            cmd_tx,
            lifecycle_config,
            supervision_config,
        }
    }

    pub fn register_agent(&mut self, agent_id: AgentId) {
        self.token_trackers
            .insert(agent_id, TokenTracker::new(self.lifecycle_config));
    }

    pub async fn handle_token_report(&mut self, agent_id: AgentId, used: u64, remaining: u64) {
        let tracker = self
            .token_trackers
            .entry(agent_id)
            .or_insert_with(|| TokenTracker::new(self.lifecycle_config));

        tracker.report(used, remaining);

        if tracker.should_force_kill() {
            let _ = self
                .cmd_tx
                .send(OrchestratorCommand::TokenThreshold {
                    agent_id,
                    directive: Directive::Abort,
                })
                .await;
        } else if tracker.should_prepare_reset() {
            tracker.mark_drain_started();
            let _ = self
                .cmd_tx
                .send(OrchestratorCommand::TokenThreshold {
                    agent_id,
                    directive: Directive::PrepareReset,
                })
                .await;
        }
    }

    pub async fn handle_agent_exited(&mut self, agent_id: AgentId) {
        self.token_trackers.remove(&agent_id);

        let agent = match self.store.get(agent_id).await {
            Ok(Some(a)) => a,
            Ok(None) => {
                tracing::warn!(%agent_id, "exited agent not found in store");
                return;
            }
            Err(e) => {
                tracing::error!(%agent_id, %e, "failed to load agent for respawn check");
                return;
            }
        };

        let checkpoint_id = match agent.checkpoint_id {
            Some(id) => id,
            None => return,
        };

        let tracker = self
            .restart_trackers
            .entry(agent.objective_id)
            .or_insert_with(|| RestartTracker::new(self.supervision_config.clone()));

        if !tracker.record_restart() {
            tracing::warn!(
                %agent_id,
                objective_id = %agent.objective_id,
                "restart limit exceeded, not respawning"
            );
            return;
        }

        let content = agent.injected_message.unwrap_or_default();

        let _ = self
            .cmd_tx
            .send(OrchestratorCommand::Respawn {
                objective_id: agent.objective_id,
                content,
                dir: agent.directory,
                restore_checkpoint_id: checkpoint_id,
            })
            .await;
    }

    pub async fn handle_agent_session_ready(&mut self, agent_id: AgentId) {
        self.register_agent(agent_id);
    }

    pub async fn run(&mut self) {
        loop {
            match self.event_rx.recv().await {
                Ok(event) => match event {
                    BusEvent::TokenReport {
                        agent_id,
                        used,
                        remaining,
                    } => {
                        self.handle_token_report(agent_id, used, remaining).await;
                    }
                    BusEvent::AgentStateChanged {
                        agent_id,
                        new_state: AgentState::Exited,
                        ..
                    } => {
                        self.handle_agent_exited(agent_id).await;
                    }
                    BusEvent::AgentSessionReady { agent_id, .. } => {
                        self.handle_agent_session_ready(agent_id).await;
                    }
                    BusEvent::Shutdown => {
                        tracing::info!("lifecycle supervisor shutting down");
                        break;
                    }
                    _ => {}
                },
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(n, "lifecycle supervisor lagged, missed events");
                }
                Err(broadcast::error::RecvError::Closed) => {
                    tracing::info!("event bus closed, supervisor exiting");
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use nephila_core::checkpoint::CheckpointNode;
    use nephila_core::config::{LifecycleConfig, SupervisionConfig};
    use nephila_core::id::{AgentId, CheckpointId};
    use nephila_core::store::AgentStore;

    #[test]
    fn compose_next_prompt_interpolates_placeholders() {
        let agent_id = AgentId::new();
        let objective_id = ObjectiveId::new();
        let result = compose_next_prompt(
            "Agent {{agent_id}} on {{objective_id}} at {{mcp_endpoint}}",
            agent_id,
            objective_id,
            "http://localhost:8080/mcp",
            "do the thing",
        );
        assert!(result.contains(&agent_id.to_string()));
        assert!(result.contains(&objective_id.to_string()));
        assert!(result.contains("http://localhost:8080/mcp"));
        assert!(result.contains("do the thing"));
    }

    #[test]
    fn compose_next_prompt_appends_task_section() {
        let result = compose_next_prompt(
            "template",
            AgentId::new(),
            ObjectiveId::new(),
            "http://localhost/mcp",
            "my task",
        );
        assert!(result.contains("# Your Task"));
        assert!(result.ends_with("my task"));
    }

    fn test_lifecycle_config() -> LifecycleConfig {
        LifecycleConfig {
            context_threshold_pct: 80,
            context_window_size: 200_000,
            token_warn_pct: 60,
            token_critical_pct: 75,
            token_force_kill_pct: 85,
            token_report_interval_normal: 10,
            token_report_interval_warn: 3,
            token_report_interval_critical: 1,
            hang_timeout_secs: 300,
            drain_timeout_secs: 60,
        }
    }

    async fn register_agent_with_checkpoint(
        store: &SqliteStore,
        agent_id: AgentId,
        objective_id: ObjectiveId,
        dir: &std::path::Path,
        msg: Option<&str>,
    ) -> CheckpointId {
        let agent = nephila_core::agent::Agent::new(
            agent_id,
            objective_id,
            dir.to_path_buf(),
            nephila_core::agent::SpawnOrigin::Operator,
            msg.map(String::from),
        );
        store.register(agent).await.unwrap();

        let cp_id = CheckpointId::new();
        let node = CheckpointNode {
            id: cp_id,
            agent_id,
            parent_id: None,
            branch_label: None,
            channels: BTreeMap::new(),
            l2_namespace: "test".into(),
            interrupt: None,
            created_at: chrono::Utc::now(),
        };
        store.save_checkpoint_metadata(&node).await.unwrap();
        store.set_checkpoint_id(agent_id, cp_id).await.unwrap();

        cp_id
    }

    fn test_supervision_config() -> SupervisionConfig {
        SupervisionConfig {
            default_strategy: "one_for_one".into(),
            max_restarts: 5,
            restart_window_secs: 600,
            max_agent_depth: 3,
        }
    }

    #[tokio::test]
    async fn token_report_above_threshold_sends_prepare_reset() {
        let (event_tx, _) = broadcast::channel(16);
        let (cmd_tx, mut cmd_rx) = mpsc::channel(16);
        let store = Arc::new(nephila_store::SqliteStore::open_in_memory(384).unwrap());

        let mut supervisor = LifecycleSupervisor::new(
            event_tx.subscribe(),
            cmd_tx,
            store,
            test_lifecycle_config(),
            test_supervision_config(),
        );

        let agent_id = AgentId::new();
        supervisor.register_agent(agent_id);

        supervisor
            .handle_token_report(agent_id, 82_000, 18_000)
            .await;

        let cmd = cmd_rx.try_recv().expect("should have sent command");
        assert!(matches!(
            cmd,
            OrchestratorCommand::TokenThreshold {
                directive: Directive::PrepareReset,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn token_report_above_force_kill_sends_abort() {
        let (event_tx, _) = broadcast::channel(16);
        let (cmd_tx, mut cmd_rx) = mpsc::channel(16);
        let store = Arc::new(nephila_store::SqliteStore::open_in_memory(384).unwrap());

        let mut supervisor = LifecycleSupervisor::new(
            event_tx.subscribe(),
            cmd_tx,
            store,
            test_lifecycle_config(),
            test_supervision_config(),
        );

        let agent_id = AgentId::new();
        supervisor.register_agent(agent_id);

        supervisor
            .handle_token_report(agent_id, 90_000, 10_000)
            .await;

        let cmd = cmd_rx.try_recv().expect("should have sent command");
        assert!(matches!(
            cmd,
            OrchestratorCommand::TokenThreshold {
                directive: Directive::Abort,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn token_report_below_threshold_sends_nothing() {
        let (event_tx, _) = broadcast::channel(16);
        let (cmd_tx, mut cmd_rx) = mpsc::channel(16);
        let store = Arc::new(nephila_store::SqliteStore::open_in_memory(384).unwrap());

        let mut supervisor = LifecycleSupervisor::new(
            event_tx.subscribe(),
            cmd_tx,
            store,
            test_lifecycle_config(),
            test_supervision_config(),
        );

        let agent_id = AgentId::new();
        supervisor.register_agent(agent_id);

        supervisor
            .handle_token_report(agent_id, 50_000, 50_000)
            .await;

        assert!(
            cmd_rx.try_recv().is_err(),
            "should not send command below threshold"
        );
    }

    #[tokio::test]
    async fn agent_exit_with_checkpoint_sends_respawn() {
        let (event_tx, _) = broadcast::channel(16);
        let (cmd_tx, mut cmd_rx) = mpsc::channel(16);
        let store = Arc::new(nephila_store::SqliteStore::open_in_memory(384).unwrap());

        let agent_id = AgentId::new();
        let objective_id = ObjectiveId::new();
        let dir = std::path::PathBuf::from("/tmp/test");

        let checkpoint_id = register_agent_with_checkpoint(
            &store,
            agent_id,
            objective_id,
            &dir,
            Some("test objective"),
        )
        .await;

        let mut supervisor = LifecycleSupervisor::new(
            event_tx.subscribe(),
            cmd_tx,
            store,
            test_lifecycle_config(),
            test_supervision_config(),
        );

        supervisor.handle_agent_exited(agent_id).await;

        let cmd = cmd_rx.try_recv().expect("should have sent respawn");
        match cmd {
            OrchestratorCommand::Respawn {
                objective_id: oid,
                restore_checkpoint_id: cpid,
                ..
            } => {
                assert_eq!(oid, objective_id);
                assert_eq!(cpid, checkpoint_id);
            }
            other => panic!("expected Respawn, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn agent_exit_without_checkpoint_does_not_respawn() {
        let (event_tx, _) = broadcast::channel(16);
        let (cmd_tx, mut cmd_rx) = mpsc::channel(16);
        let store = Arc::new(nephila_store::SqliteStore::open_in_memory(384).unwrap());

        let agent_id = AgentId::new();
        let objective_id = ObjectiveId::new();
        let dir = std::path::PathBuf::from("/tmp/test");

        let agent = nephila_core::agent::Agent::new(
            agent_id,
            objective_id,
            dir.clone(),
            nephila_core::agent::SpawnOrigin::Operator,
            Some("test objective".into()),
        );
        store.register(agent).await.unwrap();

        let mut supervisor = LifecycleSupervisor::new(
            event_tx.subscribe(),
            cmd_tx,
            store,
            test_lifecycle_config(),
            test_supervision_config(),
        );

        supervisor.handle_agent_exited(agent_id).await;

        assert!(
            cmd_rx.try_recv().is_err(),
            "should not respawn without checkpoint"
        );
    }

    #[tokio::test]
    async fn register_agent_creates_token_tracker() {
        let (event_tx, _) = broadcast::channel(16);
        let (cmd_tx, _) = mpsc::channel(16);
        let store = Arc::new(nephila_store::SqliteStore::open_in_memory(384).unwrap());

        let mut supervisor = LifecycleSupervisor::new(
            event_tx.subscribe(),
            cmd_tx,
            store,
            test_lifecycle_config(),
            test_supervision_config(),
        );

        let agent_id = AgentId::new();
        assert!(!supervisor.token_trackers.contains_key(&agent_id));
        supervisor.register_agent(agent_id);
        assert!(supervisor.token_trackers.contains_key(&agent_id));
    }

    #[tokio::test]
    async fn handle_agent_session_ready_registers_tracker() {
        let (event_tx, _) = broadcast::channel(16);
        let (cmd_tx, _) = mpsc::channel(16);
        let store = Arc::new(nephila_store::SqliteStore::open_in_memory(384).unwrap());

        let mut supervisor = LifecycleSupervisor::new(
            event_tx.subscribe(),
            cmd_tx,
            store,
            test_lifecycle_config(),
            test_supervision_config(),
        );

        let agent_id = AgentId::new();
        supervisor.handle_agent_session_ready(agent_id).await;
        assert!(supervisor.token_trackers.contains_key(&agent_id));
    }

    #[tokio::test]
    async fn agent_exit_cleans_up_token_tracker() {
        let (event_tx, _) = broadcast::channel(16);
        let (cmd_tx, _cmd_rx) = mpsc::channel(16);
        let store = Arc::new(nephila_store::SqliteStore::open_in_memory(384).unwrap());

        let agent_id = AgentId::new();
        let objective_id = ObjectiveId::new();
        let dir = std::path::PathBuf::from("/tmp/test");

        let agent = nephila_core::agent::Agent::new(
            agent_id,
            objective_id,
            dir,
            nephila_core::agent::SpawnOrigin::Operator,
            None,
        );
        store.register(agent).await.unwrap();

        let mut supervisor = LifecycleSupervisor::new(
            event_tx.subscribe(),
            cmd_tx,
            store,
            test_lifecycle_config(),
            test_supervision_config(),
        );
        supervisor.register_agent(agent_id);
        assert!(supervisor.token_trackers.contains_key(&agent_id));

        supervisor.handle_agent_exited(agent_id).await;
        assert!(!supervisor.token_trackers.contains_key(&agent_id));
    }

    #[tokio::test]
    async fn agent_exit_unknown_agent_does_not_panic() {
        let (event_tx, _) = broadcast::channel(16);
        let (cmd_tx, mut cmd_rx) = mpsc::channel(16);
        let store = Arc::new(nephila_store::SqliteStore::open_in_memory(384).unwrap());

        let mut supervisor = LifecycleSupervisor::new(
            event_tx.subscribe(),
            cmd_tx,
            store,
            test_lifecycle_config(),
            test_supervision_config(),
        );

        supervisor.handle_agent_exited(AgentId::new()).await;
        assert!(cmd_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn run_exits_on_shutdown_event() {
        let (event_tx, _) = broadcast::channel(16);
        let (cmd_tx, _) = mpsc::channel(16);
        let store = Arc::new(nephila_store::SqliteStore::open_in_memory(384).unwrap());

        let mut supervisor = LifecycleSupervisor::new(
            event_tx.subscribe(),
            cmd_tx,
            store,
            test_lifecycle_config(),
            test_supervision_config(),
        );

        event_tx.send(BusEvent::Shutdown).unwrap();
        supervisor.run().await;
        // If we reach here, the run loop correctly exited on Shutdown
    }

    #[tokio::test]
    async fn run_exits_on_channel_close() {
        let (event_tx, _) = broadcast::channel(16);
        let (cmd_tx, _) = mpsc::channel(16);
        let store = Arc::new(nephila_store::SqliteStore::open_in_memory(384).unwrap());

        let mut supervisor = LifecycleSupervisor::new(
            event_tx.subscribe(),
            cmd_tx,
            store,
            test_lifecycle_config(),
            test_supervision_config(),
        );

        drop(event_tx);
        supervisor.run().await;
    }

    #[tokio::test]
    async fn restart_limit_prevents_respawn() {
        let (event_tx, _) = broadcast::channel(16);
        let (cmd_tx, mut cmd_rx) = mpsc::channel(16);
        let store = Arc::new(nephila_store::SqliteStore::open_in_memory(384).unwrap());

        let objective_id = ObjectiveId::new();
        let dir = std::path::PathBuf::from("/tmp/test");
        let mut config = test_supervision_config();
        config.max_restarts = 1;

        let mut supervisor = LifecycleSupervisor::new(
            event_tx.subscribe(),
            cmd_tx,
            store.clone(),
            test_lifecycle_config(),
            config,
        );

        let agent_id_1 = AgentId::new();
        register_agent_with_checkpoint(&store, agent_id_1, objective_id, &dir, Some("test")).await;

        supervisor.handle_agent_exited(agent_id_1).await;
        assert!(cmd_rx.try_recv().is_ok(), "first respawn should succeed");

        let agent_id_2 = AgentId::new();
        register_agent_with_checkpoint(&store, agent_id_2, objective_id, &dir, Some("test")).await;

        supervisor.handle_agent_exited(agent_id_2).await;
        assert!(
            cmd_rx.try_recv().is_err(),
            "second respawn should be blocked by limit"
        );
    }
}

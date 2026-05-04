use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use nephila_connector::{
    ClaudeCodeConnector, ProcessHandle, SpawnConfig, TaskConnector, TaskConnectorKind,
};
use nephila_core::agent::{Agent, AgentCommand, AgentEvent, AgentState, SpawnOrigin};
use nephila_core::command::OrchestratorCommand;
use nephila_core::config::ConnectorConfig;
use nephila_core::directive::Directive;
use nephila_core::event::BusEvent;
use nephila_core::id::{AgentId, ObjectiveId};
use nephila_core::store::AgentStore;
use nephila_mcp::state::HitlRequest;
use nephila_store::SqliteStore;
use tokio::sync::{RwLock, broadcast, mpsc};

const SYSTEM_PROMPT_TEMPLATE: &str = include_str!("../../config/agent_system_prompt.md");

pub struct Orchestrator {
    agents: HashMap<AgentId, Agent>,
    connectors: HashMap<AgentId, TaskConnectorKind>,
    process_handles: HashMap<AgentId, ProcessHandle>,
    connector_config: ConnectorConfig,
    store: Arc<SqliteStore>,
    event_tx: broadcast::Sender<BusEvent>,
    hitl_requests: Arc<RwLock<HashMap<AgentId, HitlRequest>>>,
    max_agent_depth: u32,
    mcp_endpoint: String,
    cmd_tx: mpsc::Sender<OrchestratorCommand>,
}

impl Orchestrator {
    pub async fn load(
        store: Arc<SqliteStore>,
        event_tx: broadcast::Sender<BusEvent>,
        hitl_requests: Arc<RwLock<HashMap<AgentId, HitlRequest>>>,
        max_agent_depth: u32,
        connector_config: ConnectorConfig,
        mcp_endpoint: String,
        cmd_tx: mpsc::Sender<OrchestratorCommand>,
    ) -> color_eyre::Result<Self> {
        let agents_list = store.list().await?;
        let agents = agents_list.into_iter().map(|a| (a.id, a)).collect();
        Ok(Self {
            agents,
            connectors: HashMap::new(),
            process_handles: HashMap::new(),
            connector_config,
            store,
            event_tx,
            hitl_requests,
            max_agent_depth,
            mcp_endpoint,
            cmd_tx,
        })
    }

    fn agent_depth(&self, agent_id: AgentId) -> u32 {
        let mut depth = 0;
        let mut current = Some(agent_id);
        while let Some(id) = current {
            match self.agents.get(&id) {
                Some(agent) => {
                    current = agent.origin.spawned_by();
                    depth += 1;
                }
                None => break,
            }
        }
        depth
    }

    pub async fn run(&mut self, mut rx: mpsc::Receiver<OrchestratorCommand>) {
        while let Some(cmd) = rx.recv().await {
            tracing::debug!(?cmd, "orchestrator received command");

            let result = match cmd {
                OrchestratorCommand::Spawn {
                    objective_id,
                    content,
                    dir,
                } => self
                    .spawn(objective_id, content, dir, None)
                    .await
                    .map(|_| ()),
                OrchestratorCommand::SpawnAgent {
                    objective_id,
                    content,
                    dir,
                    spawned_by,
                } => {
                    let depth = self.agent_depth(spawned_by);
                    if depth >= self.max_agent_depth {
                        tracing::warn!(%spawned_by, depth, max = self.max_agent_depth, "spawn rejected: max agent depth exceeded");
                        Ok(())
                    } else {
                        self.spawn(objective_id, content, dir, Some(spawned_by))
                            .await
                            .map(|_| ())
                    }
                }
                OrchestratorCommand::Kill { agent_id } => {
                    if let Some(handle) = self.process_handles.get(&agent_id)
                        && let Err(e) = handle.kill().await
                    {
                        tracing::warn!(%agent_id, %e, "failed to kill process");
                    }
                    self.dispatch(agent_id, AgentCommand::Kill).await
                }
                OrchestratorCommand::Pause { agent_id } => {
                    self.dispatch(agent_id, AgentCommand::Pause).await
                }
                OrchestratorCommand::Resume { agent_id } => {
                    self.dispatch(agent_id, AgentCommand::Resume).await
                }
                OrchestratorCommand::HitlRespond { agent_id, response } => {
                    self.hitl_respond(agent_id, response).await;
                    Ok(())
                }
                OrchestratorCommand::Suspend { agent_id } => {
                    if let Some(handle) = self.process_handles.get(&agent_id)
                        && let Err(e) = handle.kill().await
                    {
                        tracing::warn!(%agent_id, %e, "failed to kill process for suspend");
                    }
                    self.dispatch(agent_id, AgentCommand::Kill).await
                }
                OrchestratorCommand::TokenThreshold {
                    agent_id,
                    directive,
                } => match directive {
                    Directive::Abort => self.dispatch(agent_id, AgentCommand::Kill).await,
                    Directive::PrepareReset => {
                        self.dispatch(agent_id, AgentCommand::StartSuspending).await
                    }
                    _ => Ok(()),
                },
                OrchestratorCommand::AgentExited { agent_id, success } => {
                    self.handle_agent_exited(agent_id, success).await
                }
                OrchestratorCommand::Respawn {
                    objective_id,
                    content,
                    dir,
                    restore_checkpoint_id,
                } => match self.spawn(objective_id, content, dir, None).await {
                    Ok(new_agent_id) => {
                        if let Some(agent) = self.agents.get_mut(&new_agent_id) {
                            agent.restore_checkpoint_id = Some(restore_checkpoint_id);
                            if let Err(e) = AgentStore::save(self.store.as_ref(), agent).await {
                                tracing::error!(%e, %new_agent_id, "failed to save restore checkpoint");
                            }
                        }
                        Ok(())
                    }
                    Err(e) => Err(e),
                },
            };

            if let Err(e) = result {
                tracing::error!(%e, "orchestrator command error");
            }
        }

        tracing::debug!("orchestrator exiting");
    }

    async fn spawn(
        &mut self,
        objective_id: ObjectiveId,
        content: String,
        dir: PathBuf,
        spawned_by: Option<AgentId>,
    ) -> color_eyre::Result<AgentId> {
        let origin = match spawned_by {
            Some(parent) => SpawnOrigin::Agent(parent),
            None => SpawnOrigin::Operator,
        };
        let agent = Agent::new(
            AgentId::new(),
            objective_id,
            dir.clone(),
            origin,
            Some(content.clone()),
        );
        let agent_id = agent.id;

        self.store.register(agent.clone()).await?;

        let events = agent
            .handle(AgentCommand::Activate)
            .map_err(|e| color_eyre::eyre::eyre!("{e}"))?;
        let agent = events.iter().fold(agent, |a, e| a.apply_event(e));

        let session_id = uuid::Uuid::new_v4().to_string();
        let session_events = agent
            .handle(AgentCommand::SetSession {
                session_id: session_id.clone(),
            })
            .map_err(|e| color_eyre::eyre::eyre!("{e}"))?;
        let agent = session_events.iter().fold(agent, |a, e| a.apply_event(e));

        AgentStore::save(self.store.as_ref(), &agent).await?;
        self.publish(&events);
        self.publish(&session_events);
        self.agents.insert(agent_id, agent);

        let connector = ClaudeCodeConnector::new(self.connector_config.claude_binary.clone());
        let spawn_config = SpawnConfig {
            directory: dir,
            mcp_endpoint: self.mcp_endpoint.clone(),
            request_config: None,
        };

        let prompt = nephila_lifecycle::compose_next_prompt(
            SYSTEM_PROMPT_TEMPLATE,
            agent_id,
            objective_id,
            &self.mcp_endpoint,
            &content,
        );

        let (_task_handle, process_handle) = connector
            .spawn(agent_id, &spawn_config, &prompt, &session_id)
            .await
            .map_err(|e| color_eyre::eyre::eyre!("spawn failed: {e}"))?;

        self.connectors
            .insert(agent_id, TaskConnectorKind::ClaudeCode(connector));
        self.process_handles
            .insert(agent_id, process_handle.clone());

        let cmd_tx = self.cmd_tx.clone();
        let watcher_handle = process_handle;
        tokio::spawn(async move {
            let result = watcher_handle.wait().await;
            let success = result.is_ok();
            let _ = cmd_tx
                .send(OrchestratorCommand::AgentExited { agent_id, success })
                .await;
        });

        if let Some(parent_id) = spawned_by
            && let Some(parent) = self.agents.get_mut(&parent_id)
        {
            parent.children.push(agent_id);
            AgentStore::save(self.store.as_ref(), parent).await?;
        }

        Ok(agent_id)
    }

    async fn handle_agent_exited(
        &mut self,
        agent_id: AgentId,
        success: bool,
    ) -> color_eyre::Result<()> {
        let agent = match self.agents.get(&agent_id) {
            Some(a) => a,
            None => {
                tracing::warn!(%agent_id, "AgentExited for unknown agent");
                return Ok(());
            }
        };

        if matches!(
            agent.state,
            AgentState::Exited | AgentState::Completed | AgentState::Failed
        ) {
            return Ok(());
        }

        let cmd = match agent.state {
            AgentState::Suspending => AgentCommand::Kill,
            AgentState::Active if success => AgentCommand::Complete,
            _ => AgentCommand::Fail {
                reason: "process exited unexpectedly".into(),
            },
        };

        self.dispatch(agent_id, cmd).await?;
        self.process_handles.remove(&agent_id);
        Ok(())
    }

    async fn dispatch(&mut self, agent_id: AgentId, cmd: AgentCommand) -> color_eyre::Result<()> {
        let agent = self
            .agents
            .get(&agent_id)
            .ok_or_else(|| color_eyre::eyre::eyre!("agent not found: {agent_id}"))?;

        let events = agent
            .handle(cmd)
            .map_err(|e| color_eyre::eyre::eyre!("{e}"))?;
        let agent = events.iter().fold(agent.clone(), |a, e| a.apply_event(e));
        AgentStore::save(self.store.as_ref(), &agent).await?;
        self.publish(&events);
        self.agents.insert(agent_id, agent);
        Ok(())
    }

    async fn hitl_respond(&self, agent_id: AgentId, response: String) {
        let request = self.hitl_requests.write().await.remove(&agent_id);
        match request {
            Some(req) => {
                let _ = req.response_tx.send(response.clone());
            }
            None => {
                tracing::warn!(%agent_id, "HITL response received but no pending request found");
            }
        }
        let _ = self
            .event_tx
            .send(BusEvent::HitlResponded { agent_id, response });
    }

    fn publish(&self, events: &[AgentEvent]) {
        for event in events {
            match event {
                AgentEvent::StateChanged {
                    agent_id,
                    old_state,
                    new_state,
                } => {
                    let _ = self.event_tx.send(BusEvent::AgentStateChanged {
                        agent_id: *agent_id,
                        old_state: *old_state,
                        new_state: *new_state,
                    });
                }
                AgentEvent::SessionReady {
                    agent_id,
                    session_id,
                    directory,
                } => {
                    let _ = self.event_tx.send(BusEvent::AgentSessionReady {
                        agent_id: *agent_id,
                        session_id: session_id.clone(),
                        directory: directory.clone(),
                    });
                }
                AgentEvent::DirectiveChanged { .. }
                | AgentEvent::CheckpointIdSet { .. }
                | AgentEvent::AgentSpawned { .. }
                | AgentEvent::AgentKilled { .. }
                | AgentEvent::HitlRequested { .. }
                | AgentEvent::HitlResolved { .. }
                | AgentEvent::TokenThresholdReached { .. }
                | AgentEvent::AgentSessionAssigned { .. }
                | AgentEvent::AgentConfigSnapshotted { .. } => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nephila_core::agent::Agent;
    use nephila_core::id::{AgentId, ObjectiveId};
    use std::path::PathBuf;

    fn make_agent(id: AgentId, spawned_by: Option<AgentId>) -> Agent {
        use nephila_core::agent::SpawnOrigin;
        let origin = match spawned_by {
            Some(parent) => SpawnOrigin::Agent(parent),
            None => SpawnOrigin::Operator,
        };
        Agent::new(id, ObjectiveId::new(), PathBuf::from("/tmp"), origin, None)
    }

    fn test_orchestrator(agents: HashMap<AgentId, Agent>) -> Orchestrator {
        let store = Arc::new(SqliteStore::open_in_memory(384).unwrap());
        let (event_tx, _) = broadcast::channel(16);
        let (cmd_tx, _cmd_rx) = mpsc::channel(16);
        Orchestrator {
            agents,
            connectors: HashMap::new(),
            process_handles: HashMap::new(),
            connector_config: ConnectorConfig::default(),
            store,
            event_tx,
            hitl_requests: Arc::new(RwLock::new(HashMap::new())),
            max_agent_depth: 3,
            mcp_endpoint: "http://localhost:8080/mcp".into(),
            cmd_tx,
        }
    }

    #[test]
    fn depth_of_root_agent_is_one() {
        let id = AgentId::new();
        let mut agents = HashMap::new();
        agents.insert(id, make_agent(id, None));

        let orch = test_orchestrator(agents);
        assert_eq!(orch.agent_depth(id), 1);
    }

    #[test]
    fn depth_of_child_is_two() {
        let parent = AgentId::new();
        let child = AgentId::new();
        let mut agents = HashMap::new();
        agents.insert(parent, make_agent(parent, None));
        agents.insert(child, make_agent(child, Some(parent)));

        let orch = test_orchestrator(agents);
        assert_eq!(orch.agent_depth(child), 2);
    }

    #[test]
    fn depth_chain_of_three() {
        let a = AgentId::new();
        let b = AgentId::new();
        let c = AgentId::new();
        let mut agents = HashMap::new();
        agents.insert(a, make_agent(a, None));
        agents.insert(b, make_agent(b, Some(a)));
        agents.insert(c, make_agent(c, Some(b)));

        let orch = test_orchestrator(agents);
        assert_eq!(orch.agent_depth(c), 3);
    }

    #[test]
    fn depth_of_unknown_agent_is_zero() {
        let orch = test_orchestrator(HashMap::new());
        assert_eq!(orch.agent_depth(AgentId::new()), 0);
    }

    fn activate_agent(agent: Agent) -> Agent {
        let events = agent.handle(AgentCommand::Activate).unwrap();
        events.iter().fold(agent, |a, e| a.apply_event(e))
    }

    #[tokio::test]
    async fn agent_exited_from_suspending_transitions_to_exited() {
        let agent_id = AgentId::new();
        let objective_id = ObjectiveId::new();

        let mut agent = make_agent(agent_id, None);
        agent.objective_id = objective_id;
        let agent = activate_agent(agent);
        let events = agent.handle(AgentCommand::StartSuspending).unwrap();
        let agent = events.iter().fold(agent, |a, e| a.apply_event(e));
        assert_eq!(agent.state, AgentState::Suspending);

        let mut agents = HashMap::new();
        agents.insert(agent_id, agent.clone());

        let mut orch = test_orchestrator(agents);
        orch.store.register(agent).await.unwrap();

        let result = orch.handle_agent_exited(agent_id, true).await;
        assert!(result.is_ok());

        let agent = orch.agents.get(&agent_id).unwrap();
        assert_eq!(agent.state, AgentState::Exited);
    }

    #[tokio::test]
    async fn agent_exited_active_success_transitions_to_completed() {
        let agent_id = AgentId::new();
        let agent = activate_agent(make_agent(agent_id, None));
        assert_eq!(agent.state, AgentState::Active);

        let mut agents = HashMap::new();
        agents.insert(agent_id, agent.clone());

        let mut orch = test_orchestrator(agents);
        orch.store.register(agent).await.unwrap();

        orch.handle_agent_exited(agent_id, true).await.unwrap();

        let agent = orch.agents.get(&agent_id).unwrap();
        assert_eq!(agent.state, AgentState::Completed);
    }

    #[tokio::test]
    async fn agent_exited_active_failure_transitions_to_failed() {
        let agent_id = AgentId::new();
        let agent = activate_agent(make_agent(agent_id, None));

        let mut agents = HashMap::new();
        agents.insert(agent_id, agent.clone());

        let mut orch = test_orchestrator(agents);
        orch.store.register(agent).await.unwrap();

        orch.handle_agent_exited(agent_id, false).await.unwrap();

        let agent = orch.agents.get(&agent_id).unwrap();
        assert_eq!(agent.state, AgentState::Failed);
    }

    #[tokio::test]
    async fn agent_exited_unknown_agent_is_ok() {
        let mut orch = test_orchestrator(HashMap::new());
        let result = orch.handle_agent_exited(AgentId::new(), true).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn agent_exited_already_completed_is_noop() {
        let agent_id = AgentId::new();
        let agent = activate_agent(make_agent(agent_id, None));
        let events = agent.handle(AgentCommand::Complete).unwrap();
        let agent = events.iter().fold(agent, |a, e| a.apply_event(e));
        assert_eq!(agent.state, AgentState::Completed);

        let mut agents = HashMap::new();
        agents.insert(agent_id, agent.clone());

        let mut orch = test_orchestrator(agents);
        orch.store.register(agent).await.unwrap();

        orch.handle_agent_exited(agent_id, false).await.unwrap();
        // State should remain Completed (no-op)
        assert_eq!(
            orch.agents.get(&agent_id).unwrap().state,
            AgentState::Completed
        );
    }

    #[tokio::test]
    async fn dispatch_activates_agent_and_persists() {
        let agent_id = AgentId::new();
        let agent = make_agent(agent_id, None);
        assert_eq!(agent.state, AgentState::Starting);

        let mut agents = HashMap::new();
        agents.insert(agent_id, agent.clone());

        let mut orch = test_orchestrator(agents);
        orch.store.register(agent).await.unwrap();

        orch.dispatch(agent_id, AgentCommand::Activate)
            .await
            .unwrap();

        assert_eq!(
            orch.agents.get(&agent_id).unwrap().state,
            AgentState::Active
        );
    }

    #[tokio::test]
    async fn dispatch_unknown_agent_errors() {
        let mut orch = test_orchestrator(HashMap::new());
        let result = orch.dispatch(AgentId::new(), AgentCommand::Activate).await;
        assert!(result.is_err());
    }

    #[test]
    fn publish_emits_state_changed_event() {
        let (event_tx, mut event_rx) = broadcast::channel(16);
        let store = Arc::new(SqliteStore::open_in_memory(384).unwrap());
        let (cmd_tx, _) = mpsc::channel(16);
        let orch = Orchestrator {
            agents: HashMap::new(),
            connectors: HashMap::new(),
            process_handles: HashMap::new(),
            connector_config: ConnectorConfig::default(),
            store,
            event_tx,
            hitl_requests: Arc::new(RwLock::new(HashMap::new())),
            max_agent_depth: 3,
            mcp_endpoint: "http://localhost:8080/mcp".into(),
            cmd_tx,
        };

        let agent_id = AgentId::new();
        let events = vec![AgentEvent::StateChanged {
            agent_id,
            old_state: AgentState::Starting,
            new_state: AgentState::Active,
        }];
        orch.publish(&events);

        let received = event_rx.try_recv().unwrap();
        assert!(matches!(
            received,
            BusEvent::AgentStateChanged {
                new_state: AgentState::Active,
                ..
            }
        ));
    }

    #[test]
    fn publish_emits_session_ready_event() {
        let (event_tx, mut event_rx) = broadcast::channel(16);
        let store = Arc::new(SqliteStore::open_in_memory(384).unwrap());
        let (cmd_tx, _) = mpsc::channel(16);
        let orch = Orchestrator {
            agents: HashMap::new(),
            connectors: HashMap::new(),
            process_handles: HashMap::new(),
            connector_config: ConnectorConfig::default(),
            store,
            event_tx,
            hitl_requests: Arc::new(RwLock::new(HashMap::new())),
            max_agent_depth: 3,
            mcp_endpoint: "http://localhost:8080/mcp".into(),
            cmd_tx,
        };

        let agent_id = AgentId::new();
        let events = vec![AgentEvent::SessionReady {
            agent_id,
            session_id: "sess-1".into(),
            directory: PathBuf::from("/tmp"),
        }];
        orch.publish(&events);

        let received = event_rx.try_recv().unwrap();
        assert!(matches!(received, BusEvent::AgentSessionReady { .. }));
    }

    #[test]
    fn publish_skips_non_broadcast_events() {
        let (event_tx, mut event_rx) = broadcast::channel(16);
        let store = Arc::new(SqliteStore::open_in_memory(384).unwrap());
        let (cmd_tx, _) = mpsc::channel(16);
        let orch = Orchestrator {
            agents: HashMap::new(),
            connectors: HashMap::new(),
            process_handles: HashMap::new(),
            connector_config: ConnectorConfig::default(),
            store,
            event_tx,
            hitl_requests: Arc::new(RwLock::new(HashMap::new())),
            max_agent_depth: 3,
            mcp_endpoint: "http://localhost:8080/mcp".into(),
            cmd_tx,
        };

        let events = vec![AgentEvent::DirectiveChanged {
            agent_id: AgentId::new(),
            directive: Directive::Continue,
        }];
        orch.publish(&events);

        assert!(event_rx.try_recv().is_err());
    }
}

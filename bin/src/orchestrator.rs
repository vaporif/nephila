use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use meridian_core::agent::{Agent, AgentCommand, AgentEvent};
use meridian_core::checkpoint::CheckpointSummary;
use meridian_core::command::OrchestratorCommand;
use meridian_core::directive::Directive;
use meridian_core::event::BusEvent;
use meridian_core::id::{AgentId, ObjectiveId};
use meridian_core::store::{AgentStore, CheckpointStore};
use meridian_mcp::state::HitlRequest;
use meridian_store::SqliteStore;
use tokio::sync::{RwLock, broadcast, mpsc};

pub struct Orchestrator {
    agents: HashMap<AgentId, Agent>,
    store: Arc<SqliteStore>,
    event_tx: broadcast::Sender<BusEvent>,
    hitl_requests: Arc<RwLock<HashMap<AgentId, HitlRequest>>>,
    max_agent_depth: u32,
}

impl Orchestrator {
    pub async fn load(
        store: Arc<SqliteStore>,
        event_tx: broadcast::Sender<BusEvent>,
        hitl_requests: Arc<RwLock<HashMap<AgentId, HitlRequest>>>,
        max_agent_depth: u32,
    ) -> color_eyre::Result<Self> {
        let agents_list = store.list().await?;
        let agents = agents_list.into_iter().map(|a| (a.id, a)).collect();
        Ok(Self {
            agents,
            store,
            event_tx,
            hitl_requests,
            max_agent_depth,
        })
    }

    fn agent_depth(&self, agent_id: AgentId) -> u32 {
        let mut depth = 0;
        let mut current = Some(agent_id);
        while let Some(id) = current {
            match self.agents.get(&id) {
                Some(agent) => {
                    current = agent.spawned_by;
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
                    self.dispatch(agent_id, AgentCommand::Kill).await
                }
                OrchestratorCommand::Pause { agent_id } => {
                    self.dispatch(agent_id, AgentCommand::Pause).await
                }
                OrchestratorCommand::Resume { agent_id } => {
                    self.dispatch(agent_id, AgentCommand::Resume).await
                }
                OrchestratorCommand::Rollback { agent_id, version } => {
                    self.dispatch(agent_id, AgentCommand::Rollback { version })
                        .await
                }
                OrchestratorCommand::ListCheckpoints { agent_id } => {
                    self.list_checkpoints(agent_id).await;
                    Ok(())
                }
                OrchestratorCommand::HitlRespond { agent_id, response } => {
                    self.hitl_respond(agent_id, response).await;
                    Ok(())
                }
                OrchestratorCommand::RequestReset { agent_id } => {
                    self.dispatch(agent_id, AgentCommand::Kill).await
                }
                OrchestratorCommand::TokenThreshold {
                    agent_id,
                    directive,
                } => match directive {
                    Directive::Abort => self.dispatch(agent_id, AgentCommand::Kill).await,
                    Directive::PrepareReset => {
                        self.dispatch(agent_id, AgentCommand::StartDraining).await
                    }
                    _ => Ok(()),
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
        let mut agent = Agent::new(AgentId::new(), objective_id, dir, spawned_by, Some(content));
        let agent_id = agent.id;

        self.store.register(agent.clone()).await?;

        let events = agent
            .handle(AgentCommand::Activate)
            .map_err(|e| color_eyre::eyre::eyre!("{e}"))?;

        let session_id = uuid::Uuid::new_v4().to_string();
        let session_events = agent
            .handle(AgentCommand::SetSession { session_id })
            .map_err(|e| color_eyre::eyre::eyre!("{e}"))?;

        AgentStore::save(self.store.as_ref(), &agent).await?;
        self.publish(&events);
        self.publish(&session_events);
        self.agents.insert(agent_id, agent);

        Ok(agent_id)
    }

    async fn dispatch(&mut self, agent_id: AgentId, cmd: AgentCommand) -> color_eyre::Result<()> {
        let agent = self
            .agents
            .get_mut(&agent_id)
            .ok_or_else(|| color_eyre::eyre::eyre!("agent not found: {agent_id}"))?;

        let events = agent
            .handle(cmd)
            .map_err(|e| color_eyre::eyre::eyre!("{e}"))?;
        AgentStore::save(self.store.as_ref(), agent).await?;
        self.publish(&events);
        Ok(())
    }

    async fn list_checkpoints(&self, agent_id: AgentId) {
        match self.store.list_versions(agent_id).await {
            Ok(versions) => {
                let mut summaries = Vec::new();
                for v in versions {
                    match self.store.get_version(agent_id, v).await {
                        Err(e) => {
                            tracing::warn!(%agent_id, %v, %e, "failed to load checkpoint version");
                        }
                        Ok(None) => {}
                        Ok(Some(cp)) => {
                            summaries.push(CheckpointSummary {
                                version: cp.version,
                                timestamp: cp.timestamp,
                                summary: cp.l1.chars().take(80).collect(),
                            });
                        }
                    }
                }
                let _ = self.event_tx.send(BusEvent::CheckpointList {
                    agent_id,
                    versions: summaries,
                });
            }
            Err(e) => {
                tracing::error!(%agent_id, %e, "failed to list checkpoints");
                let _ = self.event_tx.send(BusEvent::CheckpointList {
                    agent_id,
                    versions: vec![],
                });
            }
        }
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
                AgentEvent::DirectiveChanged { .. } | AgentEvent::CheckpointVersionSet { .. } => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meridian_core::agent::Agent;
    use meridian_core::id::{AgentId, ObjectiveId};
    use std::path::PathBuf;

    fn make_agent(id: AgentId, spawned_by: Option<AgentId>) -> Agent {
        Agent::new(id, ObjectiveId::new(), PathBuf::from("/tmp"), spawned_by, None)
    }

    fn test_orchestrator(agents: HashMap<AgentId, Agent>) -> Orchestrator {
        let store = Arc::new(SqliteStore::open_in_memory(384).unwrap());
        let (event_tx, _) = broadcast::channel(16);
        Orchestrator {
            agents,
            store,
            event_tx,
            hitl_requests: Arc::new(RwLock::new(HashMap::new())),
            max_agent_depth: 3,
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
}

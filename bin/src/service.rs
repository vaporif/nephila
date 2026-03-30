use meridian_core::agent::{Agent, AgentCommand, AgentEvent};
use meridian_core::checkpoint::CheckpointSummary;
use meridian_core::event::BusEvent;
use meridian_core::id::{AgentId, ObjectiveId};
use meridian_core::store::{AgentStore, CheckpointStore};
use meridian_store::SqliteStore;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::broadcast;

pub struct AgentService {
    agents: HashMap<AgentId, Agent>,
    store: Arc<SqliteStore>,
    event_tx: broadcast::Sender<BusEvent>,
}

impl AgentService {
    pub async fn load(
        store: Arc<SqliteStore>,
        event_tx: broadcast::Sender<BusEvent>,
    ) -> color_eyre::Result<Self> {
        let agents_list = store.list().await?;
        let agents = agents_list.into_iter().map(|a| (a.id, a)).collect();
        Ok(Self {
            agents,
            store,
            event_tx,
        })
    }

    pub async fn spawn(
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

    pub async fn dispatch(
        &mut self,
        agent_id: AgentId,
        cmd: AgentCommand,
    ) -> color_eyre::Result<()> {
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

    pub async fn list_checkpoints(&self, agent_id: AgentId) {
        match self.store.list_versions(agent_id).await {
            Ok(versions) => {
                let mut summaries = Vec::new();
                for v in versions {
                    if let Ok(Some(cp)) = self.store.get_version(agent_id, v).await {
                        summaries.push(CheckpointSummary {
                            version: cp.version,
                            timestamp: cp.timestamp,
                            summary: cp.l1.chars().take(80).collect(),
                        });
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

    pub fn hitl_respond(&self, agent_id: AgentId, response: String) {
        let _ = self.event_tx.send(BusEvent::HitlResponded {
            agent_id,
            response,
        });
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

use crate::directive::Directive;
use crate::id::{AgentId, CheckpointId, ObjectiveId};
use chrono::{DateTime, Utc};
use meridian_eventsourcing::aggregate::EventSourced;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display, strum::EnumString,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum AgentState {
    Starting,
    Active,
    Suspending,
    Exited,
    Completed,
    Failed,
    Paused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentPhase {
    Starting,
    Active,
    Suspending,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpawnOrigin {
    Operator,
    Agent(AgentId),
    Fork {
        source_agent_id: AgentId,
        source_checkpoint_id: CheckpointId,
    },
}

impl SpawnOrigin {
    #[must_use]
    pub fn spawned_by(&self) -> Option<AgentId> {
        match self {
            Self::Operator => None,
            Self::Agent(id) => Some(*id),
            Self::Fork {
                source_agent_id, ..
            } => Some(*source_agent_id),
        }
    }
}

impl AgentState {
    #[must_use]
    pub fn from_str_lossy(s: &str) -> Self {
        s.parse().unwrap_or(Self::Starting)
    }

    pub fn phase(self) -> Option<AgentPhase> {
        match self {
            Self::Starting => Some(AgentPhase::Starting),
            Self::Active | Self::Paused => Some(AgentPhase::Active),
            Self::Suspending => Some(AgentPhase::Suspending),
            Self::Exited | Self::Completed | Self::Failed => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub id: AgentId,
    pub state: AgentState,
    pub directive: Directive,
    pub session_id: Option<String>,
    pub directory: PathBuf,
    pub objective_id: ObjectiveId,
    pub checkpoint_id: Option<CheckpointId>,
    pub restore_checkpoint_id: Option<CheckpointId>,
    pub origin: SpawnOrigin,
    pub children: Vec<AgentId>,
    pub injected_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentCommand {
    Activate,
    Kill,
    Pause,
    Resume,
    StartSuspending,
    Complete,
    Fail { reason: String },
    SetSession { session_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentEvent {
    StateChanged {
        agent_id: AgentId,
        old_state: AgentState,
        new_state: AgentState,
    },
    DirectiveChanged {
        agent_id: AgentId,
        directive: Directive,
    },
    CheckpointIdSet {
        agent_id: AgentId,
        checkpoint_id: CheckpointId,
    },
    SessionReady {
        agent_id: AgentId,
        session_id: String,
        directory: PathBuf,
    },
    AgentSpawned {
        agent_id: AgentId,
        parent_id: Option<AgentId>,
        objective_id: ObjectiveId,
    },
    AgentKilled {
        agent_id: AgentId,
        reason: Option<String>,
    },
    HitlRequested {
        agent_id: AgentId,
        question: String,
    },
    HitlResolved {
        agent_id: AgentId,
        response: String,
    },
    TokenThresholdReached {
        agent_id: AgentId,
        tokens_used: u64,
        threshold: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransitionError {
    #[error("invalid transition: {command} not allowed in {from} state")]
    InvalidTransition { from: AgentState, command: String },
    #[error("agent is in terminal state: {state}")]
    TerminalState { state: AgentState },
}

impl Agent {
    pub fn new(
        id: AgentId,
        objective_id: ObjectiveId,
        directory: PathBuf,
        origin: SpawnOrigin,
        injected_message: Option<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id,
            state: AgentState::Starting,
            directive: Directive::Continue,
            session_id: None,
            directory,
            objective_id,
            checkpoint_id: None,
            restore_checkpoint_id: None,
            origin,
            children: Vec::new(),
            injected_message,
            created_at: now,
            updated_at: now,
        }
    }

    fn is_terminal(&self) -> bool {
        matches!(
            self.state,
            AgentState::Exited | AgentState::Completed | AgentState::Failed
        )
    }

    pub fn handle(&self, cmd: AgentCommand) -> Result<Vec<AgentEvent>, TransitionError> {
        if self.is_terminal() {
            return Err(TransitionError::TerminalState { state: self.state });
        }

        if let AgentCommand::SetSession { session_id } = cmd {
            return Ok(vec![AgentEvent::SessionReady {
                agent_id: self.id,
                session_id,
                directory: self.directory.clone(),
            }]);
        }

        let mut events = Vec::new();

        match (self.state, &cmd) {
            // Starting
            (AgentState::Starting, AgentCommand::Activate) => {
                events.push(AgentEvent::StateChanged {
                    agent_id: self.id,
                    old_state: self.state,
                    new_state: AgentState::Active,
                });
            }
            (AgentState::Starting, AgentCommand::Kill) => {
                events.push(AgentEvent::StateChanged {
                    agent_id: self.id,
                    old_state: self.state,
                    new_state: AgentState::Exited,
                });
                events.push(AgentEvent::DirectiveChanged {
                    agent_id: self.id,
                    directive: Directive::Abort,
                });
            }
            (AgentState::Starting, AgentCommand::Fail { .. }) => {
                events.push(AgentEvent::StateChanged {
                    agent_id: self.id,
                    old_state: self.state,
                    new_state: AgentState::Failed,
                });
            }

            // Active
            (AgentState::Active, AgentCommand::Pause) => {
                events.push(AgentEvent::StateChanged {
                    agent_id: self.id,
                    old_state: self.state,
                    new_state: AgentState::Paused,
                });
                events.push(AgentEvent::DirectiveChanged {
                    agent_id: self.id,
                    directive: Directive::Pause,
                });
            }
            (AgentState::Active, AgentCommand::Kill) => {
                events.push(AgentEvent::StateChanged {
                    agent_id: self.id,
                    old_state: self.state,
                    new_state: AgentState::Exited,
                });
                events.push(AgentEvent::DirectiveChanged {
                    agent_id: self.id,
                    directive: Directive::Abort,
                });
            }
            (AgentState::Active, AgentCommand::StartSuspending) => {
                events.push(AgentEvent::StateChanged {
                    agent_id: self.id,
                    old_state: self.state,
                    new_state: AgentState::Suspending,
                });
            }
            (AgentState::Active, AgentCommand::Complete) => {
                events.push(AgentEvent::StateChanged {
                    agent_id: self.id,
                    old_state: self.state,
                    new_state: AgentState::Completed,
                });
            }
            (AgentState::Active, AgentCommand::Fail { .. }) => {
                events.push(AgentEvent::StateChanged {
                    agent_id: self.id,
                    old_state: self.state,
                    new_state: AgentState::Failed,
                });
            }

            // Paused
            (AgentState::Paused, AgentCommand::Resume) => {
                events.push(AgentEvent::StateChanged {
                    agent_id: self.id,
                    old_state: self.state,
                    new_state: AgentState::Active,
                });
                events.push(AgentEvent::DirectiveChanged {
                    agent_id: self.id,
                    directive: Directive::Continue,
                });
            }
            (AgentState::Paused, AgentCommand::Kill) => {
                events.push(AgentEvent::StateChanged {
                    agent_id: self.id,
                    old_state: self.state,
                    new_state: AgentState::Exited,
                });
                events.push(AgentEvent::DirectiveChanged {
                    agent_id: self.id,
                    directive: Directive::Abort,
                });
            }
            (AgentState::Paused, AgentCommand::StartSuspending) => {
                events.push(AgentEvent::StateChanged {
                    agent_id: self.id,
                    old_state: self.state,
                    new_state: AgentState::Suspending,
                });
            }
            (AgentState::Paused, AgentCommand::Fail { .. }) => {
                events.push(AgentEvent::StateChanged {
                    agent_id: self.id,
                    old_state: self.state,
                    new_state: AgentState::Failed,
                });
            }

            // Suspending
            (AgentState::Suspending, AgentCommand::Kill) => {
                events.push(AgentEvent::StateChanged {
                    agent_id: self.id,
                    old_state: self.state,
                    new_state: AgentState::Exited,
                });
                events.push(AgentEvent::DirectiveChanged {
                    agent_id: self.id,
                    directive: Directive::Abort,
                });
            }
            (AgentState::Suspending, AgentCommand::Fail { .. }) => {
                events.push(AgentEvent::StateChanged {
                    agent_id: self.id,
                    old_state: self.state,
                    new_state: AgentState::Failed,
                });
            }

            _ => {
                return Err(TransitionError::InvalidTransition {
                    from: self.state,
                    command: format!("{cmd:?}"),
                });
            }
        }

        Ok(events)
    }

    pub fn apply_event(mut self, event: &AgentEvent) -> Self {
        match event {
            AgentEvent::StateChanged { new_state, .. } => {
                self.state = *new_state;
                self.updated_at = Utc::now();
            }
            AgentEvent::DirectiveChanged { directive, .. } => {
                self.directive = *directive;
            }
            AgentEvent::CheckpointIdSet { checkpoint_id, .. } => {
                self.checkpoint_id = Some(*checkpoint_id);
            }
            AgentEvent::SessionReady { session_id, .. } => {
                self.session_id = Some(session_id.clone());
                self.updated_at = Utc::now();
            }
            AgentEvent::AgentSpawned { .. }
            | AgentEvent::AgentKilled { .. }
            | AgentEvent::HitlRequested { .. }
            | AgentEvent::HitlResolved { .. }
            | AgentEvent::TokenThresholdReached { .. } => {}
        }
        self
    }
}

impl EventSourced for Agent {
    type Event = AgentEvent;
    type Command = AgentCommand;
    type Error = TransitionError;

    fn aggregate_type() -> &'static str {
        "agent"
    }

    fn aggregate_id(&self) -> String {
        self.id.to_string()
    }

    fn apply(self, event: &Self::Event) -> Self {
        self.apply_event(event)
    }

    fn handle(&self, command: Self::Command) -> Result<Vec<Self::Event>, Self::Error> {
        Agent::handle(self, command)
    }

    fn default_state() -> Self {
        Agent {
            id: AgentId::new(),
            state: AgentState::Starting,
            directive: Directive::Continue,
            session_id: None,
            directory: PathBuf::from("/tmp"),
            objective_id: ObjectiveId::new(),
            checkpoint_id: None,
            restore_checkpoint_id: None,
            origin: SpawnOrigin::Operator,
            children: Vec::new(),
            injected_message: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directive::Directive;

    fn test_agent() -> Agent {
        Agent::new(
            AgentId::new(),
            ObjectiveId::new(),
            PathBuf::from("/tmp/test"),
            SpawnOrigin::Operator,
            None,
        )
    }

    fn apply_all(agent: Agent, events: &[AgentEvent]) -> Agent {
        events.iter().fold(agent, |a, e| a.apply_event(e))
    }

    #[test]
    fn starting_activate_transitions_to_active() {
        let agent = test_agent();
        assert_eq!(agent.state, AgentState::Starting);
        let events = agent.handle(AgentCommand::Activate).unwrap();
        let agent = apply_all(agent, &events);
        assert_eq!(agent.state, AgentState::Active);
    }

    #[test]
    fn starting_kill_transitions_to_exited() {
        let agent = test_agent();
        let events = agent.handle(AgentCommand::Kill).unwrap();
        let agent = apply_all(agent, &events);
        assert_eq!(agent.state, AgentState::Exited);
        assert_eq!(agent.directive, Directive::Abort);
    }

    #[test]
    fn starting_fail_transitions_to_failed() {
        let agent = test_agent();
        let events = agent
            .handle(AgentCommand::Fail {
                reason: "oops".into(),
            })
            .unwrap();
        let agent = apply_all(agent, &events);
        assert_eq!(agent.state, AgentState::Failed);
    }

    #[test]
    fn active_pause_transitions_to_paused() {
        let agent = test_agent();
        let events = agent.handle(AgentCommand::Activate).unwrap();
        let agent = apply_all(agent, &events);
        let events = agent.handle(AgentCommand::Pause).unwrap();
        let agent = apply_all(agent, &events);
        assert_eq!(agent.state, AgentState::Paused);
        assert_eq!(agent.directive, Directive::Pause);
    }

    #[test]
    fn active_start_suspending_transitions_to_suspending() {
        let agent = test_agent();
        let events = agent.handle(AgentCommand::Activate).unwrap();
        let agent = apply_all(agent, &events);
        let events = agent.handle(AgentCommand::StartSuspending).unwrap();
        let agent = apply_all(agent, &events);
        assert_eq!(agent.state, AgentState::Suspending);
    }

    #[test]
    fn active_complete_transitions_to_completed() {
        let agent = test_agent();
        let events = agent.handle(AgentCommand::Activate).unwrap();
        let agent = apply_all(agent, &events);
        let events = agent.handle(AgentCommand::Complete).unwrap();
        let agent = apply_all(agent, &events);
        assert_eq!(agent.state, AgentState::Completed);
    }

    #[test]
    fn active_kill_transitions_to_exited() {
        let agent = test_agent();
        let events = agent.handle(AgentCommand::Activate).unwrap();
        let agent = apply_all(agent, &events);
        let events = agent.handle(AgentCommand::Kill).unwrap();
        let agent = apply_all(agent, &events);
        assert_eq!(agent.state, AgentState::Exited);
    }

    #[test]
    fn paused_resume_transitions_to_active() {
        let agent = test_agent();
        let events = agent.handle(AgentCommand::Activate).unwrap();
        let agent = apply_all(agent, &events);
        let events = agent.handle(AgentCommand::Pause).unwrap();
        let agent = apply_all(agent, &events);
        let events = agent.handle(AgentCommand::Resume).unwrap();
        let agent = apply_all(agent, &events);
        assert_eq!(agent.state, AgentState::Active);
        assert_eq!(agent.directive, Directive::Continue);
    }

    #[test]
    fn paused_start_suspending_transitions_to_suspending() {
        let agent = test_agent();
        let events = agent.handle(AgentCommand::Activate).unwrap();
        let agent = apply_all(agent, &events);
        let events = agent.handle(AgentCommand::Pause).unwrap();
        let agent = apply_all(agent, &events);
        let events = agent.handle(AgentCommand::StartSuspending).unwrap();
        let agent = apply_all(agent, &events);
        assert_eq!(agent.state, AgentState::Suspending);
    }

    #[test]
    fn suspending_kill_transitions_to_exited() {
        let agent = test_agent();
        let events = agent.handle(AgentCommand::Activate).unwrap();
        let agent = apply_all(agent, &events);
        let events = agent.handle(AgentCommand::StartSuspending).unwrap();
        let agent = apply_all(agent, &events);
        let events = agent.handle(AgentCommand::Kill).unwrap();
        let agent = apply_all(agent, &events);
        assert_eq!(agent.state, AgentState::Exited);
    }

    #[test]
    fn suspending_fail_transitions_to_failed() {
        let agent = test_agent();
        let events = agent.handle(AgentCommand::Activate).unwrap();
        let agent = apply_all(agent, &events);
        let events = agent.handle(AgentCommand::StartSuspending).unwrap();
        let agent = apply_all(agent, &events);
        let events = agent
            .handle(AgentCommand::Fail {
                reason: "err".into(),
            })
            .unwrap();
        let agent = apply_all(agent, &events);
        assert_eq!(agent.state, AgentState::Failed);
    }

    #[test]
    fn exited_rejects_all_commands() {
        let agent = test_agent();
        let events = agent.handle(AgentCommand::Kill).unwrap();
        let agent = apply_all(agent, &events);
        assert!(agent.handle(AgentCommand::Activate).is_err());
        assert!(agent.handle(AgentCommand::Kill).is_err());
    }

    #[test]
    fn completed_rejects_all_commands() {
        let agent = test_agent();
        let events = agent.handle(AgentCommand::Activate).unwrap();
        let agent = apply_all(agent, &events);
        let events = agent.handle(AgentCommand::Complete).unwrap();
        let agent = apply_all(agent, &events);
        assert!(agent.handle(AgentCommand::Kill).is_err());
    }

    #[test]
    fn failed_rejects_all_commands() {
        let agent = test_agent();
        let events = agent.handle(AgentCommand::Activate).unwrap();
        let agent = apply_all(agent, &events);
        let events = agent
            .handle(AgentCommand::Fail {
                reason: "err".into(),
            })
            .unwrap();
        let agent = apply_all(agent, &events);
        assert!(agent.handle(AgentCommand::Resume).is_err());
    }

    #[test]
    fn starting_rejects_pause() {
        let agent = test_agent();
        assert!(agent.handle(AgentCommand::Pause).is_err());
    }

    #[test]
    fn starting_rejects_resume() {
        let agent = test_agent();
        assert!(agent.handle(AgentCommand::Resume).is_err());
    }

    #[test]
    fn active_rejects_activate() {
        let agent = test_agent();
        let events = agent.handle(AgentCommand::Activate).unwrap();
        let agent = apply_all(agent, &events);
        assert!(agent.handle(AgentCommand::Activate).is_err());
    }

    #[test]
    fn active_rejects_resume() {
        let agent = test_agent();
        let events = agent.handle(AgentCommand::Activate).unwrap();
        let agent = apply_all(agent, &events);
        assert!(agent.handle(AgentCommand::Resume).is_err());
    }

    #[test]
    fn set_session_in_starting_state() {
        let agent = test_agent();
        let events = agent
            .handle(AgentCommand::SetSession {
                session_id: "sess-1".into(),
            })
            .unwrap();
        let agent = apply_all(agent, &events);
        assert_eq!(agent.session_id, Some("sess-1".to_string()));
        assert_eq!(agent.state, AgentState::Starting);
    }

    #[test]
    fn set_session_rejected_in_terminal_state() {
        let agent = test_agent();
        let events = agent.handle(AgentCommand::Kill).unwrap();
        let agent = apply_all(agent, &events);
        assert!(
            agent
                .handle(AgentCommand::SetSession {
                    session_id: "s".into()
                })
                .is_err()
        );
    }

    #[test]
    fn spawn_origin_operator_spawned_by_returns_none() {
        assert_eq!(SpawnOrigin::Operator.spawned_by(), None);
    }

    #[test]
    fn spawn_origin_agent_spawned_by_returns_id() {
        let parent = AgentId::new();
        assert_eq!(SpawnOrigin::Agent(parent).spawned_by(), Some(parent));
    }

    #[test]
    fn spawn_origin_fork_spawned_by_returns_source_agent() {
        let source = AgentId::new();
        let cp = CheckpointId::new();
        assert_eq!(
            SpawnOrigin::Fork {
                source_agent_id: source,
                source_checkpoint_id: cp
            }
            .spawned_by(),
            Some(source),
        );
    }

    #[test]
    fn checkpoint_id_set_event_updates_agent() {
        let agent = test_agent();
        let cp_id = CheckpointId::new();
        let agent_id = agent.id;
        let agent = agent.apply_event(&AgentEvent::CheckpointIdSet {
            agent_id,
            checkpoint_id: cp_id,
        });
        assert_eq!(agent.checkpoint_id, Some(cp_id));
    }

    #[test]
    fn agent_phase_starting() {
        let agent = test_agent();
        assert_eq!(agent.state.phase(), Some(AgentPhase::Starting));
    }

    #[test]
    fn agent_phase_active() {
        let agent = test_agent();
        let events = agent.handle(AgentCommand::Activate).unwrap();
        let agent = apply_all(agent, &events);
        assert_eq!(agent.state.phase(), Some(AgentPhase::Active));
    }

    #[test]
    fn agent_phase_suspending() {
        let agent = test_agent();
        let events = agent.handle(AgentCommand::Activate).unwrap();
        let agent = apply_all(agent, &events);
        let events = agent.handle(AgentCommand::StartSuspending).unwrap();
        let agent = apply_all(agent, &events);
        assert_eq!(agent.state.phase(), Some(AgentPhase::Suspending));
    }
}

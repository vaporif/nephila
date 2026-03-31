use crate::directive::Directive;
use crate::id::{AgentId, CheckpointVersion, ObjectiveId};
use chrono::{DateTime, Utc};
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
    Draining,
    Restoring,
    Exited,
    Completed,
    Failed,
    Paused,
}

/// Determines which MCP tools are exposed to the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentPhase {
    Restoring,
    Active,
    Draining,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpawnOrigin {
    User,
    Agent(AgentId),
}

impl SpawnOrigin {
    #[must_use]
    pub fn spawned_by(&self) -> Option<AgentId> {
        match self {
            Self::User => None,
            Self::Agent(id) => Some(*id),
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
            Self::Restoring => Some(AgentPhase::Restoring),
            Self::Starting | Self::Active => Some(AgentPhase::Active),
            Self::Draining => Some(AgentPhase::Draining),
            Self::Exited | Self::Completed | Self::Failed | Self::Paused => None,
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
    pub checkpoint_version: Option<CheckpointVersion>,
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
    Rollback { version: CheckpointVersion },
    StartDraining,
    Complete,
    Fail { reason: String },
    Restored,
    SetSession { session_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
    CheckpointVersionSet {
        agent_id: AgentId,
        version: CheckpointVersion,
    },
    SessionReady {
        agent_id: AgentId,
        session_id: String,
        directory: PathBuf,
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
            checkpoint_version: None,
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

    pub fn handle(&mut self, cmd: AgentCommand) -> Result<Vec<AgentEvent>, TransitionError> {
        if self.is_terminal() {
            return Err(TransitionError::TerminalState { state: self.state });
        }

        if let AgentCommand::SetSession { session_id } = cmd {
            self.session_id = Some(session_id.clone());
            self.updated_at = Utc::now();
            return Ok(vec![AgentEvent::SessionReady {
                agent_id: self.id,
                session_id,
                directory: self.directory.clone(),
            }]);
        }

        let old_state = self.state;
        let mut events = Vec::new();

        match (self.state, &cmd) {
            (AgentState::Starting, AgentCommand::Activate) => {
                self.state = AgentState::Active;
            }
            (AgentState::Starting, AgentCommand::Kill) => {
                self.state = AgentState::Exited;
                self.directive = Directive::Abort;
                events.push(AgentEvent::DirectiveChanged {
                    agent_id: self.id,
                    directive: Directive::Abort,
                });
            }
            (AgentState::Starting, AgentCommand::Fail { .. }) => {
                self.state = AgentState::Failed;
            }

            (AgentState::Active, AgentCommand::Pause) => {
                self.state = AgentState::Paused;
                self.directive = Directive::Pause;
                events.push(AgentEvent::DirectiveChanged {
                    agent_id: self.id,
                    directive: Directive::Pause,
                });
            }
            (AgentState::Active, AgentCommand::Kill) => {
                self.state = AgentState::Exited;
                self.directive = Directive::Abort;
                events.push(AgentEvent::DirectiveChanged {
                    agent_id: self.id,
                    directive: Directive::Abort,
                });
            }
            (AgentState::Active, AgentCommand::StartDraining) => {
                self.state = AgentState::Draining;
            }
            (AgentState::Active, AgentCommand::Complete) => {
                self.state = AgentState::Completed;
            }
            (AgentState::Active, AgentCommand::Fail { .. }) => {
                self.state = AgentState::Failed;
            }

            (AgentState::Paused, AgentCommand::Resume) => {
                self.state = AgentState::Active;
                self.directive = Directive::Continue;
                events.push(AgentEvent::DirectiveChanged {
                    agent_id: self.id,
                    directive: Directive::Continue,
                });
            }
            (AgentState::Paused, AgentCommand::Kill) => {
                self.state = AgentState::Exited;
                self.directive = Directive::Abort;
                events.push(AgentEvent::DirectiveChanged {
                    agent_id: self.id,
                    directive: Directive::Abort,
                });
            }
            (AgentState::Paused, AgentCommand::Fail { .. }) => {
                self.state = AgentState::Failed;
            }

            (AgentState::Draining, AgentCommand::Kill) => {
                self.state = AgentState::Exited;
                self.directive = Directive::Abort;
                events.push(AgentEvent::DirectiveChanged {
                    agent_id: self.id,
                    directive: Directive::Abort,
                });
            }
            (AgentState::Draining, AgentCommand::Rollback { version }) => {
                self.state = AgentState::Restoring;
                self.checkpoint_version = Some(*version);
                self.directive = Directive::PrepareReset;
                events.push(AgentEvent::CheckpointVersionSet {
                    agent_id: self.id,
                    version: *version,
                });
                events.push(AgentEvent::DirectiveChanged {
                    agent_id: self.id,
                    directive: Directive::PrepareReset,
                });
            }
            (AgentState::Draining, AgentCommand::Complete) => {
                self.state = AgentState::Completed;
            }
            (AgentState::Draining, AgentCommand::Fail { .. }) => {
                self.state = AgentState::Failed;
            }

            (AgentState::Restoring, AgentCommand::Kill) => {
                self.state = AgentState::Exited;
                self.directive = Directive::Abort;
                events.push(AgentEvent::DirectiveChanged {
                    agent_id: self.id,
                    directive: Directive::Abort,
                });
            }
            (AgentState::Restoring, AgentCommand::Restored) => {
                self.state = AgentState::Active;
                self.directive = Directive::Continue;
                events.push(AgentEvent::DirectiveChanged {
                    agent_id: self.id,
                    directive: Directive::Continue,
                });
            }
            (AgentState::Restoring, AgentCommand::Fail { .. }) => {
                self.state = AgentState::Failed;
            }

            _ => {
                return Err(TransitionError::InvalidTransition {
                    from: old_state,
                    command: format!("{cmd:?}"),
                });
            }
        }

        self.updated_at = Utc::now();

        events.insert(
            0,
            AgentEvent::StateChanged {
                agent_id: self.id,
                old_state,
                new_state: self.state,
            },
        );

        Ok(events)
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
            SpawnOrigin::User,
            None,
        )
    }

    #[test]
    fn starting_activate_transitions_to_active() {
        let mut agent = test_agent();
        assert_eq!(agent.state, AgentState::Starting);
        let events = agent.handle(AgentCommand::Activate).unwrap();
        assert_eq!(agent.state, AgentState::Active);
        assert!(events.iter().any(|e| matches!(
            e,
            AgentEvent::StateChanged {
                old_state: AgentState::Starting,
                new_state: AgentState::Active,
                ..
            }
        )));
    }

    #[test]
    fn starting_kill_transitions_to_exited() {
        let mut agent = test_agent();
        let events = agent.handle(AgentCommand::Kill).unwrap();
        assert_eq!(agent.state, AgentState::Exited);
        assert_eq!(agent.directive, Directive::Abort);
        assert!(events.iter().any(|e| matches!(
            e,
            AgentEvent::DirectiveChanged {
                directive: Directive::Abort,
                ..
            }
        )));
    }

    #[test]
    fn starting_fail_transitions_to_failed() {
        let mut agent = test_agent();
        let events = agent
            .handle(AgentCommand::Fail {
                reason: "oops".into(),
            })
            .unwrap();
        assert_eq!(agent.state, AgentState::Failed);
        assert!(events.iter().any(|e| matches!(
            e,
            AgentEvent::StateChanged {
                new_state: AgentState::Failed,
                ..
            }
        )));
    }

    #[test]
    fn active_pause_transitions_to_paused() {
        let mut agent = test_agent();
        agent.handle(AgentCommand::Activate).unwrap();
        let events = agent.handle(AgentCommand::Pause).unwrap();
        assert_eq!(agent.state, AgentState::Paused);
        assert_eq!(agent.directive, Directive::Pause);
        assert!(events.iter().any(|e| matches!(
            e,
            AgentEvent::DirectiveChanged {
                directive: Directive::Pause,
                ..
            }
        )));
    }

    #[test]
    fn active_kill_transitions_to_exited() {
        let mut agent = test_agent();
        agent.handle(AgentCommand::Activate).unwrap();
        let _events = agent.handle(AgentCommand::Kill).unwrap();
        assert_eq!(agent.state, AgentState::Exited);
        assert_eq!(agent.directive, Directive::Abort);
    }

    #[test]
    fn active_start_draining_transitions_to_draining() {
        let mut agent = test_agent();
        agent.handle(AgentCommand::Activate).unwrap();
        let events = agent.handle(AgentCommand::StartDraining).unwrap();
        assert_eq!(agent.state, AgentState::Draining);
        assert!(events.iter().any(|e| matches!(
            e,
            AgentEvent::StateChanged {
                new_state: AgentState::Draining,
                ..
            }
        )));
    }

    #[test]
    fn active_complete_transitions_to_completed() {
        let mut agent = test_agent();
        agent.handle(AgentCommand::Activate).unwrap();
        let _events = agent.handle(AgentCommand::Complete).unwrap();
        assert_eq!(agent.state, AgentState::Completed);
    }

    #[test]
    fn active_fail_transitions_to_failed() {
        let mut agent = test_agent();
        agent.handle(AgentCommand::Activate).unwrap();
        let _events = agent
            .handle(AgentCommand::Fail {
                reason: "err".into(),
            })
            .unwrap();
        assert_eq!(agent.state, AgentState::Failed);
    }

    #[test]
    fn paused_resume_transitions_to_active() {
        let mut agent = test_agent();
        agent.handle(AgentCommand::Activate).unwrap();
        agent.handle(AgentCommand::Pause).unwrap();
        let _events = agent.handle(AgentCommand::Resume).unwrap();
        assert_eq!(agent.state, AgentState::Active);
        assert_eq!(agent.directive, Directive::Continue);
    }

    #[test]
    fn paused_kill_transitions_to_exited() {
        let mut agent = test_agent();
        agent.handle(AgentCommand::Activate).unwrap();
        agent.handle(AgentCommand::Pause).unwrap();
        let _events = agent.handle(AgentCommand::Kill).unwrap();
        assert_eq!(agent.state, AgentState::Exited);
    }

    #[test]
    fn paused_fail_transitions_to_failed() {
        let mut agent = test_agent();
        agent.handle(AgentCommand::Activate).unwrap();
        agent.handle(AgentCommand::Pause).unwrap();
        let _events = agent
            .handle(AgentCommand::Fail {
                reason: "err".into(),
            })
            .unwrap();
        assert_eq!(agent.state, AgentState::Failed);
    }

    #[test]
    fn draining_rollback_transitions_to_restoring() {
        let mut agent = test_agent();
        agent.handle(AgentCommand::Activate).unwrap();
        agent.handle(AgentCommand::StartDraining).unwrap();
        let version = CheckpointVersion(1);
        let events = agent.handle(AgentCommand::Rollback { version }).unwrap();
        assert_eq!(agent.state, AgentState::Restoring);
        assert_eq!(agent.checkpoint_version, Some(version));
        assert_eq!(agent.directive, Directive::PrepareReset);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::CheckpointVersionSet { .. }))
        );
    }

    #[test]
    fn draining_complete_transitions_to_completed() {
        let mut agent = test_agent();
        agent.handle(AgentCommand::Activate).unwrap();
        agent.handle(AgentCommand::StartDraining).unwrap();
        let _events = agent.handle(AgentCommand::Complete).unwrap();
        assert_eq!(agent.state, AgentState::Completed);
    }

    #[test]
    fn draining_kill_transitions_to_exited() {
        let mut agent = test_agent();
        agent.handle(AgentCommand::Activate).unwrap();
        agent.handle(AgentCommand::StartDraining).unwrap();
        let _events = agent.handle(AgentCommand::Kill).unwrap();
        assert_eq!(agent.state, AgentState::Exited);
    }

    #[test]
    fn draining_fail_transitions_to_failed() {
        let mut agent = test_agent();
        agent.handle(AgentCommand::Activate).unwrap();
        agent.handle(AgentCommand::StartDraining).unwrap();
        let _events = agent
            .handle(AgentCommand::Fail {
                reason: "err".into(),
            })
            .unwrap();
        assert_eq!(agent.state, AgentState::Failed);
    }

    #[test]
    fn restoring_restored_transitions_to_active() {
        let mut agent = test_agent();
        agent.handle(AgentCommand::Activate).unwrap();
        agent.handle(AgentCommand::StartDraining).unwrap();
        agent
            .handle(AgentCommand::Rollback {
                version: CheckpointVersion(1),
            })
            .unwrap();
        let _events = agent.handle(AgentCommand::Restored).unwrap();
        assert_eq!(agent.state, AgentState::Active);
        assert_eq!(agent.directive, Directive::Continue);
    }

    #[test]
    fn restoring_kill_transitions_to_exited() {
        let mut agent = test_agent();
        agent.handle(AgentCommand::Activate).unwrap();
        agent.handle(AgentCommand::StartDraining).unwrap();
        agent
            .handle(AgentCommand::Rollback {
                version: CheckpointVersion(1),
            })
            .unwrap();
        let _events = agent.handle(AgentCommand::Kill).unwrap();
        assert_eq!(agent.state, AgentState::Exited);
    }

    #[test]
    fn restoring_fail_transitions_to_failed() {
        let mut agent = test_agent();
        agent.handle(AgentCommand::Activate).unwrap();
        agent.handle(AgentCommand::StartDraining).unwrap();
        agent
            .handle(AgentCommand::Rollback {
                version: CheckpointVersion(1),
            })
            .unwrap();
        let _events = agent
            .handle(AgentCommand::Fail {
                reason: "err".into(),
            })
            .unwrap();
        assert_eq!(agent.state, AgentState::Failed);
    }

    #[test]
    fn starting_rejects_pause() {
        let mut agent = test_agent();
        let err = agent.handle(AgentCommand::Pause).unwrap_err();
        assert!(matches!(
            err,
            TransitionError::InvalidTransition {
                from: AgentState::Starting,
                ..
            }
        ));
        assert_eq!(agent.state, AgentState::Starting);
    }

    #[test]
    fn starting_rejects_resume() {
        let mut agent = test_agent();
        let err = agent.handle(AgentCommand::Resume).unwrap_err();
        assert!(matches!(err, TransitionError::InvalidTransition { .. }));
    }

    #[test]
    fn starting_rejects_complete() {
        let mut agent = test_agent();
        let err = agent.handle(AgentCommand::Complete).unwrap_err();
        assert!(matches!(err, TransitionError::InvalidTransition { .. }));
    }

    #[test]
    fn active_rejects_activate() {
        let mut agent = test_agent();
        agent.handle(AgentCommand::Activate).unwrap();
        let err = agent.handle(AgentCommand::Activate).unwrap_err();
        assert!(matches!(
            err,
            TransitionError::InvalidTransition {
                from: AgentState::Active,
                ..
            }
        ));
    }

    #[test]
    fn active_rejects_resume() {
        let mut agent = test_agent();
        agent.handle(AgentCommand::Activate).unwrap();
        let err = agent.handle(AgentCommand::Resume).unwrap_err();
        assert!(matches!(err, TransitionError::InvalidTransition { .. }));
    }

    #[test]
    fn active_rejects_rollback() {
        let mut agent = test_agent();
        agent.handle(AgentCommand::Activate).unwrap();
        let err = agent
            .handle(AgentCommand::Rollback {
                version: CheckpointVersion(1),
            })
            .unwrap_err();
        assert!(matches!(err, TransitionError::InvalidTransition { .. }));
    }

    #[test]
    fn paused_rejects_pause() {
        let mut agent = test_agent();
        agent.handle(AgentCommand::Activate).unwrap();
        agent.handle(AgentCommand::Pause).unwrap();
        let err = agent.handle(AgentCommand::Pause).unwrap_err();
        assert!(matches!(
            err,
            TransitionError::InvalidTransition {
                from: AgentState::Paused,
                ..
            }
        ));
    }

    #[test]
    fn exited_rejects_all_commands() {
        let mut agent = test_agent();
        agent.handle(AgentCommand::Kill).unwrap();
        assert_eq!(agent.state, AgentState::Exited);

        let err = agent.handle(AgentCommand::Activate).unwrap_err();
        assert!(matches!(
            err,
            TransitionError::TerminalState {
                state: AgentState::Exited
            }
        ));

        let err = agent.handle(AgentCommand::Kill).unwrap_err();
        assert!(matches!(err, TransitionError::TerminalState { .. }));

        let err = agent
            .handle(AgentCommand::SetSession {
                session_id: "s".into(),
            })
            .unwrap_err();
        assert!(matches!(err, TransitionError::TerminalState { .. }));
    }

    #[test]
    fn completed_rejects_all_commands() {
        let mut agent = test_agent();
        agent.handle(AgentCommand::Activate).unwrap();
        agent.handle(AgentCommand::Complete).unwrap();
        assert_eq!(agent.state, AgentState::Completed);

        let err = agent.handle(AgentCommand::Kill).unwrap_err();
        assert!(matches!(
            err,
            TransitionError::TerminalState {
                state: AgentState::Completed
            }
        ));
    }

    #[test]
    fn failed_rejects_all_commands() {
        let mut agent = test_agent();
        agent.handle(AgentCommand::Activate).unwrap();
        agent
            .handle(AgentCommand::Fail {
                reason: "err".into(),
            })
            .unwrap();

        let err = agent.handle(AgentCommand::Resume).unwrap_err();
        assert!(matches!(
            err,
            TransitionError::TerminalState {
                state: AgentState::Failed
            }
        ));
    }

    #[test]
    fn set_session_in_starting_state() {
        let mut agent = test_agent();
        let events = agent
            .handle(AgentCommand::SetSession {
                session_id: "sess-1".into(),
            })
            .unwrap();
        assert_eq!(agent.session_id, Some("sess-1".to_string()));
        assert!(events.iter().any(
            |e| matches!(e, AgentEvent::SessionReady { session_id, .. } if session_id == "sess-1")
        ));
        assert_eq!(agent.state, AgentState::Starting);
    }

    #[test]
    fn set_session_in_active_state() {
        let mut agent = test_agent();
        agent.handle(AgentCommand::Activate).unwrap();
        let _events = agent
            .handle(AgentCommand::SetSession {
                session_id: "sess-2".into(),
            })
            .unwrap();
        assert_eq!(agent.session_id, Some("sess-2".to_string()));
        assert_eq!(agent.state, AgentState::Active);
    }

    #[test]
    fn set_session_in_paused_state() {
        let mut agent = test_agent();
        agent.handle(AgentCommand::Activate).unwrap();
        agent.handle(AgentCommand::Pause).unwrap();
        let _events = agent
            .handle(AgentCommand::SetSession {
                session_id: "sess-3".into(),
            })
            .unwrap();
        assert_eq!(agent.session_id, Some("sess-3".to_string()));
    }

    #[test]
    fn set_session_rejected_in_terminal_state() {
        let mut agent = test_agent();
        agent.handle(AgentCommand::Kill).unwrap();
        let err = agent
            .handle(AgentCommand::SetSession {
                session_id: "s".into(),
            })
            .unwrap_err();
        assert!(matches!(err, TransitionError::TerminalState { .. }));
    }

    #[test]
    fn agent_with_spawn_origin_user() {
        let agent = test_agent();
        assert!(matches!(agent.origin, SpawnOrigin::User));
        assert!(agent.children.is_empty());
    }

    #[test]
    fn agent_with_spawn_origin_agent() {
        let parent_id = AgentId::new();
        let agent = Agent::new(
            AgentId::new(),
            ObjectiveId::new(),
            PathBuf::from("/tmp/test"),
            SpawnOrigin::Agent(parent_id),
            None,
        );
        assert!(matches!(agent.origin, SpawnOrigin::Agent(id) if id == parent_id));
    }

    #[test]
    fn spawn_origin_spawned_by_returns_agent_id() {
        let parent = AgentId::new();
        assert_eq!(SpawnOrigin::Agent(parent).spawned_by(), Some(parent));
    }

    #[test]
    fn spawn_origin_user_spawned_by_returns_none() {
        assert_eq!(SpawnOrigin::User.spawned_by(), None);
    }
}

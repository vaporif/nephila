//! Production [`SessionDriver`] implementation that wraps an
//! `Arc<ClaudeCodeSession>`.
//!
//! Translates the supervisor's autonomy decisions into connector calls:
//!
//!   - `send_agent_prompt` → `session.send_turn(PromptSource::Agent, ..)`
//!   - `pause`             → `session.pause()` (SIGSTOP)
//!   - `shutdown`          → `session.shutdown_in_place()`
//!
//! Errors from the connector are mapped to [`DriverError`] variants:
//! `ConnectorError::Persist` → [`DriverError::Persist`],
//! `ConnectorError::Spawn` → [`DriverError::Io`], everything else is
//! collapsed to [`DriverError::ProcessGone`] since the supervisor's only
//! response is to mark the session crashed regardless.

use std::sync::Arc;

use nephila_connector::ConnectorError;
use nephila_connector::session::{ClaudeCodeSession, PromptSource};

use crate::session_supervisor::{DriverError, SessionDriver};

#[derive(Clone)]
pub struct ClaudeCodeDriver {
    session: Arc<ClaudeCodeSession>,
}

impl std::fmt::Debug for ClaudeCodeDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaudeCodeDriver")
            .field("session_id", &self.session.id())
            .field("agent_id", &self.session.agent_id())
            .finish()
    }
}

impl ClaudeCodeDriver {
    #[must_use]
    pub const fn new(session: Arc<ClaudeCodeSession>) -> Self {
        Self { session }
    }
}

fn map_connector_error(e: ConnectorError) -> DriverError {
    match e {
        ConnectorError::Persist(p) => DriverError::Persist(p),
        ConnectorError::Spawn(io) => DriverError::Io(io),
        // The supervisor treats every other connector error the same way
        // (transition to Crashed, log) — collapse into ProcessGone with the
        // detail captured in the warn-log below.
        other => {
            tracing::warn!(error = %other, "ClaudeCodeDriver: connector error mapped to ProcessGone");
            DriverError::ProcessGone
        }
    }
}

impl SessionDriver for ClaudeCodeDriver {
    async fn send_agent_prompt(&self, prompt: String) -> Result<(), DriverError> {
        self.session
            .send_turn(PromptSource::Agent, prompt)
            .await
            .map(|_turn_id| ())
            .map_err(map_connector_error)
    }

    async fn pause(&self) -> Result<(), DriverError> {
        self.session.pause().await.map_err(map_connector_error)
    }

    async fn shutdown(&self) -> Result<(), DriverError> {
        self.session
            .shutdown_in_place()
            .await
            .map_err(map_connector_error)
    }
}

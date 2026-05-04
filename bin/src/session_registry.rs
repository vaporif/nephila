//! Stub `SessionRegistry` — slice 3 placeholder.
//!
//! Slice 3 only needs a way for `SessionSupervisor` to learn about new
//! sessions as they spin up. Slice 4 (Task 6 step 3) will fill this in with
//! `ensure_session(agent)`, `on_crash(...)`, a `DashMap` of live sessions, and
//! a cross-process lockfile owner. The stub lives in this file so slice 4
//! only ADDS methods to an existing struct — never moves code between files.
//!
//! The single public surface today is:
//!   - `subscribe_session_started()` — supervisor subscribes here in its
//!     `run()` loop to learn about newly-spawned sessions.
//!   - `fire_started(agent_id)` — test seam used by integration tests; in
//!     slice 4 this becomes the body of `ensure_session(agent)`.

use nephila_core::id::AgentId;
use tokio::sync::broadcast;

const NEW_SESSION_CHANNEL_BOUND: usize = 64;

#[derive(Debug)]
pub struct SessionRegistry {
    started_tx: broadcast::Sender<AgentId>,
}

impl Default for SessionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionRegistry {
    #[must_use]
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(NEW_SESSION_CHANNEL_BOUND);
        Self { started_tx: tx }
    }

    /// Subscribe to "a new session has started" notifications.
    ///
    /// Each `recv` yields the `AgentId` whose session was just spawned. The
    /// supervisor uses this to dynamically attach a per-session subscription
    /// to its `JoinSet`.
    pub fn subscribe_session_started(&self) -> broadcast::Receiver<AgentId> {
        self.started_tx.subscribe()
    }

    /// Test seam — the property test calls this directly.
    ///
    /// Slice 4 (Task 6 step 3) replaces this with `ensure_session(agent)`
    /// which calls `started_tx.send(agent.id)` internally. This stub is here
    /// so slice 4 only adds methods, never moves code between files.
    #[allow(dead_code)] // wired in slice 4
    pub fn fire_started(&self, agent_id: AgentId) {
        let _ = self.started_tx.send(agent_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fire_then_recv_returns_agent_id() {
        let reg = SessionRegistry::new();
        let mut rx = reg.subscribe_session_started();
        let agent = AgentId::new();
        reg.fire_started(agent);
        assert_eq!(rx.recv().await.expect("recv"), agent);
    }
}

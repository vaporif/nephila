//! `Agent` reducer applies `AgentSessionAssigned` and
//! `AgentConfigSnapshotted` correctly.
//!
//! - Fresh agent has `session_id == None` and `last_config_snapshot == None`.
//! - After `AgentSessionAssigned`, `agent.session_id == Some(<uuid_str>)`.
//! - After `AgentConfigSnapshotted`, `agent.last_config_snapshot == Some(snapshot)`.

use std::path::PathBuf;

use chrono::Utc;
use nephila_core::agent::{Agent, AgentConfigSnapshot, AgentEvent, SpawnOrigin};
use nephila_core::id::{AgentId, ObjectiveId};
use nephila_core::session_event::SessionId;

fn fresh_agent() -> Agent {
    Agent::new(
        AgentId::new(),
        ObjectiveId::new(),
        PathBuf::from("/tmp/test"),
        SpawnOrigin::Operator,
        None,
    )
}

#[test]
fn fresh_agent_has_no_session_id_or_config_snapshot() {
    let agent = fresh_agent();
    assert!(agent.session_id.is_none());
    assert!(agent.last_config_snapshot.is_none());
}

#[test]
fn agent_session_assigned_sets_session_id() {
    let agent = fresh_agent();
    let agent_id = agent.id;
    let sid: SessionId = uuid::Uuid::new_v4();

    let agent = agent.apply_event(&AgentEvent::AgentSessionAssigned {
        agent_id,
        session_id: sid,
        ts: Utc::now(),
    });

    assert_eq!(agent.session_id.as_deref(), Some(sid.to_string().as_str()));
}

#[test]
fn agent_config_snapshotted_sets_last_config_snapshot() {
    let agent = fresh_agent();
    let agent_id = agent.id;
    let snap = AgentConfigSnapshot {
        working_dir: PathBuf::from("/work"),
        mcp_endpoint: "http://stub".into(),
        permission_mode: "bypassPermissions".into(),
        claude_binary: PathBuf::from("/usr/local/bin/claude"),
    };

    let agent = agent.apply_event(&AgentEvent::AgentConfigSnapshotted {
        agent_id,
        snapshot: snap.clone(),
        ts: Utc::now(),
    });

    assert_eq!(agent.last_config_snapshot.as_ref(), Some(&snap));
}

#[test]
fn applying_session_assigned_then_config_snapshot_preserves_both() {
    let agent = fresh_agent();
    let agent_id = agent.id;
    let sid: SessionId = uuid::Uuid::new_v4();
    let snap = AgentConfigSnapshot {
        working_dir: PathBuf::from("/work"),
        mcp_endpoint: "http://stub".into(),
        permission_mode: "bypassPermissions".into(),
        claude_binary: PathBuf::from("/usr/local/bin/claude"),
    };

    let agent = agent
        .apply_event(&AgentEvent::AgentSessionAssigned {
            agent_id,
            session_id: sid,
            ts: Utc::now(),
        })
        .apply_event(&AgentEvent::AgentConfigSnapshotted {
            agent_id,
            snapshot: snap.clone(),
            ts: Utc::now(),
        });

    assert_eq!(agent.session_id.as_deref(), Some(sid.to_string().as_str()));
    assert_eq!(agent.last_config_snapshot.as_ref(), Some(&snap));
}

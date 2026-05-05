use nephila_core::command::OrchestratorCommand;
use nephila_core::id::{AgentId, ObjectiveId};

#[test]
fn debug_redacts_spawn_content() {
    let cmd = OrchestratorCommand::Spawn {
        objective_id: ObjectiveId::new(),
        content: "MY_API_KEY=sk-secret-xyz".into(),
        dir: std::path::PathBuf::from("/tmp"),
        restore_checkpoint_id: None,
    };
    let s = format!("{cmd:?}");
    assert!(
        !s.contains("sk-secret-xyz"),
        "Debug must not leak Spawn.content; got: {s}"
    );
    assert!(
        s.contains("Spawn"),
        "Debug should still name the variant; got: {s}"
    );
    assert!(
        s.contains("bytes") || s.contains("redacted"),
        "Debug should indicate redaction; got: {s}"
    );
}

#[test]
fn debug_redacts_spawn_agent_content() {
    let cmd = OrchestratorCommand::SpawnAgent {
        objective_id: ObjectiveId::new(),
        content: "INNER_SECRET_TOKEN".into(),
        dir: std::path::PathBuf::from("/tmp"),
        spawned_by: AgentId::new(),
    };
    let s = format!("{cmd:?}");
    assert!(
        !s.contains("INNER_SECRET_TOKEN"),
        "Debug must not leak SpawnAgent.content; got: {s}"
    );
}

#[test]
fn debug_redacts_respawn_content() {
    use nephila_core::id::CheckpointId;
    let cmd = OrchestratorCommand::Spawn {
        objective_id: ObjectiveId::new(),
        content: "RESPAWN_PAYLOAD_SECRET".into(),
        dir: std::path::PathBuf::from("/tmp"),
        restore_checkpoint_id: Some(CheckpointId::new()),
    };
    let s = format!("{cmd:?}");
    assert!(
        !s.contains("RESPAWN_PAYLOAD_SECRET"),
        "Debug must not leak Spawn.content (with restore_checkpoint_id); got: {s}"
    );
}

#[test]
fn debug_redacts_hitl_response() {
    let cmd = OrchestratorCommand::HitlRespond {
        agent_id: AgentId::new(),
        response: "BEARER_TOKEN_HERE".into(),
    };
    let s = format!("{cmd:?}");
    assert!(
        !s.contains("BEARER_TOKEN_HERE"),
        "Debug must not leak HitlRespond.response; got: {s}"
    );
}

#[test]
fn debug_passes_through_pure_id_variants() {
    let id = AgentId::new();
    let cmd = OrchestratorCommand::Kill { agent_id: id };
    let s = format!("{cmd:?}");
    assert!(s.contains("Kill"), "Debug should name the variant: {s}");
    assert!(
        s.contains(&format!("{id}")),
        "Debug should pass through agent_id: {s}"
    );
}

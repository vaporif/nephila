use meridian_core::agent::{Agent, AgentState, SpawnOrigin};
use meridian_core::checkpoint::{ChannelEntry, CheckpointNode, L2Chunk, ReducerKind};
use meridian_core::directive::Directive;
use meridian_core::id::*;
use meridian_core::memory::{Embedding, LifecycleState, MemoryEntry};
use meridian_core::objective::{NewObjective, ObjectiveStatus};
use meridian_core::store::*;
use meridian_store::SqliteStore;
use std::collections::BTreeMap;
use std::path::PathBuf;

const DIM: usize = 8;

fn make_embedding(val: f32) -> Vec<f32> {
    vec![val; DIM]
}

fn make_channels() -> BTreeMap<String, ChannelEntry> {
    BTreeMap::from([
        (
            "objectives".into(),
            ChannelEntry {
                reducer: ReducerKind::Overwrite,
                value: serde_json::json!([{"id": "obj1", "description": "Implement auth", "status": "in_progress"}]),
            },
        ),
        (
            "progress_summary".into(),
            ChannelEntry {
                reducer: ReducerKind::Overwrite,
                value: serde_json::json!(
                    "Session focused on auth middleware. Created JWT validation."
                ),
            },
        ),
        (
            "decisions".into(),
            ChannelEntry {
                reducer: ReducerKind::Append,
                value: serde_json::json!(["Use JWT for auth"]),
            },
        ),
        (
            "blockers".into(),
            ChannelEntry {
                reducer: ReducerKind::Append,
                value: serde_json::json!([]),
            },
        ),
    ])
}

#[tokio::test]
async fn test_full_context_reset_loop() {
    let store = SqliteStore::open_in_memory(DIM).unwrap();

    let obj_id = ObjectiveStore::create(
        &store,
        NewObjective {
            parent_id: None,
            agent_id: None,
            description: "Implement authentication system".into(),
        },
    )
    .await
    .unwrap();

    let agent_id = AgentId::new();
    let agent = Agent::new(
        agent_id,
        obj_id,
        PathBuf::from("/tmp/agent-1"),
        SpawnOrigin::Operator,
        None,
    );
    AgentStore::register(&store, agent).await.unwrap();
    ObjectiveStore::assign_agent(&store, obj_id, agent_id)
        .await
        .unwrap();
    AgentStore::update_state(&store, agent_id, AgentState::Active)
        .await
        .unwrap();

    let directive = AgentStore::get_directive(&store, agent_id).await.unwrap();
    assert_eq!(directive, Directive::Continue);

    ObjectiveStore::update_status(&store, obj_id, ObjectiveStatus::InProgress)
        .await
        .unwrap();

    AgentStore::set_directive(&store, agent_id, Directive::PrepareReset)
        .await
        .unwrap();
    assert_eq!(
        AgentStore::get_directive(&store, agent_id).await.unwrap(),
        Directive::PrepareReset
    );
    AgentStore::update_state(&store, agent_id, AgentState::Suspending)
        .await
        .unwrap();

    let l2_chunks = vec![
        L2Chunk {
            id: EntryId::new(),
            content: "Created auth middleware with JWT parsing".into(),
            tags: vec!["auth".into(), "middleware".into()],
        },
        L2Chunk {
            id: EntryId::new(),
            content: "Added protected route decorator for API endpoints".into(),
            tags: vec!["auth".into(), "routes".into()],
        },
    ];
    let l2_embeddings: Vec<Embedding> = vec![make_embedding(0.2), make_embedding(0.3)];

    let node = CheckpointNode {
        id: CheckpointId::new(),
        agent_id,
        parent_id: None,
        branch_label: None,
        channels: make_channels(),
        l2_namespace: "general".into(),
        interrupt: None,
        created_at: chrono::Utc::now(),
    };
    let node_id = node.id;

    CheckpointStore::save(&store, &node, &l2_chunks, &l2_embeddings)
        .await
        .unwrap();
    AgentStore::set_checkpoint_id(&store, agent_id, node_id)
        .await
        .unwrap();

    for (chunk, emb) in l2_chunks.iter().zip(l2_embeddings.iter()) {
        let entry = MemoryEntry {
            id: chunk.id,
            agent_id,
            content: chunk.content.clone(),
            embedding: emb.clone(),
            tags: chunk.tags.clone(),
            lifecycle_state: LifecycleState::Generated,
            importance: 0.5,
            access_count: 0,
            created_at: chrono::Utc::now(),
        };
        MemoryStore::store(&store, entry).await.unwrap();
    }

    AgentStore::update_state(&store, agent_id, AgentState::Exited)
        .await
        .unwrap();
    let agent_record = AgentStore::get(&store, agent_id).await.unwrap().unwrap();
    assert_eq!(agent_record.state, AgentState::Exited);
    assert_eq!(agent_record.checkpoint_id, Some(node_id));

    let new_agent_id = AgentId::new();
    let mut new_agent = Agent::new(
        new_agent_id,
        obj_id,
        PathBuf::from("/tmp/agent-2"),
        SpawnOrigin::Operator,
        None,
    );
    new_agent.restore_checkpoint_id = Some(node_id);
    AgentStore::register(&store, new_agent).await.unwrap();

    let checkpoint = CheckpointStore::get_latest(&store, agent_id)
        .await
        .unwrap()
        .expect("checkpoint should exist");
    assert_eq!(checkpoint.id, node_id);
    assert!(checkpoint.channels.contains_key("objectives"));
    assert!(checkpoint.channels.contains_key("progress_summary"));

    let query_embedding = make_embedding(0.2);
    let results = MemoryStore::search(&store, &query_embedding, 5)
        .await
        .unwrap();
    assert!(!results.is_empty(), "memory search should return results");

    AgentStore::update_state(&store, new_agent_id, AgentState::Active)
        .await
        .unwrap();
    let directive = AgentStore::get_directive(&store, new_agent_id)
        .await
        .unwrap();
    assert_eq!(directive, Directive::Continue);

    let tree = ObjectiveStore::get_tree(&store, obj_id).await.unwrap();
    assert_eq!(tree.root.status, ObjectiveStatus::InProgress);
}

#[tokio::test]
async fn test_checkpoint_tree_ancestry() {
    let store = SqliteStore::open_in_memory(DIM).unwrap();

    let obj_id = ObjectiveStore::create(
        &store,
        NewObjective {
            parent_id: None,
            agent_id: None,
            description: "Test objective".into(),
        },
    )
    .await
    .unwrap();

    let agent_id = AgentId::new();
    AgentStore::register(
        &store,
        Agent::new(
            agent_id,
            obj_id,
            PathBuf::from("/tmp/test"),
            SpawnOrigin::Operator,
            None,
        ),
    )
    .await
    .unwrap();

    let n1 = CheckpointNode {
        id: CheckpointId::new(),
        agent_id,
        parent_id: None,
        branch_label: None,
        channels: make_channels(),
        l2_namespace: "general".into(),
        interrupt: None,
        created_at: chrono::Utc::now(),
    };
    CheckpointStore::save(&store, &n1, &[], &[]).await.unwrap();

    let n2 = CheckpointNode {
        id: CheckpointId::new(),
        agent_id,
        parent_id: Some(n1.id),
        branch_label: None,
        channels: make_channels(),
        l2_namespace: "general".into(),
        interrupt: None,
        created_at: chrono::Utc::now(),
    };
    CheckpointStore::save(&store, &n2, &[], &[]).await.unwrap();

    let latest = CheckpointStore::get_latest(&store, agent_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(latest.id, n2.id);

    let ancestry = CheckpointStore::get_ancestry(&store, n2.id).await.unwrap();
    assert_eq!(ancestry.len(), 2);
    assert_eq!(ancestry[0].id, n1.id);
    assert_eq!(ancestry[1].id, n2.id);
}

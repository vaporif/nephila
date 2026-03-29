use meridian_core::agent::{AgentRecord, AgentState};
use meridian_core::checkpoint::{L0State, L2Chunk, ObjectiveSnapshot};
use meridian_core::directive::Directive;
use meridian_core::id::*;
use meridian_core::memory::{Embedding, LifecycleState, MemoryEntry};
use meridian_core::objective::{NewObjective, ObjectiveStatus};
use meridian_core::store::*;
use meridian_store::SqliteStore;
use std::path::PathBuf;

const DIM: usize = 8;

fn make_embedding(val: f32) -> Vec<f32> {
    vec![val; DIM]
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
    let agent = AgentRecord {
        id: agent_id,
        state: AgentState::Starting,
        directory: PathBuf::from("/tmp/agent-1"),
        objective_id: obj_id,
        checkpoint_version: None,
        spawned_by: None,
        injected_message: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
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
    AgentStore::update_state(&store, agent_id, AgentState::Draining)
        .await
        .unwrap();

    let l0 = L0State {
        objectives: vec![ObjectiveSnapshot {
            id: obj_id,
            description: "Implement authentication system".into(),
            status: "in_progress".into(),
        }],
        next_steps: vec![
            "Add JWT token validation".into(),
            "Write tests for auth middleware".into(),
        ],
    };
    let l1 = "Session focused on auth middleware. Created JWT validation, added route protection. Next: token validation and test coverage.".to_string();
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

    let version = CheckpointVersion(1);
    CheckpointStore::save(&store, agent_id, version, &l0, &l1, &l2_chunks, &l2_embeddings)
        .await
        .unwrap();
    AgentStore::set_checkpoint_version(&store, agent_id, version)
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
    assert_eq!(agent_record.checkpoint_version, Some(CheckpointVersion(1)));

    let new_agent_id = AgentId::new();
    let new_agent = AgentRecord {
        id: new_agent_id,
        state: AgentState::Restoring,
        directory: PathBuf::from("/tmp/agent-2"),
        objective_id: obj_id,
        checkpoint_version: Some(version),
        spawned_by: None,
        injected_message: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    AgentStore::register(&store, new_agent).await.unwrap();

    let checkpoint = CheckpointStore::get_latest(&store, agent_id)
        .await
        .unwrap()
        .expect("checkpoint should exist");
    assert_eq!(checkpoint.version, CheckpointVersion(1));
    assert_eq!(checkpoint.l0.objectives.len(), 1);
    assert_eq!(checkpoint.l0.next_steps.len(), 2);
    assert!(checkpoint.l1.contains("JWT validation"));

    let query_embedding = make_embedding(0.2);
    let results = MemoryStore::search(&store, &query_embedding, 5).await.unwrap();
    assert!(!results.is_empty(), "memory search should return results");

    AgentStore::update_state(&store, new_agent_id, AgentState::Active)
        .await
        .unwrap();
    let directive = AgentStore::get_directive(&store, new_agent_id).await.unwrap();
    assert_eq!(directive, Directive::Continue);

    let tree = ObjectiveStore::get_tree(&store, obj_id).await.unwrap();
    assert_eq!(tree.root.status, ObjectiveStatus::InProgress);
}

#[tokio::test]
async fn test_multiple_checkpoint_versions() {
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
        AgentRecord {
            id: agent_id,
            state: AgentState::Active,
            directory: PathBuf::from("/tmp/test"),
            objective_id: obj_id,
            checkpoint_version: None,
            spawned_by: None,
            injected_message: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        },
    )
    .await
    .unwrap();

    for v in 1..=2u32 {
        let l0 = L0State {
            objectives: vec![],
            next_steps: vec![format!("step from v{v}")],
        };
        CheckpointStore::save(
            &store,
            agent_id,
            CheckpointVersion(v),
            &l0,
            &format!("L1 content for v{v}"),
            &[],
            &[],
        )
        .await
        .unwrap();
    }

    let latest = CheckpointStore::get_latest(&store, agent_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(latest.version, CheckpointVersion(2));
    assert!(latest.l1.contains("v2"));

    let v1 = CheckpointStore::get_version(&store, agent_id, CheckpointVersion(1))
        .await
        .unwrap()
        .unwrap();
    assert!(v1.l1.contains("v1"));

    let versions = CheckpointStore::list_versions(&store, agent_id).await.unwrap();
    assert_eq!(versions, vec![CheckpointVersion(2), CheckpointVersion(1)]);
}

#[tokio::test]
async fn test_hitl_tracking() {
    let store = SqliteStore::open_in_memory(DIM).unwrap();

    let obj_id = ObjectiveStore::create(
        &store,
        NewObjective {
            parent_id: None,
            agent_id: None,
            description: "Test".into(),
        },
    )
    .await
    .unwrap();

    let agent_id = AgentId::new();
    AgentStore::register(
        &store,
        AgentRecord {
            id: agent_id,
            state: AgentState::Active,
            directory: PathBuf::from("/tmp/test"),
            objective_id: obj_id,
            checkpoint_version: None,
            spawned_by: None,
            injected_message: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        },
    )
    .await
    .unwrap();

    assert_eq!(HitlStore::record_ask(&store, agent_id, 12345).await.unwrap(), 1);
    assert_eq!(HitlStore::record_ask(&store, agent_id, 12345).await.unwrap(), 2);
    assert_eq!(HitlStore::record_ask(&store, agent_id, 99999).await.unwrap(), 1);
    assert_eq!(HitlStore::get_ask_count(&store, agent_id, 12345).await.unwrap(), 2);
    assert_eq!(HitlStore::get_ask_count(&store, agent_id, 11111).await.unwrap(), 0);
}

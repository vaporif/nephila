use chrono::Utc;
use meridian_core::id::AgentId;
use meridian_core::ObjectiveId;

use crate::SqliteStore;

pub const TEST_DIM: usize = 384;

pub fn make_embedding(val: f32) -> Vec<f32> {
    vec![val; TEST_DIM]
}

pub async fn make_agent_and_store() -> (SqliteStore, AgentId) {
    let store = SqliteStore::open_in_memory(TEST_DIM).unwrap();
    let agent_id = AgentId::new();
    let obj_id = ObjectiveId::new();
    store
        .writer
        .execute(move |conn| {
            let now = Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO agents (id, state, directive, directory, objective_id, created_at, updated_at)
                 VALUES (?1, 'starting', 'continue', '/tmp', ?2, ?3, ?4)",
                rusqlite::params![agent_id, obj_id, &now, &now],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    (store, agent_id)
}

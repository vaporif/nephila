use crate::SqliteStore;
use crate::util::{f32_slice_to_bytes, parse_rfc3339};
use chrono::Utc;
use meridian_core::checkpoint::{Checkpoint, L0State, L2Chunk};
use meridian_core::id::{AgentId, CheckpointVersion};
use meridian_core::memory::Embedding;
use meridian_core::store::CheckpointStore;

impl CheckpointStore for SqliteStore {
    async fn save(
        &self,
        agent_id: AgentId,
        version: CheckpointVersion,
        l0: &L0State,
        l1: &str,
        l2_chunks: &[L2Chunk],
        l2_embeddings: &[Embedding],
    ) -> meridian_core::Result<()> {
        let l0_json = serde_json::to_string(l0)?;
        let l1_owned = l1.to_string();
        let l2_data: Vec<(String, Vec<u8>)> = l2_chunks
            .iter()
            .zip(l2_embeddings.iter())
            .map(|(chunk, emb)| {
                let content =
                    serde_json::to_string(chunk).map_err(meridian_core::MeridianError::from)?;
                let bytes = f32_slice_to_bytes(emb);
                Ok((content, bytes))
            })
            .collect::<meridian_core::Result<Vec<_>>>()?;

        self.writer
            .execute(move |conn| {
                let now = Utc::now().to_rfc3339();
                let tx = conn.unchecked_transaction()?;

                tx.execute(
                    "INSERT INTO checkpoints (agent_id, version, layer, content, created_at)
                     VALUES (?1, ?2, 'l0', ?3, ?4)",
                    rusqlite::params![
                        agent_id,
                        version,
                        l0_json,
                        now,
                    ],
                )?;

                tx.execute(
                    "INSERT INTO checkpoints (agent_id, version, layer, content, created_at)
                     VALUES (?1, ?2, 'l1', ?3, ?4)",
                    rusqlite::params![
                        agent_id,
                        version,
                        l1_owned,
                        now,
                    ],
                )?;

                for (idx, (content, emb_bytes)) in l2_data.iter().enumerate() {
                    let layer = format!("l2_{idx}");
                    tx.execute(
                        "INSERT INTO checkpoints (agent_id, version, layer, content, embedding, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        rusqlite::params![
                            agent_id,
                            version,
                            layer,
                            content,
                            emb_bytes,
                            now,
                        ],
                    )?;
                }

                tx.commit()?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    async fn get_latest(&self, agent_id: AgentId) -> meridian_core::Result<Option<Checkpoint>> {
        let checkpoint = self
            .writer
            .execute(move |conn| {
                let max_version: Option<i64> = conn.query_row(
                    "SELECT MAX(version) FROM checkpoints WHERE agent_id = ?1",
                    rusqlite::params![agent_id],
                    |row| row.get(0),
                )?;

                let version = match max_version {
                    Some(v) => v,
                    None => return Ok(None),
                };

                load_checkpoint(conn, agent_id, CheckpointVersion(version as u32))
            })
            .await?;
        Ok(checkpoint)
    }

    async fn get_version(
        &self,
        agent_id: AgentId,
        version: CheckpointVersion,
    ) -> meridian_core::Result<Option<Checkpoint>> {
        let checkpoint = self
            .writer
            .execute(move |conn| load_checkpoint(conn, agent_id, version))
            .await?;
        Ok(checkpoint)
    }

    async fn list_versions(
        &self,
        agent_id: AgentId,
    ) -> meridian_core::Result<Vec<CheckpointVersion>> {
        let versions = self
            .writer
            .execute(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT DISTINCT version FROM checkpoints WHERE agent_id = ?1 ORDER BY version DESC",
                )?;
                let rows = stmt
                    .query_map(rusqlite::params![agent_id], |row| {
                        row.get(0)
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await?;
        Ok(versions)
    }
}

fn load_checkpoint(
    conn: &rusqlite::Connection,
    agent_id: AgentId,
    version: CheckpointVersion,
) -> Result<Option<Checkpoint>, rusqlite::Error> {
    let l0_row: Option<(String, String)> = {
        let mut stmt = conn.prepare(
            "SELECT content, created_at FROM checkpoints WHERE agent_id = ?1 AND version = ?2 AND layer = 'l0'",
        )?;
        match stmt.query_row(rusqlite::params![agent_id, version], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        }) {
            Ok(r) => Some(r),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => return Err(e),
        }
    };

    let (l0_content, created_at_str) = match l0_row {
        Some(r) => r,
        None => return Ok(None),
    };

    let l1_content: String = conn.query_row(
        "SELECT content FROM checkpoints WHERE agent_id = ?1 AND version = ?2 AND layer = 'l1'",
        rusqlite::params![agent_id, version],
        |row| row.get(0),
    )?;

    let l0: L0State = serde_json::from_str(&l0_content).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let timestamp = parse_rfc3339(&created_at_str)?;

    Ok(Some(Checkpoint {
        agent_id,
        version,
        l0,
        l1: l1_content,
        timestamp,
    }))
}

#[cfg(test)]
mod tests {
    use crate::SqliteStore;
    use chrono::Utc;
    use meridian_core::checkpoint::{L0State, L2Chunk, ObjectiveSnapshot};
    use meridian_core::id::{AgentId, CheckpointVersion, EntryId, ObjectiveId};
    use meridian_core::store::CheckpointStore;

    fn make_l0() -> L0State {
        L0State {
            objectives: vec![ObjectiveSnapshot {
                id: ObjectiveId::new(),
                description: "test objective".to_string(),
                status: "pending".to_string(),
            }],
            next_steps: vec!["step 1".to_string()],
        }
    }

    async fn make_agent_with_objective(store: &SqliteStore) -> AgentId {
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
        agent_id
    }

    #[tokio::test]
    async fn save_and_get_latest() {
        let store = SqliteStore::open_in_memory(384).unwrap();
        let agent_id = make_agent_with_objective(&store).await;

        let l0 = make_l0();
        let l1 = "This is the L1 narrative summary.";
        let l2 = vec![L2Chunk {
            id: EntryId::new(),
            content: "chunk content".to_string(),
            tags: vec!["tag1".to_string()],
        }];
        let embedding = vec![0.1f32; 384];

        store
            .save(agent_id, CheckpointVersion(1), &l0, l1, &l2, &[embedding])
            .await
            .unwrap();

        let cp = store.get_latest(agent_id).await.unwrap().unwrap();
        assert_eq!(cp.version, CheckpointVersion(1));
        assert_eq!(cp.l1, "This is the L1 narrative summary.");
        assert_eq!(cp.l0.objectives.len(), 1);
    }

    #[tokio::test]
    async fn multiple_versions() {
        let store = SqliteStore::open_in_memory(384).unwrap();
        let agent_id = make_agent_with_objective(&store).await;

        let l0 = make_l0();
        store
            .save(agent_id, CheckpointVersion(1), &l0, "v1 text", &[], &[])
            .await
            .unwrap();
        store
            .save(agent_id, CheckpointVersion(2), &l0, "v2 text", &[], &[])
            .await
            .unwrap();

        let latest = store.get_latest(agent_id).await.unwrap().unwrap();
        assert_eq!(latest.version, CheckpointVersion(2));
        assert_eq!(latest.l1, "v2 text");

        let v1 = store
            .get_version(agent_id, CheckpointVersion(1))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(v1.l1, "v1 text");
    }

    #[tokio::test]
    async fn list_versions_test() {
        let store = SqliteStore::open_in_memory(384).unwrap();
        let agent_id = make_agent_with_objective(&store).await;

        let l0 = make_l0();
        store
            .save(agent_id, CheckpointVersion(1), &l0, "v1", &[], &[])
            .await
            .unwrap();
        store
            .save(agent_id, CheckpointVersion(3), &l0, "v3", &[], &[])
            .await
            .unwrap();
        store
            .save(agent_id, CheckpointVersion(2), &l0, "v2", &[], &[])
            .await
            .unwrap();

        let versions = store.list_versions(agent_id).await.unwrap();
        assert_eq!(
            versions,
            vec![
                CheckpointVersion(3),
                CheckpointVersion(2),
                CheckpointVersion(1),
            ]
        );
    }

    #[tokio::test]
    async fn get_latest_no_checkpoints_returns_none() {
        let store = SqliteStore::open_in_memory(384).unwrap();
        let agent_id = make_agent_with_objective(&store).await;
        let result = store.get_latest(agent_id).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn get_version_nonexistent_returns_none() {
        let store = SqliteStore::open_in_memory(384).unwrap();
        let agent_id = make_agent_with_objective(&store).await;
        let result = store
            .get_version(agent_id, CheckpointVersion(99))
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn list_versions_empty() {
        let store = SqliteStore::open_in_memory(384).unwrap();
        let agent_id = make_agent_with_objective(&store).await;
        let versions = store.list_versions(agent_id).await.unwrap();
        assert!(versions.is_empty());
    }
}

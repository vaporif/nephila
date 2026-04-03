use crate::SqliteStore;
use crate::util::{f32_slice_to_bytes, parse_rfc3339};
use chrono::Utc;
use nephila_core::checkpoint::{
    ChannelEntry, CheckpointNode, InterruptSnapshot, L2Chunk, L2SearchResult,
};
use nephila_core::id::{AgentId, CheckpointId, EntryId};
use nephila_core::memory::Embedding;
use nephila_core::store::CheckpointStore;
use std::collections::BTreeMap;

impl CheckpointStore for SqliteStore {
    async fn save(
        &self,
        node: &CheckpointNode,
        l2_chunks: &[L2Chunk],
        l2_embeddings: &[Embedding],
    ) -> nephila_core::Result<()> {
        let id = node.id;
        let agent_id = node.agent_id;
        let parent_id = node.parent_id;
        let branch_label = node.branch_label.clone();
        let channels_json =
            serde_json::to_string(&node.channels).map_err(nephila_core::NephilaError::from)?;
        let l2_namespace = node.l2_namespace.clone();
        let interrupt_json = node
            .interrupt
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(nephila_core::NephilaError::from)?;
        let created_at = node.created_at.to_rfc3339();

        let l2_data: Vec<(EntryId, String, String, String, Vec<u8>)> = l2_chunks
            .iter()
            .zip(l2_embeddings.iter())
            .map(|(chunk, emb)| {
                let tags_json = serde_json::to_string(&chunk.tags)
                    .map_err(nephila_core::NephilaError::from)?;
                let bytes = f32_slice_to_bytes(emb);
                Ok((
                    chunk.id,
                    chunk.content.clone(),
                    tags_json,
                    l2_namespace.clone(),
                    bytes,
                ))
            })
            .collect::<nephila_core::Result<Vec<_>>>()?;

        self.writer
            .execute(move |conn| {
                let tx = conn.unchecked_transaction()?;

                tx.execute(
                    "INSERT INTO checkpoints (id, agent_id, parent_id, branch_label, channels, l2_namespace, interrupt, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    rusqlite::params![
                        id,
                        agent_id,
                        parent_id,
                        branch_label,
                        channels_json,
                        l2_namespace,
                        interrupt_json,
                        created_at,
                    ],
                )?;

                let now = Utc::now().to_rfc3339();
                for (chunk_id, content, tags, ns, emb_bytes) in &l2_data {
                    tx.execute(
                        "INSERT INTO l2_chunks (id, checkpoint_id, agent_id, namespace, content, tags, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        rusqlite::params![chunk_id, id, agent_id, ns, content, tags, now],
                    )?;

                    tx.execute(
                        "INSERT INTO vec_l2_chunks (chunk_id, embedding) VALUES (?1, ?2)",
                        rusqlite::params![chunk_id.to_string(), emb_bytes],
                    )?;
                }

                tx.commit()?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    async fn get(&self, id: CheckpointId) -> nephila_core::Result<Option<CheckpointNode>> {
        let node = self.writer.execute(move |conn| load_node(conn, id)).await?;
        Ok(node)
    }

    async fn get_latest(&self, agent_id: AgentId) -> nephila_core::Result<Option<CheckpointNode>> {
        let node = self
            .writer
            .execute(move |conn| {
                let id: Option<CheckpointId> = match conn.query_row(
                    "SELECT id FROM checkpoints WHERE agent_id = ?1 ORDER BY created_at DESC LIMIT 1",
                    rusqlite::params![agent_id],
                    |row| row.get(0),
                ) {
                    Ok(id) => Some(id),
                    Err(rusqlite::Error::QueryReturnedNoRows) => None,
                    Err(e) => return Err(e),
                };

                match id {
                    Some(id) => load_node(conn, id),
                    None => Ok(None),
                }
            })
            .await?;
        Ok(node)
    }

    async fn get_children(&self, id: CheckpointId) -> nephila_core::Result<Vec<CheckpointNode>> {
        let nodes = self
            .writer
            .execute(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, agent_id, parent_id, branch_label, channels, l2_namespace, interrupt, created_at
                     FROM checkpoints WHERE parent_id = ?1 ORDER BY created_at",
                )?;
                let rows = stmt
                    .query_map(rusqlite::params![id], row_to_node)?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await?;
        Ok(nodes)
    }

    async fn get_ancestry(&self, id: CheckpointId) -> nephila_core::Result<Vec<CheckpointNode>> {
        let nodes = self
            .writer
            .execute(move |conn| {
                let mut stmt = conn.prepare(
                    "WITH RECURSIVE ancestry(id, agent_id, parent_id, branch_label, channels, l2_namespace, interrupt, created_at, depth) AS (
                        SELECT id, agent_id, parent_id, branch_label, channels, l2_namespace, interrupt, created_at, 0
                        FROM checkpoints WHERE id = ?1
                        UNION ALL
                        SELECT c.id, c.agent_id, c.parent_id, c.branch_label, c.channels, c.l2_namespace, c.interrupt, c.created_at, a.depth + 1
                        FROM checkpoints c JOIN ancestry a ON c.id = a.parent_id
                    )
                    SELECT id, agent_id, parent_id, branch_label, channels, l2_namespace, interrupt, created_at
                    FROM ancestry ORDER BY depth DESC",
                )?;
                let rows = stmt
                    .query_map(rusqlite::params![id], row_to_node)?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await?;
        Ok(nodes)
    }

    async fn list_branches(&self, agent_id: AgentId) -> nephila_core::Result<Vec<CheckpointNode>> {
        let nodes = self
            .writer
            .execute(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT c.id, c.agent_id, c.parent_id, c.branch_label, c.channels, c.l2_namespace, c.interrupt, c.created_at
                     FROM checkpoints c
                     LEFT JOIN checkpoints child ON child.parent_id = c.id
                     WHERE c.agent_id = ?1 AND child.id IS NULL",
                )?;
                let rows = stmt
                    .query_map(rusqlite::params![agent_id], row_to_node)?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await?;
        Ok(nodes)
    }

    async fn search_l2(
        &self,
        agent_id: AgentId,
        namespace: Option<&str>,
        embedding: &[f32],
        limit: usize,
    ) -> nephila_core::Result<Vec<L2SearchResult>> {
        let emb_bytes = f32_slice_to_bytes(embedding);
        let ns = namespace.map(|s| s.to_string());
        let results = self
            .writer
            .execute(move |conn| {
                let (query, params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match &ns {
                    Some(ns_val) => (
                        "SELECT c.id, c.content, c.tags, c.agent_id, c.namespace, v.distance
                         FROM vec_l2_chunks v
                         JOIN l2_chunks c ON c.id = v.chunk_id
                         WHERE c.agent_id = ?1 AND c.namespace = ?2 AND v.embedding MATCH ?3
                         ORDER BY v.distance
                         LIMIT ?4"
                            .to_string(),
                        vec![
                            Box::new(agent_id) as Box<dyn rusqlite::types::ToSql>,
                            Box::new(ns_val.clone()),
                            Box::new(emb_bytes.clone()),
                            Box::new(limit as i64),
                        ],
                    ),
                    None => (
                        "SELECT c.id, c.content, c.tags, c.agent_id, c.namespace, v.distance
                         FROM vec_l2_chunks v
                         JOIN l2_chunks c ON c.id = v.chunk_id
                         WHERE c.agent_id = ?1 AND v.embedding MATCH ?2
                         ORDER BY v.distance
                         LIMIT ?3"
                            .to_string(),
                        vec![
                            Box::new(agent_id) as Box<dyn rusqlite::types::ToSql>,
                            Box::new(emb_bytes.clone()),
                            Box::new(limit as i64),
                        ],
                    ),
                };

                let mut stmt = conn.prepare(&query)?;
                let param_refs: Vec<&dyn rusqlite::types::ToSql> =
                    params.iter().map(|p| p.as_ref()).collect();
                let rows = stmt
                    .query_map(param_refs.as_slice(), |row| {
                        let id: EntryId = row.get(0)?;
                        let content: String = row.get(1)?;
                        let tags_json: String = row.get(2)?;
                        let agent_id: AgentId = row.get(3)?;
                        let namespace: String = row.get(4)?;
                        let distance: f32 = row.get(5)?;
                        let tags: Vec<String> =
                            serde_json::from_str(&tags_json).unwrap_or_default();
                        Ok(L2SearchResult {
                            chunk: L2Chunk { id, content, tags },
                            agent_id,
                            namespace,
                            score: 1.0 - distance,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await?;
        Ok(results)
    }

    async fn search_l2_global(
        &self,
        namespace: Option<&str>,
        embedding: &[f32],
        limit: usize,
    ) -> nephila_core::Result<Vec<L2SearchResult>> {
        let emb_bytes = f32_slice_to_bytes(embedding);
        let ns = namespace.map(|s| s.to_string());
        let results = self
            .writer
            .execute(move |conn| {
                let (query, params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match &ns {
                    Some(ns_val) => (
                        "SELECT c.id, c.content, c.tags, c.agent_id, c.namespace, v.distance
                         FROM vec_l2_chunks v
                         JOIN l2_chunks c ON c.id = v.chunk_id
                         WHERE c.namespace = ?1 AND v.embedding MATCH ?2
                         ORDER BY v.distance
                         LIMIT ?3"
                            .to_string(),
                        vec![
                            Box::new(ns_val.clone()) as Box<dyn rusqlite::types::ToSql>,
                            Box::new(emb_bytes.clone()),
                            Box::new(limit as i64),
                        ],
                    ),
                    None => (
                        "SELECT c.id, c.content, c.tags, c.agent_id, c.namespace, v.distance
                         FROM vec_l2_chunks v
                         JOIN l2_chunks c ON c.id = v.chunk_id
                         WHERE v.embedding MATCH ?1
                         ORDER BY v.distance
                         LIMIT ?2"
                            .to_string(),
                        vec![
                            Box::new(emb_bytes.clone()) as Box<dyn rusqlite::types::ToSql>,
                            Box::new(limit as i64),
                        ],
                    ),
                };

                let mut stmt = conn.prepare(&query)?;
                let param_refs: Vec<&dyn rusqlite::types::ToSql> =
                    params.iter().map(|p| p.as_ref()).collect();
                let rows = stmt
                    .query_map(param_refs.as_slice(), |row| {
                        let id: EntryId = row.get(0)?;
                        let content: String = row.get(1)?;
                        let tags_json: String = row.get(2)?;
                        let agent_id: AgentId = row.get(3)?;
                        let namespace: String = row.get(4)?;
                        let distance: f32 = row.get(5)?;
                        let tags: Vec<String> =
                            serde_json::from_str(&tags_json).unwrap_or_default();
                        Ok(L2SearchResult {
                            chunk: L2Chunk { id, content, tags },
                            agent_id,
                            namespace,
                            score: 1.0 - distance,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await?;
        Ok(results)
    }
}

fn row_to_node(row: &rusqlite::Row) -> Result<CheckpointNode, rusqlite::Error> {
    let channels_json: String = row.get(4)?;
    let interrupt_json: Option<String> = row.get(6)?;
    let created_str: String = row.get(7)?;

    let channels: BTreeMap<String, ChannelEntry> =
        serde_json::from_str(&channels_json).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(e))
        })?;

    let interrupt: Option<InterruptSnapshot> = interrupt_json
        .map(|s| serde_json::from_str(&s))
        .transpose()
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(6, rusqlite::types::Type::Text, Box::new(e))
        })?;

    Ok(CheckpointNode {
        id: row.get(0)?,
        agent_id: row.get(1)?,
        parent_id: row.get(2)?,
        branch_label: row.get(3)?,
        channels,
        l2_namespace: row.get(5)?,
        interrupt,
        created_at: parse_rfc3339(&created_str)?,
    })
}

fn load_node(
    conn: &rusqlite::Connection,
    id: CheckpointId,
) -> Result<Option<CheckpointNode>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT id, agent_id, parent_id, branch_label, channels, l2_namespace, interrupt, created_at
         FROM checkpoints WHERE id = ?1",
    )?;
    match stmt.query_row(rusqlite::params![id], row_to_node) {
        Ok(node) => Ok(Some(node)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use crate::SqliteStore;
    use chrono::Utc;
    use nephila_core::checkpoint::{ChannelEntry, CheckpointNode, L2Chunk, ReducerKind};
    use nephila_core::id::{AgentId, CheckpointId, EntryId, ObjectiveId};
    use nephila_core::store::CheckpointStore;
    use std::collections::BTreeMap;

    fn make_channels() -> BTreeMap<String, ChannelEntry> {
        BTreeMap::from([
            (
                "objectives".into(),
                ChannelEntry {
                    reducer: ReducerKind::Overwrite,
                    value: serde_json::json!([]),
                },
            ),
            (
                "progress_summary".into(),
                ChannelEntry {
                    reducer: ReducerKind::Overwrite,
                    value: serde_json::json!("initial"),
                },
            ),
            (
                "decisions".into(),
                ChannelEntry {
                    reducer: ReducerKind::Append,
                    value: serde_json::json!([]),
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

    fn make_node(agent_id: AgentId, parent_id: Option<CheckpointId>) -> CheckpointNode {
        CheckpointNode {
            id: CheckpointId::new(),
            agent_id,
            parent_id,
            branch_label: None,
            channels: make_channels(),
            l2_namespace: "general".into(),
            interrupt: None,
            created_at: Utc::now(),
        }
    }

    async fn make_agent(store: &SqliteStore) -> AgentId {
        let agent_id = AgentId::new();
        let obj_id = ObjectiveId::new();
        store
            .writer
            .execute(move |conn| {
                let now = Utc::now().to_rfc3339();
                conn.execute(
                    "INSERT INTO agents (id, state, directive, directory, objective_id, spawn_origin_type, created_at, updated_at)
                     VALUES (?1, 'starting', 'continue', '/tmp', ?2, 'operator', ?3, ?4)",
                    rusqlite::params![agent_id, obj_id, &now, &now],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        agent_id
    }

    #[tokio::test]
    async fn save_and_get() {
        let store = SqliteStore::open_in_memory(384).unwrap();
        let agent_id = make_agent(&store).await;
        let node = make_node(agent_id, None);
        let node_id = node.id;

        store.save(&node, &[], &[]).await.unwrap();
        let fetched = store.get(node_id).await.unwrap().unwrap();
        assert_eq!(fetched.id, node_id);
        assert_eq!(fetched.agent_id, agent_id);
        assert!(fetched.parent_id.is_none());
        assert_eq!(fetched.channels.len(), 4);
    }

    #[tokio::test]
    async fn get_latest() {
        let store = SqliteStore::open_in_memory(384).unwrap();
        let agent_id = make_agent(&store).await;

        let n1 = make_node(agent_id, None);
        store.save(&n1, &[], &[]).await.unwrap();

        // small delay to ensure ordering
        let mut n2 = make_node(agent_id, Some(n1.id));
        n2.created_at = Utc::now();
        store.save(&n2, &[], &[]).await.unwrap();

        let latest = store.get_latest(agent_id).await.unwrap().unwrap();
        assert_eq!(latest.id, n2.id);
    }

    #[tokio::test]
    async fn get_ancestry_chain() {
        let store = SqliteStore::open_in_memory(384).unwrap();
        let agent_id = make_agent(&store).await;

        let n1 = make_node(agent_id, None);
        store.save(&n1, &[], &[]).await.unwrap();

        let n2 = make_node(agent_id, Some(n1.id));
        store.save(&n2, &[], &[]).await.unwrap();

        let n3 = make_node(agent_id, Some(n2.id));
        store.save(&n3, &[], &[]).await.unwrap();

        let ancestry = store.get_ancestry(n3.id).await.unwrap();
        assert_eq!(ancestry.len(), 3);
        assert_eq!(ancestry[0].id, n1.id);
        assert_eq!(ancestry[1].id, n2.id);
        assert_eq!(ancestry[2].id, n3.id);
    }

    #[tokio::test]
    async fn list_branches_two_branches() {
        let store = SqliteStore::open_in_memory(384).unwrap();
        let agent_id = make_agent(&store).await;

        let root = make_node(agent_id, None);
        store.save(&root, &[], &[]).await.unwrap();

        let mut branch_a = make_node(agent_id, Some(root.id));
        branch_a.branch_label = Some("branch-a".into());
        store.save(&branch_a, &[], &[]).await.unwrap();

        let mut branch_b = make_node(agent_id, Some(root.id));
        branch_b.branch_label = Some("branch-b".into());
        store.save(&branch_b, &[], &[]).await.unwrap();

        let leaves = store.list_branches(agent_id).await.unwrap();
        assert_eq!(leaves.len(), 2);
        let ids: Vec<_> = leaves.iter().map(|n| n.id).collect();
        assert!(ids.contains(&branch_a.id));
        assert!(ids.contains(&branch_b.id));
    }

    #[tokio::test]
    async fn get_children() {
        let store = SqliteStore::open_in_memory(384).unwrap();
        let agent_id = make_agent(&store).await;

        let root = make_node(agent_id, None);
        store.save(&root, &[], &[]).await.unwrap();

        let child1 = make_node(agent_id, Some(root.id));
        store.save(&child1, &[], &[]).await.unwrap();

        let child2 = make_node(agent_id, Some(root.id));
        store.save(&child2, &[], &[]).await.unwrap();

        let children = store.get_children(root.id).await.unwrap();
        assert_eq!(children.len(), 2);
    }

    #[tokio::test]
    async fn get_latest_no_checkpoints() {
        let store = SqliteStore::open_in_memory(384).unwrap();
        let agent_id = make_agent(&store).await;
        let result = store.get_latest(agent_id).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn save_with_l2_chunks() {
        let store = SqliteStore::open_in_memory(384).unwrap();
        let agent_id = make_agent(&store).await;

        let node = make_node(agent_id, None);
        let chunks = vec![L2Chunk {
            id: EntryId::new(),
            content: "detail chunk".into(),
            tags: vec!["tag1".into()],
        }];
        let embeddings = vec![vec![0.1f32; 384]];

        store.save(&node, &chunks, &embeddings).await.unwrap();

        let fetched = store.get(node.id).await.unwrap().unwrap();
        assert_eq!(fetched.id, node.id);
    }
}

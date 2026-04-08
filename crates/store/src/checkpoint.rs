use crate::SqliteStore;
use crate::util::parse_rfc3339;
use nephila_core::checkpoint::{ChannelEntry, CheckpointNode, InterruptSnapshot};
use nephila_core::id::{AgentId, CheckpointId};
use std::collections::BTreeMap;

impl SqliteStore {
    pub async fn save_checkpoint_metadata(
        &self,
        node: &CheckpointNode,
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

        self.writer
            .execute(move |conn| {
                conn.execute(
                    "INSERT INTO checkpoints (id, agent_id, parent_id, branch_label, channels, l2_namespace, interrupt, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    rusqlite::params![
                        id, agent_id, parent_id, branch_label,
                        channels_json, l2_namespace, interrupt_json, created_at,
                    ],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    pub async fn get_checkpoint(
        &self,
        id: CheckpointId,
    ) -> nephila_core::Result<Option<CheckpointNode>> {
        let node = self.writer.execute(move |conn| load_node(conn, id)).await?;
        Ok(node)
    }

    pub async fn get_latest_checkpoint(
        &self,
        agent_id: AgentId,
    ) -> nephila_core::Result<Option<CheckpointNode>> {
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

    pub async fn get_checkpoint_children(
        &self,
        id: CheckpointId,
    ) -> nephila_core::Result<Vec<CheckpointNode>> {
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

    pub async fn get_checkpoint_ancestry(
        &self,
        id: CheckpointId,
    ) -> nephila_core::Result<Vec<CheckpointNode>> {
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

    pub async fn list_checkpoint_branches(
        &self,
        agent_id: AgentId,
    ) -> nephila_core::Result<Vec<CheckpointNode>> {
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
    use nephila_core::checkpoint::{ChannelEntry, CheckpointNode, ReducerKind};
    use nephila_core::id::{AgentId, CheckpointId, ObjectiveId};
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

        store.save_checkpoint_metadata(&node).await.unwrap();
        let fetched = store.get_checkpoint(node_id).await.unwrap().unwrap();
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
        store.save_checkpoint_metadata(&n1).await.unwrap();

        let mut n2 = make_node(agent_id, Some(n1.id));
        n2.created_at = Utc::now();
        store.save_checkpoint_metadata(&n2).await.unwrap();

        let latest = store
            .get_latest_checkpoint(agent_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(latest.id, n2.id);
    }

    #[tokio::test]
    async fn get_ancestry_chain() {
        let store = SqliteStore::open_in_memory(384).unwrap();
        let agent_id = make_agent(&store).await;

        let n1 = make_node(agent_id, None);
        store.save_checkpoint_metadata(&n1).await.unwrap();

        let n2 = make_node(agent_id, Some(n1.id));
        store.save_checkpoint_metadata(&n2).await.unwrap();

        let n3 = make_node(agent_id, Some(n2.id));
        store.save_checkpoint_metadata(&n3).await.unwrap();

        let ancestry = store.get_checkpoint_ancestry(n3.id).await.unwrap();
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
        store.save_checkpoint_metadata(&root).await.unwrap();

        let mut branch_a = make_node(agent_id, Some(root.id));
        branch_a.branch_label = Some("branch-a".into());
        store.save_checkpoint_metadata(&branch_a).await.unwrap();

        let mut branch_b = make_node(agent_id, Some(root.id));
        branch_b.branch_label = Some("branch-b".into());
        store.save_checkpoint_metadata(&branch_b).await.unwrap();

        let leaves = store.list_checkpoint_branches(agent_id).await.unwrap();
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
        store.save_checkpoint_metadata(&root).await.unwrap();

        let child1 = make_node(agent_id, Some(root.id));
        store.save_checkpoint_metadata(&child1).await.unwrap();

        let child2 = make_node(agent_id, Some(root.id));
        store.save_checkpoint_metadata(&child2).await.unwrap();

        let children = store.get_checkpoint_children(root.id).await.unwrap();
        assert_eq!(children.len(), 2);
    }

    #[tokio::test]
    async fn get_latest_no_checkpoints() {
        let store = SqliteStore::open_in_memory(384).unwrap();
        let agent_id = make_agent(&store).await;
        let result = store.get_latest_checkpoint(agent_id).await.unwrap();
        assert!(result.is_none());
    }
}

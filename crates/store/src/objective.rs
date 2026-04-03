use crate::SqliteStore;
use crate::util::parse_rfc3339;
use chrono::Utc;
use nephila_core::id::{AgentId, ObjectiveId};
use nephila_core::objective::{NewObjective, ObjectiveNode, ObjectiveStatus, ObjectiveTree};
use nephila_core::store::ObjectiveStore;
use std::collections::HashMap;

impl ObjectiveStore for SqliteStore {
    async fn create(&self, objective: NewObjective) -> nephila_core::Result<ObjectiveId> {
        let id = ObjectiveId::new();
        let now = Utc::now().to_rfc3339();
        self.writer
            .execute(move |conn| {
                conn.execute(
                    "INSERT INTO objectives (id, parent_id, agent_id, description, status, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    rusqlite::params![
                        id,
                        objective.parent_id,
                        objective.agent_id,
                        objective.description,
                        ObjectiveStatus::Pending.to_string(),
                        &now,
                        &now,
                    ],
                )?;
                Ok(())
            })
            .await?;
        Ok(id)
    }

    async fn update_status(
        &self,
        id: ObjectiveId,
        status: ObjectiveStatus,
    ) -> nephila_core::Result<()> {
        let now = Utc::now().to_rfc3339();
        self.writer
            .execute(move |conn| {
                let rows = conn.execute(
                    "UPDATE objectives SET status = ?1, updated_at = ?2 WHERE id = ?3",
                    rusqlite::params![status.to_string(), now, id],
                )?;
                if rows == 0 {
                    return Err(rusqlite::Error::QueryReturnedNoRows);
                }
                Ok(())
            })
            .await
            .map_err(|_| nephila_core::NephilaError::ObjectiveNotFound(id))?;
        Ok(())
    }

    async fn get_node(&self, id: ObjectiveId) -> nephila_core::Result<Option<ObjectiveNode>> {
        let node = self
            .writer
            .execute(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, parent_id, agent_id, description, status, created_at, updated_at
                     FROM objectives WHERE id = ?1",
                )?;
                let result = stmt.query_row(rusqlite::params![id], row_to_node);
                match result {
                    Ok(node) => Ok(Some(node)),
                    Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                    Err(e) => Err(e),
                }
            })
            .await?;
        Ok(node)
    }

    async fn get_tree(&self, root_id: ObjectiveId) -> nephila_core::Result<ObjectiveTree> {
        let tree = self
            .writer
            .execute(move |conn| {
                let mut stmt = conn.prepare(
                    "WITH RECURSIVE subtree AS (
                         SELECT id, parent_id, agent_id, description, status, created_at, updated_at
                         FROM objectives WHERE id = ?1
                         UNION ALL
                         SELECT o.id, o.parent_id, o.agent_id, o.description, o.status, o.created_at, o.updated_at
                         FROM objectives o
                         JOIN subtree s ON o.parent_id = s.id
                     )
                     SELECT id, parent_id, agent_id, description, status, created_at, updated_at
                     FROM subtree",
                )?;
                let all_nodes: Vec<ObjectiveNode> = stmt
                    .query_map(rusqlite::params![root_id], row_to_node)?
                    .collect::<Result<Vec<_>, _>>()?;

                let root = build_tree(root_id, all_nodes);
                Ok(root)
            })
            .await?;

        match tree {
            Some(root) => Ok(ObjectiveTree { root }),
            None => Err(crate::StoreError::NotFound(
                "objective root not found".to_string(),
            ))?,
        }
    }

    async fn assign_agent(
        &self,
        objective_id: ObjectiveId,
        agent_id: AgentId,
    ) -> nephila_core::Result<()> {
        let now = Utc::now().to_rfc3339();
        self.writer
            .execute(move |conn| {
                let rows = conn.execute(
                    "UPDATE objectives SET agent_id = ?1, updated_at = ?2 WHERE id = ?3",
                    rusqlite::params![agent_id, now, objective_id],
                )?;
                if rows == 0 {
                    return Err(rusqlite::Error::QueryReturnedNoRows);
                }
                Ok(())
            })
            .await
            .map_err(|_| nephila_core::NephilaError::ObjectiveNotFound(objective_id))?;
        Ok(())
    }
}

fn row_to_node(row: &rusqlite::Row) -> Result<ObjectiveNode, rusqlite::Error> {
    let description: String = row.get(3)?;
    let status_str: String = row.get(4)?;
    let created_str: String = row.get(5)?;
    let updated_str: String = row.get(6)?;

    Ok(ObjectiveNode {
        id: row.get(0)?,
        parent_id: row.get(1)?,
        agent_id: row.get(2)?,
        description,
        status: status_str
            .parse::<ObjectiveStatus>()
            .unwrap_or(ObjectiveStatus::Pending),
        children: Vec::new(),
        created_at: parse_rfc3339(&created_str)?,
        updated_at: parse_rfc3339(&updated_str)?,
    })
}

fn build_tree(root_id: ObjectiveId, nodes: Vec<ObjectiveNode>) -> Option<ObjectiveNode> {
    let mut by_id: HashMap<ObjectiveId, ObjectiveNode> = HashMap::new();
    let mut children_map: HashMap<ObjectiveId, Vec<ObjectiveId>> = HashMap::new();

    for node in &nodes {
        if let Some(pid) = node.parent_id {
            children_map.entry(pid).or_default().push(node.id);
        }
    }
    for node in nodes {
        by_id.insert(node.id, node);
    }

    build_subtree(root_id, &mut by_id, &children_map)
}

fn build_subtree(
    id: ObjectiveId,
    by_id: &mut HashMap<ObjectiveId, ObjectiveNode>,
    children_map: &HashMap<ObjectiveId, Vec<ObjectiveId>>,
) -> Option<ObjectiveNode> {
    let mut node = by_id.remove(&id)?;
    if let Some(child_ids) = children_map.get(&id) {
        for &child_id in child_ids {
            if let Some(child) = build_subtree(child_id, by_id, children_map) {
                node.children.push(child);
            }
        }
    }
    Some(node)
}

#[cfg(test)]
mod tests {
    use crate::SqliteStore;
    use nephila_core::objective::{NewObjective, ObjectiveStatus};
    use nephila_core::store::ObjectiveStore;

    #[tokio::test]
    async fn create_and_get() {
        let store = SqliteStore::open_in_memory(384).unwrap();
        let id = store
            .create(NewObjective {
                parent_id: None,
                agent_id: None,
                description: "root objective".to_string(),
            })
            .await
            .unwrap();

        let node = store.get_node(id).await.unwrap().unwrap();
        assert_eq!(node.description, "root objective");
        assert_eq!(node.status, ObjectiveStatus::Pending);
    }

    #[tokio::test]
    async fn status_update() {
        let store = SqliteStore::open_in_memory(384).unwrap();
        let id = store
            .create(NewObjective {
                parent_id: None,
                agent_id: None,
                description: "test".to_string(),
            })
            .await
            .unwrap();

        store
            .update_status(id, ObjectiveStatus::InProgress)
            .await
            .unwrap();
        let node = store.get_node(id).await.unwrap().unwrap();
        assert_eq!(node.status, ObjectiveStatus::InProgress);
    }

    #[tokio::test]
    async fn tree_building() {
        let store = SqliteStore::open_in_memory(384).unwrap();

        let root_id = store
            .create(NewObjective {
                parent_id: None,
                agent_id: None,
                description: "root".to_string(),
            })
            .await
            .unwrap();

        let _child1 = store
            .create(NewObjective {
                parent_id: Some(root_id),
                agent_id: None,
                description: "child 1".to_string(),
            })
            .await
            .unwrap();

        let _child2 = store
            .create(NewObjective {
                parent_id: Some(root_id),
                agent_id: None,
                description: "child 2".to_string(),
            })
            .await
            .unwrap();

        let tree = store.get_tree(root_id).await.unwrap();
        assert_eq!(tree.root.description, "root");
        assert_eq!(tree.root.children.len(), 2);

        let descs: Vec<&str> = tree
            .root
            .children
            .iter()
            .map(|c| c.description.as_str())
            .collect();
        assert!(descs.contains(&"child 1"));
        assert!(descs.contains(&"child 2"));
    }

    #[tokio::test]
    async fn update_status_nonexistent_errors() {
        let store = SqliteStore::open_in_memory(384).unwrap();
        let fake_id = nephila_core::ObjectiveId::new();
        let result = store.update_status(fake_id, ObjectiveStatus::Done).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn get_node_nonexistent_returns_none() {
        let store = SqliteStore::open_in_memory(384).unwrap();
        let fake_id = nephila_core::ObjectiveId::new();
        let result = store.get_node(fake_id).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn get_tree_nonexistent_root_errors() {
        let store = SqliteStore::open_in_memory(384).unwrap();
        let fake_id = nephila_core::ObjectiveId::new();
        let result = store.get_tree(fake_id).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn get_tree_isolates_subtree() {
        let store = SqliteStore::open_in_memory(384).unwrap();

        let root1 = store
            .create(NewObjective {
                parent_id: None,
                agent_id: None,
                description: "root 1".to_string(),
            })
            .await
            .unwrap();

        let _child1 = store
            .create(NewObjective {
                parent_id: Some(root1),
                agent_id: None,
                description: "child of root 1".to_string(),
            })
            .await
            .unwrap();

        let root2 = store
            .create(NewObjective {
                parent_id: None,
                agent_id: None,
                description: "root 2".to_string(),
            })
            .await
            .unwrap();

        let _child2 = store
            .create(NewObjective {
                parent_id: Some(root2),
                agent_id: None,
                description: "child of root 2".to_string(),
            })
            .await
            .unwrap();

        let tree = store.get_tree(root1).await.unwrap();
        assert_eq!(tree.root.description, "root 1");
        assert_eq!(tree.root.children.len(), 1);
        assert_eq!(tree.root.children[0].description, "child of root 1");
    }
}

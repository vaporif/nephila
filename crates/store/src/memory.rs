use crate::SqliteStore;
use crate::util::{bytes_to_f32_vec, f32_slice_to_bytes, parse_rfc3339};
use nephila_core::id::EntryId;
use nephila_core::memory::{Embedding, LifecycleState, Link, MemoryEntry, SearchResult};
use nephila_core::store::MemoryStore;

const FIND_SIMILAR_MAX_K: usize = 100;

impl MemoryStore for SqliteStore {
    async fn store(&self, entry: MemoryEntry) -> nephila_core::Result<EntryId> {
        let id = entry.id;
        let emb_bytes = f32_slice_to_bytes(&entry.embedding);
        let tags_json =
            serde_json::to_string(&entry.tags).map_err(nephila_core::NephilaError::from)?;
        self.writer
            .execute(move |conn| {
                let now = entry.created_at.to_rfc3339();
                let tx = conn.unchecked_transaction()?;

                tx.execute(
                    "INSERT INTO memories (id, agent_id, content, embedding, tags, lifecycle_state, importance, access_count, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    rusqlite::params![
                        id,
                        entry.agent_id,
                        entry.content,
                        &emb_bytes,
                        tags_json,
                        entry.lifecycle_state.to_string(),
                        entry.importance as f64,
                        entry.access_count as i64,
                        now,
                    ],
                )?;

                let rowid = tx.last_insert_rowid();

                tx.execute(
                    "INSERT INTO vec_memories (rowid, embedding) VALUES (?1, ?2)",
                    rusqlite::params![rowid, &emb_bytes],
                )?;

                tx.commit()?;
                Ok(())
            })
            .await?;
        Ok(id)
    }

    async fn get(&self, id: EntryId) -> nephila_core::Result<Option<MemoryEntry>> {
        let entry = self
            .writer
            .execute(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, agent_id, content, embedding, tags, lifecycle_state, importance, access_count, created_at
                     FROM memories WHERE id = ?1",
                )?;
                match stmt.query_row(rusqlite::params![id], row_to_memory) {
                    Ok(entry) => Ok(Some(entry)),
                    Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                    Err(e) => Err(e),
                }
            })
            .await?;
        Ok(entry)
    }

    async fn search(
        &self,
        query: &Embedding,
        limit: usize,
    ) -> nephila_core::Result<Vec<SearchResult>> {
        let query_bytes = f32_slice_to_bytes(query);
        let results = self
            .writer
            .execute(move |conn| {
                let mut vec_stmt = conn.prepare(
                    "SELECT rowid, distance
                     FROM vec_memories
                     WHERE embedding MATCH ?1 AND k = ?2",
                )?;
                let vec_rows: Vec<(i64, f64)> = vec_stmt
                    .query_map(rusqlite::params![query_bytes, limit as i64], |row| {
                        Ok((row.get(0)?, row.get(1)?))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;

                let mut results = Vec::new();
                let mut mem_stmt = conn.prepare(
                    "SELECT id, agent_id, content, embedding, tags, lifecycle_state, importance, access_count, created_at
                     FROM memories WHERE rowid = ?1",
                )?;
                for (rowid, distance) in vec_rows {
                    match mem_stmt.query_row(rusqlite::params![rowid], row_to_memory) {
                        Ok(entry) => {
                            results.push(SearchResult {
                                entry,
                                score: (1.0 - distance) as f32,
                                linked: Vec::new(),
                            });
                        }
                        Err(rusqlite::Error::QueryReturnedNoRows) => continue,
                        Err(e) => return Err(e),
                    }
                }
                Ok(results)
            })
            .await?;
        Ok(results)
    }

    async fn find_similar(
        &self,
        embedding: &Embedding,
        threshold: f32,
    ) -> nephila_core::Result<Vec<(EntryId, f32)>> {
        let query_bytes = f32_slice_to_bytes(embedding);
        let results = self
            .writer
            .execute(move |conn| {
                let mut vec_stmt = conn.prepare(
                    "SELECT rowid, distance
                     FROM vec_memories
                     WHERE embedding MATCH ?1 AND k = ?2",
                )?;
                let rows: Vec<(i64, f64)> = vec_stmt
                    .query_map(
                        rusqlite::params![query_bytes, FIND_SIMILAR_MAX_K as i64],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )?
                    .collect::<Result<Vec<_>, _>>()?;

                let mut results = Vec::new();
                let mut mem_stmt = conn.prepare("SELECT id FROM memories WHERE rowid = ?1")?;
                for (rowid, distance) in rows {
                    let score = (1.0 - distance) as f32;
                    if score >= threshold {
                        let entry_id: EntryId =
                            mem_stmt.query_row(rusqlite::params![rowid], |row| row.get(0))?;
                        results.push((entry_id, score));
                    }
                }
                Ok(results)
            })
            .await?;
        Ok(results)
    }

    async fn update_links(&self, id: EntryId, links: Vec<Link>) -> nephila_core::Result<()> {
        self.writer
            .execute(move |conn| {
                let tx = conn.unchecked_transaction()?;
                for link in &links {
                    tx.execute(
                        "INSERT OR REPLACE INTO memory_links (source_id, target_id, similarity_score)
                         VALUES (?1, ?2, ?3)",
                        rusqlite::params![
                            id,
                            link.target_id,
                            link.similarity_score as f64,
                        ],
                    )?;
                    tx.execute(
                        "INSERT OR REPLACE INTO memory_links (source_id, target_id, similarity_score)
                         VALUES (?1, ?2, ?3)",
                        rusqlite::params![
                            link.target_id,
                            id,
                            link.similarity_score as f64,
                        ],
                    )?;
                }
                tx.commit()?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    async fn get_linked(
        &self,
        id: EntryId,
        _depth: usize,
    ) -> nephila_core::Result<Vec<MemoryEntry>> {
        let entries = self
            .writer
            .execute(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT m.id, m.agent_id, m.content, m.embedding, m.tags, m.lifecycle_state, m.importance, m.access_count, m.created_at
                     FROM memory_links ml
                     JOIN memories m ON m.id = ml.target_id
                     WHERE ml.source_id = ?1",
                )?;
                let rows = stmt
                    .query_map(rusqlite::params![id], row_to_memory)?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await?;
        Ok(entries)
    }

    async fn transition_state(
        &self,
        id: EntryId,
        new_state: LifecycleState,
    ) -> nephila_core::Result<()> {
        self.writer
            .execute(move |conn| {
                let rows = conn.execute(
                    "UPDATE memories SET lifecycle_state = ?1 WHERE id = ?2",
                    rusqlite::params![new_state.to_string(), id],
                )?;
                if rows == 0 {
                    return Err(rusqlite::Error::QueryReturnedNoRows);
                }
                Ok(())
            })
            .await
            .map_err(|_| nephila_core::NephilaError::EntryNotFound(id))?;
        Ok(())
    }

    async fn increment_access(&self, id: EntryId) -> nephila_core::Result<()> {
        self.writer
            .execute(move |conn| {
                let rows = conn.execute(
                    "UPDATE memories SET access_count = access_count + 1 WHERE id = ?1",
                    rusqlite::params![id],
                )?;
                if rows == 0 {
                    return Err(rusqlite::Error::QueryReturnedNoRows);
                }
                Ok(())
            })
            .await
            .map_err(|_| nephila_core::NephilaError::EntryNotFound(id))?;
        Ok(())
    }
}

fn row_to_memory(row: &rusqlite::Row) -> Result<MemoryEntry, rusqlite::Error> {
    let content: String = row.get(2)?;
    let emb_bytes: Vec<u8> = row.get(3)?;
    let tags_json: String = row.get(4)?;
    let lifecycle_str: String = row.get(5)?;
    let importance: f64 = row.get(6)?;
    let access_count: i64 = row.get(7)?;
    let created_str: String = row.get(8)?;

    Ok(MemoryEntry {
        id: row.get(0)?,
        agent_id: row.get(1)?,
        content,
        embedding: bytes_to_f32_vec(&emb_bytes),
        tags: serde_json::from_str(&tags_json).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?,
        lifecycle_state: lifecycle_str
            .parse::<LifecycleState>()
            .unwrap_or(LifecycleState::Generated),
        importance: importance as f32,
        access_count: access_count as u32,
        created_at: parse_rfc3339(&created_str)?,
    })
}

#[cfg(test)]
mod tests {
    use crate::test_util::{make_agent_and_store, make_embedding};
    use chrono::Utc;
    use nephila_core::id::EntryId;
    use nephila_core::memory::{LifecycleState, Link, MemoryEntry};
    use nephila_core::store::MemoryStore;

    #[tokio::test]
    async fn store_and_get() {
        let (store, agent_id) = make_agent_and_store().await;
        let entry = MemoryEntry {
            id: EntryId::new(),
            agent_id,
            content: "test memory".to_string(),
            embedding: make_embedding(0.1),
            tags: vec!["tag1".to_string()],
            lifecycle_state: LifecycleState::Generated,
            importance: 0.8,
            access_count: 0,
            created_at: Utc::now(),
        };
        let id = entry.id;

        store.store(entry).await.unwrap();
        let fetched = store.get(id).await.unwrap().unwrap();
        assert_eq!(fetched.content, "test memory");
        assert_eq!(fetched.tags, vec!["tag1".to_string()]);
    }

    #[tokio::test]
    async fn search_memories() {
        let (store, agent_id) = make_agent_and_store().await;

        let e1 = MemoryEntry {
            id: EntryId::new(),
            agent_id,
            content: "close match".to_string(),
            embedding: make_embedding(0.5),
            tags: vec![],
            lifecycle_state: LifecycleState::Active,
            importance: 0.9,
            access_count: 0,
            created_at: Utc::now(),
        };
        let e2 = MemoryEntry {
            id: EntryId::new(),
            agent_id,
            content: "different".to_string(),
            embedding: make_embedding(0.9),
            tags: vec![],
            lifecycle_state: LifecycleState::Active,
            importance: 0.5,
            access_count: 0,
            created_at: Utc::now(),
        };

        store.store(e1).await.unwrap();
        store.store(e2).await.unwrap();

        let query = make_embedding(0.5);
        let results = store.search(&query, 2).await.unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].entry.content, "close match");
    }

    #[tokio::test]
    async fn lifecycle_transition() {
        let (store, agent_id) = make_agent_and_store().await;
        let entry = MemoryEntry {
            id: EntryId::new(),
            agent_id,
            content: "lifecycle test".to_string(),
            embedding: make_embedding(0.3),
            tags: vec![],
            lifecycle_state: LifecycleState::Generated,
            importance: 0.5,
            access_count: 0,
            created_at: Utc::now(),
        };
        let id = entry.id;

        store.store(entry).await.unwrap();
        store
            .transition_state(id, LifecycleState::Active)
            .await
            .unwrap();

        let fetched = store.get(id).await.unwrap().unwrap();
        assert_eq!(fetched.lifecycle_state, LifecycleState::Active);
    }

    #[tokio::test]
    async fn increment_access_count() {
        let (store, agent_id) = make_agent_and_store().await;
        let entry = MemoryEntry {
            id: EntryId::new(),
            agent_id,
            content: "access test".to_string(),
            embedding: make_embedding(0.2),
            tags: vec![],
            lifecycle_state: LifecycleState::Generated,
            importance: 0.5,
            access_count: 0,
            created_at: Utc::now(),
        };
        let id = entry.id;

        store.store(entry).await.unwrap();
        store.increment_access(id).await.unwrap();
        store.increment_access(id).await.unwrap();

        let fetched = store.get(id).await.unwrap().unwrap();
        assert_eq!(fetched.access_count, 2);
    }

    #[tokio::test]
    async fn update_and_get_links() {
        let (store, agent_id) = make_agent_and_store().await;

        let e1 = MemoryEntry {
            id: EntryId::new(),
            agent_id,
            content: "entry 1".to_string(),
            embedding: make_embedding(0.1),
            tags: vec![],
            lifecycle_state: LifecycleState::Active,
            importance: 0.5,
            access_count: 0,
            created_at: Utc::now(),
        };
        let e2 = MemoryEntry {
            id: EntryId::new(),
            agent_id,
            content: "entry 2".to_string(),
            embedding: make_embedding(0.2),
            tags: vec![],
            lifecycle_state: LifecycleState::Active,
            importance: 0.5,
            access_count: 0,
            created_at: Utc::now(),
        };
        let id1 = e1.id;
        let id2 = e2.id;

        store.store(e1).await.unwrap();
        store.store(e2).await.unwrap();

        store
            .update_links(
                id1,
                vec![Link {
                    target_id: id2,
                    similarity_score: 0.95,
                }],
            )
            .await
            .unwrap();

        let linked = store.get_linked(id1, 1).await.unwrap();
        assert_eq!(linked.len(), 1);
        assert_eq!(linked[0].id, id2);
    }

    #[tokio::test]
    async fn transition_state_nonexistent_errors() {
        let (store, _) = make_agent_and_store().await;
        let fake_id = EntryId::new();
        let result = store
            .transition_state(fake_id, LifecycleState::Active)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn increment_access_nonexistent_errors() {
        let (store, _) = make_agent_and_store().await;
        let fake_id = EntryId::new();
        let result = store.increment_access(fake_id).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn get_nonexistent_returns_none() {
        let (store, _) = make_agent_and_store().await;
        let fake_id = EntryId::new();
        let result = store.get(fake_id).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn search_empty_store() {
        let (store, _) = make_agent_and_store().await;
        let query = make_embedding(0.5);
        let results = store.search(&query, 10).await.unwrap();
        assert!(results.is_empty());
    }
}

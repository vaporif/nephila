use crate::SqliteStore;
use meridian_core::id::AgentId;
use meridian_core::store::HitlStore;

impl HitlStore for SqliteStore {
    async fn record_ask(
        &self,
        agent_id: AgentId,
        question_hash: u64,
    ) -> meridian_core::Result<u32> {
        let count = self
            .writer
            .execute(move |conn| {
                conn.execute(
                    "INSERT INTO hitl_tracking (agent_id, question_hash, ask_count)
                     VALUES (?1, ?2, 1)
                     ON CONFLICT(agent_id, question_hash) DO UPDATE SET ask_count = ask_count + 1",
                    rusqlite::params![agent_id, question_hash as i64],
                )?;
                let count: u32 = conn.query_row(
                    "SELECT ask_count FROM hitl_tracking WHERE agent_id = ?1 AND question_hash = ?2",
                    rusqlite::params![agent_id, question_hash as i64],
                    |row| row.get(0),
                )?;
                Ok(count)
            })
            .await?;
        Ok(count)
    }

    async fn get_ask_count(
        &self,
        agent_id: AgentId,
        question_hash: u64,
    ) -> meridian_core::Result<u32> {
        let count = self
            .writer
            .execute(move |conn| {
                let result = conn.query_row(
                    "SELECT ask_count FROM hitl_tracking WHERE agent_id = ?1 AND question_hash = ?2",
                    rusqlite::params![agent_id, question_hash as i64],
                    |row| row.get::<_, u32>(0),
                );
                match result {
                    Ok(c) => Ok(c),
                    Err(rusqlite::Error::QueryReturnedNoRows) => Ok(0),
                    Err(e) => Err(e),
                }
            })
            .await?;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use crate::test_util::make_agent_and_store;
    use meridian_core::store::HitlStore;

    #[tokio::test]
    async fn record_and_get_count() {
        let (store, agent_id) = make_agent_and_store().await;

        let count = store.record_ask(agent_id, 12345).await.unwrap();
        assert_eq!(count, 1);

        let count = store.record_ask(agent_id, 12345).await.unwrap();
        assert_eq!(count, 2);

        let count = store.get_ask_count(agent_id, 12345).await.unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn get_count_nonexistent() {
        let (store, agent_id) = make_agent_and_store().await;
        let count = store.get_ask_count(agent_id, 99999).await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn different_hashes_independent() {
        let (store, agent_id) = make_agent_and_store().await;

        store.record_ask(agent_id, 111).await.unwrap();
        store.record_ask(agent_id, 111).await.unwrap();
        store.record_ask(agent_id, 222).await.unwrap();

        let c1 = store.get_ask_count(agent_id, 111).await.unwrap();
        let c2 = store.get_ask_count(agent_id, 222).await.unwrap();
        assert_eq!(c1, 2);
        assert_eq!(c2, 1);
    }
}

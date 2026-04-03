use crate::SqliteStore;
use crate::util::parse_rfc3339;
use chrono::Utc;
use meridian_core::checkpoint::InterruptType;
use meridian_core::id::{AgentId, InterruptId};
use meridian_core::interrupt::{InterruptRequest, InterruptStatus};
use meridian_core::store::InterruptStore;

impl InterruptStore for SqliteStore {
    async fn save(&self, request: &InterruptRequest) -> meridian_core::Result<()> {
        let req = request.clone();
        self.writer
            .execute(move |conn| {
                // Duplicate hash detection: if same question_hash is pending for same agent, bump ask_count
                if let Some(ref hash) = req.question_hash {
                    let existing: Option<(String, i64)> = match conn.query_row(
                        "SELECT id, ask_count FROM interrupt_requests WHERE agent_id = ?1 AND question_hash = ?2 AND status = 'pending'",
                        rusqlite::params![req.agent_id, hash],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    ) {
                        Ok(r) => Some(r),
                        Err(rusqlite::Error::QueryReturnedNoRows) => None,
                        Err(e) => return Err(e),
                    };

                    if let Some((_id, count)) = existing {
                        if count >= 3 {
                            return Err(rusqlite::Error::InvalidParameterName(
                                "duplicate question exceeded max retries".into(),
                            ));
                        }
                        conn.execute(
                            "UPDATE interrupt_requests SET ask_count = ask_count + 1 WHERE agent_id = ?1 AND question_hash = ?2 AND status = 'pending'",
                            rusqlite::params![req.agent_id, hash],
                        )?;
                        return Ok(());
                    }
                }

                let interrupt_type_str = serde_json::to_string(&req.interrupt_type)
                    .unwrap_or_else(|_| "\"drain\"".into())
                    .trim_matches('"')
                    .to_string();
                let payload_json = req.payload.as_ref().map(|p| p.to_string());

                conn.execute(
                    "INSERT INTO interrupt_requests (id, agent_id, checkpoint_id, interrupt_type, payload, status, response, question_hash, ask_count, created_at, resolved_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    rusqlite::params![
                        req.id,
                        req.agent_id,
                        req.checkpoint_id,
                        interrupt_type_str,
                        payload_json,
                        "pending",
                        Option::<String>::None,
                        req.question_hash,
                        req.ask_count,
                        req.created_at.to_rfc3339(),
                        Option::<String>::None,
                    ],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    async fn get_pending(
        &self,
        agent_id: AgentId,
    ) -> meridian_core::Result<Option<InterruptRequest>> {
        let result = self
            .writer
            .execute(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, agent_id, checkpoint_id, interrupt_type, payload, status, response, question_hash, ask_count, created_at, resolved_at
                     FROM interrupt_requests WHERE agent_id = ?1 AND status = 'pending'
                     ORDER BY created_at DESC LIMIT 1",
                )?;
                match stmt.query_row(rusqlite::params![agent_id], row_to_interrupt) {
                    Ok(req) => Ok(Some(req)),
                    Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                    Err(e) => Err(e),
                }
            })
            .await?;
        Ok(result)
    }

    async fn resolve(
        &self,
        id: InterruptId,
        response: serde_json::Value,
    ) -> meridian_core::Result<()> {
        let response_json = response.to_string();
        let now = Utc::now().to_rfc3339();
        self.writer
            .execute(move |conn| {
                conn.execute(
                    "UPDATE interrupt_requests SET status = 'resolved', response = ?1, resolved_at = ?2 WHERE id = ?3",
                    rusqlite::params![response_json, now, id],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    async fn expire(&self, id: InterruptId) -> meridian_core::Result<()> {
        let now = Utc::now().to_rfc3339();
        self.writer
            .execute(move |conn| {
                conn.execute(
                    "UPDATE interrupt_requests SET status = 'expired', resolved_at = ?1 WHERE id = ?2",
                    rusqlite::params![now, id],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    async fn list_pending(&self) -> meridian_core::Result<Vec<InterruptRequest>> {
        let results = self
            .writer
            .execute(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, agent_id, checkpoint_id, interrupt_type, payload, status, response, question_hash, ask_count, created_at, resolved_at
                     FROM interrupt_requests WHERE status = 'pending'
                     ORDER BY created_at",
                )?;
                let rows = stmt
                    .query_map([], row_to_interrupt)?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await?;
        Ok(results)
    }
}

fn row_to_interrupt(row: &rusqlite::Row) -> Result<InterruptRequest, rusqlite::Error> {
    let interrupt_type_str: String = row.get(3)?;
    let payload_str: Option<String> = row.get(4)?;
    let status_str: String = row.get(5)?;
    let response_str: Option<String> = row.get(6)?;
    let created_str: String = row.get(9)?;
    let resolved_str: Option<String> = row.get(10)?;

    let interrupt_type = match interrupt_type_str.as_str() {
        "hitl" => InterruptType::Hitl,
        "pause" => InterruptType::Pause,
        _ => InterruptType::Drain,
    };

    let status = match status_str.as_str() {
        "resolved" => InterruptStatus::Resolved,
        "expired" => InterruptStatus::Expired,
        _ => InterruptStatus::Pending,
    };

    let payload = payload_str
        .map(|s| serde_json::from_str(&s))
        .transpose()
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(e))
        })?;

    let response = response_str
        .map(|s| serde_json::from_str(&s))
        .transpose()
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(6, rusqlite::types::Type::Text, Box::new(e))
        })?;

    let resolved_at = resolved_str.map(|s| parse_rfc3339(&s)).transpose()?;

    Ok(InterruptRequest {
        id: row.get(0)?,
        agent_id: row.get(1)?,
        checkpoint_id: row.get(2)?,
        interrupt_type,
        payload,
        status,
        response,
        question_hash: row.get(7)?,
        ask_count: row.get(8)?,
        created_at: parse_rfc3339(&created_str)?,
        resolved_at,
    })
}

#[cfg(test)]
mod tests {
    use crate::SqliteStore;
    use chrono::Utc;
    use meridian_core::checkpoint::InterruptType;
    use meridian_core::id::{AgentId, CheckpointId, InterruptId, ObjectiveId};
    use meridian_core::interrupt::{InterruptRequest, InterruptStatus};
    use meridian_core::store::InterruptStore;

    async fn setup_store_with_agent_and_checkpoint() -> (SqliteStore, AgentId, CheckpointId) {
        let store = SqliteStore::open_in_memory(384).unwrap();
        let agent_id = AgentId::new();
        let obj_id = ObjectiveId::new();
        let cp_id = CheckpointId::new();

        store
            .writer
            .execute(move |conn| {
                let now = Utc::now().to_rfc3339();
                conn.execute(
                    "INSERT INTO agents (id, state, directive, directory, objective_id, spawn_origin_type, created_at, updated_at)
                     VALUES (?1, 'starting', 'continue', '/tmp', ?2, 'operator', ?3, ?4)",
                    rusqlite::params![agent_id, obj_id, &now, &now],
                )?;
                conn.execute(
                    "INSERT INTO checkpoints (id, agent_id, channels, l2_namespace, created_at)
                     VALUES (?1, ?2, '{}', 'general', ?3)",
                    rusqlite::params![cp_id, agent_id, &now],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        (store, agent_id, cp_id)
    }

    fn make_interrupt(agent_id: AgentId, checkpoint_id: CheckpointId) -> InterruptRequest {
        InterruptRequest {
            id: InterruptId::new(),
            agent_id,
            checkpoint_id,
            interrupt_type: InterruptType::Hitl,
            payload: Some(serde_json::json!({"question": "proceed?"})),
            status: InterruptStatus::Pending,
            response: None,
            question_hash: Some("abc123".into()),
            ask_count: 1,
            created_at: Utc::now(),
            resolved_at: None,
        }
    }

    #[tokio::test]
    async fn save_and_get_pending() {
        let (store, agent_id, cp_id) = setup_store_with_agent_and_checkpoint().await;
        let req = make_interrupt(agent_id, cp_id);

        store.save(&req).await.unwrap();
        let pending = store.get_pending(agent_id).await.unwrap().unwrap();
        assert_eq!(pending.id, req.id);
        assert_eq!(pending.interrupt_type, InterruptType::Hitl);
        assert!(matches!(pending.status, InterruptStatus::Pending));
    }

    #[tokio::test]
    async fn resolve_interrupt() {
        let (store, agent_id, cp_id) = setup_store_with_agent_and_checkpoint().await;
        let req = make_interrupt(agent_id, cp_id);
        let req_id = req.id;

        store.save(&req).await.unwrap();
        store
            .resolve(req_id, serde_json::json!("yes"))
            .await
            .unwrap();

        let pending = store.get_pending(agent_id).await.unwrap();
        assert!(pending.is_none());
    }

    #[tokio::test]
    async fn expire_interrupt() {
        let (store, agent_id, cp_id) = setup_store_with_agent_and_checkpoint().await;
        let req = make_interrupt(agent_id, cp_id);
        let req_id = req.id;

        store.save(&req).await.unwrap();
        store.expire(req_id).await.unwrap();

        let pending = store.get_pending(agent_id).await.unwrap();
        assert!(pending.is_none());
    }

    #[tokio::test]
    async fn list_pending_multiple() {
        let (store, agent_id, cp_id) = setup_store_with_agent_and_checkpoint().await;

        let mut req1 = make_interrupt(agent_id, cp_id);
        req1.question_hash = Some("hash1".into());
        store.save(&req1).await.unwrap();

        let mut req2 = make_interrupt(agent_id, cp_id);
        req2.question_hash = Some("hash2".into());
        store.save(&req2).await.unwrap();

        let pending = store.list_pending().await.unwrap();
        assert_eq!(pending.len(), 2);
    }

    #[tokio::test]
    async fn duplicate_hash_increments_count() {
        let (store, agent_id, cp_id) = setup_store_with_agent_and_checkpoint().await;

        let req = make_interrupt(agent_id, cp_id);
        store.save(&req).await.unwrap();

        // Save same hash again
        let mut req2 = make_interrupt(agent_id, cp_id);
        req2.question_hash = req.question_hash.clone();
        store.save(&req2).await.unwrap();

        let pending = store.get_pending(agent_id).await.unwrap().unwrap();
        assert_eq!(pending.ask_count, 2);
    }
}

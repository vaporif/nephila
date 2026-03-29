use crate::util::parse_rfc3339;
use crate::SqliteStore;
use chrono::{DateTime, Utc};
use meridian_core::event::{EventType, McpEvent};
use meridian_core::id::AgentId;
use meridian_core::store::EventStore;

impl EventStore for SqliteStore {
    async fn append(&self, event: McpEvent) -> meridian_core::Result<()> {
        let event_type_str =
            serde_json::to_string(&event.event_type).map_err(meridian_core::MeridianError::from)?;
        let content_str =
            serde_json::to_string(&event.content).map_err(meridian_core::MeridianError::from)?;
        self.writer
            .execute(move |conn| {
                conn.execute(
                    "INSERT INTO events (id, agent_id, event_type, timestamp, content, objective_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![
                        event.id.to_string(),
                        event.agent_id,
                        event_type_str,
                        event.timestamp.to_rfc3339(),
                        content_str,
                        event.objective_id,
                    ],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    async fn get_events(
        &self,
        agent_id: AgentId,
        since: Option<DateTime<Utc>>,
        limit: usize,
    ) -> meridian_core::Result<Vec<McpEvent>> {
        let events = self
            .writer
            .execute(move |conn| {
                let rows = match since {
                    Some(ts) => {
                        let ts_str = ts.to_rfc3339();
                        let mut stmt = conn.prepare(
                            "SELECT id, agent_id, event_type, timestamp, content, objective_id
                             FROM events
                             WHERE agent_id = ?1 AND timestamp > ?2
                             ORDER BY timestamp DESC
                             LIMIT ?3",
                        )?;
                        stmt.query_map(
                            rusqlite::params![agent_id, ts_str, limit as i64],
                            row_to_event,
                        )?
                        .collect::<Result<Vec<_>, _>>()?
                    }
                    None => {
                        let mut stmt = conn.prepare(
                            "SELECT id, agent_id, event_type, timestamp, content, objective_id
                             FROM events
                             WHERE agent_id = ?1
                             ORDER BY timestamp DESC
                             LIMIT ?2",
                        )?;
                        stmt.query_map(
                            rusqlite::params![agent_id, limit as i64],
                            row_to_event,
                        )?
                        .collect::<Result<Vec<_>, _>>()?
                    }
                };
                Ok(rows)
            })
            .await?;
        Ok(events)
    }

    async fn get_tool_calls(
        &self,
        agent_id: AgentId,
    ) -> meridian_core::Result<Vec<McpEvent>> {
        let tool_call_type = serde_json::to_string(&EventType::ToolCall)
            .map_err(meridian_core::MeridianError::from)?;
        let events = self
            .writer
            .execute(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, agent_id, event_type, timestamp, content, objective_id
                     FROM events
                     WHERE agent_id = ?1 AND event_type = ?2
                     ORDER BY timestamp DESC",
                )?;
                let rows = stmt
                    .query_map(
                        rusqlite::params![agent_id, tool_call_type],
                        row_to_event,
                    )?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await?;
        Ok(events)
    }
}

fn row_to_event(row: &rusqlite::Row) -> Result<McpEvent, rusqlite::Error> {
    let id_str: String = row.get(0)?;
    let event_type_str: String = row.get(2)?;
    let timestamp_str: String = row.get(3)?;
    let content_str: String = row.get(4)?;

    Ok(McpEvent {
        id: uuid::Uuid::parse_str(&id_str).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?,
        agent_id: row.get(1)?,
        event_type: serde_json::from_str(&event_type_str).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?,
        timestamp: parse_rfc3339(&timestamp_str)?,
        content: serde_json::from_str(&content_str).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?,
        objective_id: row.get(5)?,
    })
}

#[cfg(test)]
mod tests {
    use crate::test_util::make_agent_and_store;
    use chrono::Utc;
    use meridian_core::event::{EventType, McpEvent};
    use meridian_core::id::AgentId;
    use meridian_core::store::EventStore;

    fn make_event(agent_id: AgentId, event_type: EventType) -> McpEvent {
        McpEvent {
            id: uuid::Uuid::new_v4(),
            agent_id,
            event_type,
            timestamp: Utc::now(),
            content: serde_json::json!({"tool": "test"}),
            objective_id: None,
        }
    }

    #[tokio::test]
    async fn append_and_get_events() {
        let (store, agent_id) = make_agent_and_store().await;

        store
            .append(make_event(agent_id, EventType::ToolCall))
            .await
            .unwrap();
        store
            .append(make_event(agent_id, EventType::StateChange))
            .await
            .unwrap();
        store
            .append(make_event(agent_id, EventType::ToolResult))
            .await
            .unwrap();

        let all = store.get_events(agent_id, None, 10).await.unwrap();
        assert_eq!(all.len(), 3);
    }

    #[tokio::test]
    async fn get_events_with_limit() {
        let (store, agent_id) = make_agent_and_store().await;

        for _ in 0..5 {
            store
                .append(make_event(agent_id, EventType::ToolCall))
                .await
                .unwrap();
        }

        let limited = store.get_events(agent_id, None, 2).await.unwrap();
        assert_eq!(limited.len(), 2);
    }

    #[tokio::test]
    async fn get_tool_calls_filter() {
        let (store, agent_id) = make_agent_and_store().await;

        store
            .append(make_event(agent_id, EventType::ToolCall))
            .await
            .unwrap();
        store
            .append(make_event(agent_id, EventType::StateChange))
            .await
            .unwrap();
        store
            .append(make_event(agent_id, EventType::ToolCall))
            .await
            .unwrap();

        let tool_calls = store.get_tool_calls(agent_id).await.unwrap();
        assert_eq!(tool_calls.len(), 2);
    }

    #[tokio::test]
    async fn get_events_empty() {
        let (store, agent_id) = make_agent_and_store().await;
        let events = store.get_events(agent_id, None, 10).await.unwrap();
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn get_tool_calls_empty() {
        let (store, agent_id) = make_agent_and_store().await;
        let calls = store.get_tool_calls(agent_id).await.unwrap();
        assert!(calls.is_empty());
    }

    #[tokio::test]
    async fn get_events_since_filters_correctly() {
        let (store, agent_id) = make_agent_and_store().await;

        let before = Utc::now();
        store
            .append(make_event(agent_id, EventType::ToolCall))
            .await
            .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let midpoint = Utc::now();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        store
            .append(make_event(agent_id, EventType::StateChange))
            .await
            .unwrap();

        let since_mid = store
            .get_events(agent_id, Some(midpoint), 10)
            .await
            .unwrap();
        assert_eq!(since_mid.len(), 1);

        let since_before = store
            .get_events(agent_id, Some(before), 10)
            .await
            .unwrap();
        assert_eq!(since_before.len(), 2);
    }
}

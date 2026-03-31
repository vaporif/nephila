use crate::SqliteStore;
use crate::util::parse_rfc3339;
use chrono::{DateTime, Utc};
use meridian_eventsourcing::envelope::EventEnvelope;
use meridian_eventsourcing::id::{EventId, TraceId};
use meridian_eventsourcing::outcome::Outcome;
use meridian_eventsourcing::snapshot::{ContextSnapshot, Snapshot};
use meridian_eventsourcing::store::{DomainEventStore, EventStoreError};
use std::collections::HashMap;

impl DomainEventStore for SqliteStore {
    async fn append(&self, envelope: &EventEnvelope) -> Result<(), EventStoreError> {
        let id = envelope.id.0.to_string();
        let aggregate_type = envelope.aggregate_type.clone();
        let aggregate_id = envelope.aggregate_id.clone();
        let sequence = envelope.sequence;
        let event_type = envelope.event_type.clone();
        let payload = serde_json::to_string(&envelope.payload)
            .map_err(|e| EventStoreError::Serialization(e.to_string()))?;
        let trace_id = envelope.trace_id.0.clone();
        let outcome = serde_json::to_string(&envelope.outcome)
            .map_err(|e| EventStoreError::Serialization(e.to_string()))?;
        let timestamp = envelope.timestamp.to_rfc3339();
        let context_snapshot = serde_json::to_string(&envelope.context_snapshot)
            .map_err(|e| EventStoreError::Serialization(e.to_string()))?;
        let metadata = serde_json::to_string(&envelope.metadata)
            .map_err(|e| EventStoreError::Serialization(e.to_string()))?;

        self.writer
            .execute(move |conn| {
                conn.execute(
                    "INSERT INTO domain_events (id, aggregate_type, aggregate_id, sequence, event_type, payload, trace_id, outcome, timestamp, context_snapshot, metadata)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    rusqlite::params![
                        id,
                        aggregate_type,
                        aggregate_id,
                        sequence as i64,
                        event_type,
                        payload,
                        trace_id,
                        outcome,
                        timestamp,
                        context_snapshot,
                        metadata,
                    ],
                )?;
                Ok(())
            })
            .await
            .map_err(|e| EventStoreError::Storage(e.to_string()))?;
        Ok(())
    }

    async fn load_events(
        &self,
        aggregate_type: &str,
        aggregate_id: &str,
        since_sequence: u64,
    ) -> Result<Vec<EventEnvelope>, EventStoreError> {
        let agg_type = aggregate_type.to_string();
        let agg_id = aggregate_id.to_string();
        let envelopes = self
            .writer
            .execute(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, aggregate_type, aggregate_id, sequence, event_type, payload, trace_id, outcome, timestamp, context_snapshot, metadata
                     FROM domain_events
                     WHERE aggregate_type = ?1 AND aggregate_id = ?2 AND sequence > ?3
                     ORDER BY sequence ASC",
                )?;
                let rows = stmt
                    .query_map(
                        rusqlite::params![agg_type, agg_id, since_sequence as i64],
                        row_to_envelope,
                    )?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await
            .map_err(|e| EventStoreError::Storage(e.to_string()))?;
        Ok(envelopes)
    }

    async fn load_by_trace_id(
        &self,
        trace_id: &TraceId,
    ) -> Result<Vec<EventEnvelope>, EventStoreError> {
        let tid = trace_id.0.clone();
        let envelopes = self
            .writer
            .execute(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, aggregate_type, aggregate_id, sequence, event_type, payload, trace_id, outcome, timestamp, context_snapshot, metadata
                     FROM domain_events
                     WHERE trace_id = ?1
                     ORDER BY timestamp ASC",
                )?;
                let rows = stmt
                    .query_map(rusqlite::params![tid], row_to_envelope)?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await
            .map_err(|e| EventStoreError::Storage(e.to_string()))?;
        Ok(envelopes)
    }

    async fn load_by_time_range(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<EventEnvelope>, EventStoreError> {
        let from_str = from.to_rfc3339();
        let to_str = to.to_rfc3339();
        let envelopes = self
            .writer
            .execute(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, aggregate_type, aggregate_id, sequence, event_type, payload, trace_id, outcome, timestamp, context_snapshot, metadata
                     FROM domain_events
                     WHERE timestamp >= ?1 AND timestamp <= ?2
                     ORDER BY timestamp ASC",
                )?;
                let rows = stmt
                    .query_map(rusqlite::params![from_str, to_str], row_to_envelope)?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await
            .map_err(|e| EventStoreError::Storage(e.to_string()))?;
        Ok(envelopes)
    }

    async fn save_snapshot(&self, snapshot: &Snapshot) -> Result<(), EventStoreError> {
        let aggregate_type = snapshot.aggregate_type.clone();
        let aggregate_id = snapshot.aggregate_id.clone();
        let sequence = snapshot.sequence;
        let state = serde_json::to_string(&snapshot.state)
            .map_err(|e| EventStoreError::Serialization(e.to_string()))?;
        let timestamp = snapshot.timestamp.to_rfc3339();

        self.writer
            .execute(move |conn| {
                conn.execute(
                    "INSERT OR REPLACE INTO aggregate_snapshots (aggregate_type, aggregate_id, sequence, state, timestamp)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![aggregate_type, aggregate_id, sequence as i64, state, timestamp],
                )?;
                Ok(())
            })
            .await
            .map_err(|e| EventStoreError::Storage(e.to_string()))?;
        Ok(())
    }

    async fn load_latest_snapshot(
        &self,
        aggregate_type: &str,
        aggregate_id: &str,
    ) -> Result<Option<Snapshot>, EventStoreError> {
        let agg_type = aggregate_type.to_string();
        let agg_id = aggregate_id.to_string();
        let snapshot = self
            .writer
            .execute(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT aggregate_type, aggregate_id, sequence, state, timestamp
                     FROM aggregate_snapshots
                     WHERE aggregate_type = ?1 AND aggregate_id = ?2
                     ORDER BY sequence DESC
                     LIMIT 1",
                )?;
                let mut rows =
                    stmt.query_map(rusqlite::params![agg_type, agg_id], row_to_snapshot)?;
                match rows.next() {
                    Some(row) => Ok(Some(row?)),
                    None => Ok(None),
                }
            })
            .await
            .map_err(|e| EventStoreError::Storage(e.to_string()))?;
        Ok(snapshot)
    }
}

fn row_to_envelope(row: &rusqlite::Row) -> Result<EventEnvelope, rusqlite::Error> {
    let id_str: String = row.get(0)?;
    let sequence: i64 = row.get(3)?;
    let payload_str: String = row.get(5)?;
    let outcome_str: String = row.get(7)?;
    let timestamp_str: String = row.get(8)?;
    let context_snapshot_str: String = row.get(9)?;
    let metadata_str: String = row.get(10)?;

    let id = uuid::Uuid::parse_str(&id_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let payload: serde_json::Value = serde_json::from_str(&payload_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let outcome: Option<Outcome> = serde_json::from_str(&outcome_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let context_snapshot: Option<ContextSnapshot> = serde_json::from_str(&context_snapshot_str)
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?;
    let metadata: HashMap<String, String> = serde_json::from_str(&metadata_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;

    Ok(EventEnvelope {
        id: EventId(id),
        aggregate_type: row.get(1)?,
        aggregate_id: row.get(2)?,
        sequence: sequence as u64,
        event_type: row.get(4)?,
        payload,
        trace_id: TraceId(row.get(6)?),
        outcome,
        timestamp: parse_rfc3339(&timestamp_str)?,
        context_snapshot,
        metadata,
    })
}

fn row_to_snapshot(row: &rusqlite::Row) -> Result<Snapshot, rusqlite::Error> {
    let sequence: i64 = row.get(2)?;
    let state_str: String = row.get(3)?;
    let timestamp_str: String = row.get(4)?;

    let state: serde_json::Value = serde_json::from_str(&state_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;

    Ok(Snapshot {
        aggregate_type: row.get(0)?,
        aggregate_id: row.get(1)?,
        sequence: sequence as u64,
        state,
        timestamp: parse_rfc3339(&timestamp_str)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SqliteStore;
    use chrono::Utc;
    use meridian_eventsourcing::id::EventId;

    fn make_store() -> SqliteStore {
        SqliteStore::open_in_memory(384).unwrap()
    }

    fn make_envelope(aggregate_type: &str, aggregate_id: &str, sequence: u64) -> EventEnvelope {
        EventEnvelope {
            id: EventId::new(),
            aggregate_type: aggregate_type.to_string(),
            aggregate_id: aggregate_id.to_string(),
            sequence,
            event_type: "test_event".to_string(),
            payload: serde_json::json!({"key": "value"}),
            trace_id: TraceId("trace-1".to_string()),
            outcome: Some(Outcome::Success),
            timestamp: Utc::now(),
            context_snapshot: None,
            metadata: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn append_and_load_events() {
        let store = make_store();
        let env1 = make_envelope("agent", "a1", 1);
        let env2 = make_envelope("agent", "a1", 2);

        store.append(&env1).await.unwrap();
        store.append(&env2).await.unwrap();

        let events = store.load_events("agent", "a1", 0).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].sequence, 1);
        assert_eq!(events[1].sequence, 2);
    }

    #[tokio::test]
    async fn load_events_since_sequence() {
        let store = make_store();
        for seq in 1..=5 {
            store
                .append(&make_envelope("agent", "a1", seq))
                .await
                .unwrap();
        }

        let events = store.load_events("agent", "a1", 3).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].sequence, 4);
    }

    #[tokio::test]
    async fn load_by_trace_id() {
        let store = make_store();
        store
            .append(&make_envelope("agent", "a1", 1))
            .await
            .unwrap();

        let events = store
            .load_by_trace_id(&TraceId("trace-1".to_string()))
            .await
            .unwrap();
        assert_eq!(events.len(), 1);

        let empty = store
            .load_by_trace_id(&TraceId("nonexistent".to_string()))
            .await
            .unwrap();
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn save_and_load_snapshot() {
        let store = make_store();
        let snapshot = Snapshot {
            aggregate_type: "agent".to_string(),
            aggregate_id: "a1".to_string(),
            sequence: 10,
            state: serde_json::json!({"counter": 42}),
            timestamp: Utc::now(),
        };

        store.save_snapshot(&snapshot).await.unwrap();

        let loaded = store
            .load_latest_snapshot("agent", "a1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.sequence, 10);
        assert_eq!(loaded.state, serde_json::json!({"counter": 42}));
    }

    #[tokio::test]
    async fn load_latest_snapshot_returns_highest_sequence() {
        let store = make_store();
        for seq in [5, 10, 15] {
            let snapshot = Snapshot {
                aggregate_type: "agent".to_string(),
                aggregate_id: "a1".to_string(),
                sequence: seq,
                state: serde_json::json!({"seq": seq}),
                timestamp: Utc::now(),
            };
            store.save_snapshot(&snapshot).await.unwrap();
        }

        let loaded = store
            .load_latest_snapshot("agent", "a1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.sequence, 15);
    }

    #[tokio::test]
    async fn load_latest_snapshot_none_when_missing() {
        let store = make_store();
        let result = store
            .load_latest_snapshot("agent", "nonexistent")
            .await
            .unwrap();
        assert!(result.is_none());
    }
}

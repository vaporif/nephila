use crate::SqliteStore;
use crate::util::parse_rfc3339;
use chrono::{DateTime, Utc};
use meridian_eventsourcing::id::{SpanId, TraceId};
use meridian_eventsourcing::store::TracingStoreError;
use meridian_eventsourcing::tracing::{SpanEvent, SpanLevel, SpanStatus, StoredSpan};
use std::collections::HashMap;

impl meridian_eventsourcing::store::TracingStore for SqliteStore {
    async fn record_span(&self, span: &StoredSpan) -> Result<(), TracingStoreError> {
        let span_id = span.span_id.0.clone();
        let trace_id = span.trace_id.0.clone();
        let parent_span_id = span.parent_span_id.as_ref().map(|s| s.0.clone());
        let name = span.name.clone();
        let level = serde_json::to_string(&span.level)
            .map_err(|e| TracingStoreError::Serialization(e.to_string()))?;
        let target = span.target.clone();
        let start_time = span.start_time.to_rfc3339();
        let end_time = span.end_time.map(|t| t.to_rfc3339());
        let duration_us = span.duration_us.map(|d| d as i64);
        let attributes = serde_json::to_string(&span.attributes)
            .map_err(|e| TracingStoreError::Serialization(e.to_string()))?;
        let events = serde_json::to_string(&span.events)
            .map_err(|e| TracingStoreError::Serialization(e.to_string()))?;
        let status = serde_json::to_string(&span.status)
            .map_err(|e| TracingStoreError::Serialization(e.to_string()))?;

        self.writer
            .execute(move |conn| {
                conn.execute(
                    "INSERT INTO spans (span_id, trace_id, parent_span_id, name, level, target, start_time, end_time, duration_us, attributes, events, status)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                    rusqlite::params![
                        span_id,
                        trace_id,
                        parent_span_id,
                        name,
                        level,
                        target,
                        start_time,
                        end_time,
                        duration_us,
                        attributes,
                        events,
                        status,
                    ],
                )?;
                Ok(())
            })
            .await
            .map_err(|e| TracingStoreError::Storage(e.to_string()))?;
        Ok(())
    }

    async fn load_spans_by_trace(
        &self,
        trace_id: &TraceId,
    ) -> Result<Vec<StoredSpan>, TracingStoreError> {
        let tid = trace_id.0.clone();
        let spans = self
            .writer
            .execute(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT span_id, trace_id, parent_span_id, name, level, target, start_time, end_time, duration_us, attributes, events, status
                     FROM spans
                     WHERE trace_id = ?1
                     ORDER BY start_time ASC",
                )?;
                let rows = stmt
                    .query_map(rusqlite::params![tid], row_to_span)?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await
            .map_err(|e| TracingStoreError::Storage(e.to_string()))?;
        Ok(spans)
    }

    async fn load_spans_by_time_range(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<StoredSpan>, TracingStoreError> {
        let from_str = from.to_rfc3339();
        let to_str = to.to_rfc3339();
        let spans = self
            .writer
            .execute(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT span_id, trace_id, parent_span_id, name, level, target, start_time, end_time, duration_us, attributes, events, status
                     FROM spans
                     WHERE start_time >= ?1 AND start_time <= ?2
                     ORDER BY start_time ASC",
                )?;
                let rows = stmt
                    .query_map(rusqlite::params![from_str, to_str], row_to_span)?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await
            .map_err(|e| TracingStoreError::Storage(e.to_string()))?;
        Ok(spans)
    }

    async fn load_child_spans(
        &self,
        parent_span_id: &SpanId,
    ) -> Result<Vec<StoredSpan>, TracingStoreError> {
        let pid = parent_span_id.0.clone();
        let spans = self
            .writer
            .execute(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT span_id, trace_id, parent_span_id, name, level, target, start_time, end_time, duration_us, attributes, events, status
                     FROM spans
                     WHERE parent_span_id = ?1
                     ORDER BY start_time ASC",
                )?;
                let rows = stmt
                    .query_map(rusqlite::params![pid], row_to_span)?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await
            .map_err(|e| TracingStoreError::Storage(e.to_string()))?;
        Ok(spans)
    }
}

fn row_to_span(row: &rusqlite::Row) -> Result<StoredSpan, rusqlite::Error> {
    let parent_span_id_opt: Option<String> = row.get(2)?;
    let level_str: String = row.get(4)?;
    let start_time_str: String = row.get(6)?;
    let end_time_str: Option<String> = row.get(7)?;
    let duration_us: Option<i64> = row.get(8)?;
    let attributes_str: String = row.get(9)?;
    let events_str: String = row.get(10)?;
    let status_str: String = row.get(11)?;

    let level: SpanLevel = serde_json::from_str(&level_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let attributes: HashMap<String, serde_json::Value> = serde_json::from_str(&attributes_str)
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?;
    let events: Vec<SpanEvent> = serde_json::from_str(&events_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let status: SpanStatus = serde_json::from_str(&status_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;

    let end_time = end_time_str.as_deref().map(parse_rfc3339).transpose()?;

    Ok(StoredSpan {
        span_id: SpanId(row.get(0)?),
        trace_id: TraceId(row.get(1)?),
        parent_span_id: parent_span_id_opt.map(SpanId),
        name: row.get(3)?,
        level,
        target: row.get(5)?,
        start_time: parse_rfc3339(&start_time_str)?,
        end_time,
        duration_us: duration_us.map(|d| d as u64),
        attributes,
        events,
        status,
    })
}

#[cfg(test)]
mod tests {
    use crate::SqliteStore;
    use chrono::Utc;
    use meridian_eventsourcing::id::{SpanId, TraceId};
    use meridian_eventsourcing::store::TracingStore;
    use meridian_eventsourcing::tracing::{SpanLevel, SpanStatus, StoredSpan};
    use std::collections::HashMap;

    fn make_store() -> SqliteStore {
        SqliteStore::open_in_memory(384).unwrap()
    }

    fn make_span(span_id: &str, trace_id: &str, parent: Option<&str>) -> StoredSpan {
        StoredSpan {
            span_id: SpanId(span_id.to_string()),
            trace_id: TraceId(trace_id.to_string()),
            parent_span_id: parent.map(|p| SpanId(p.to_string())),
            name: "test_span".to_string(),
            level: SpanLevel::Info,
            target: "test::target".to_string(),
            start_time: Utc::now(),
            end_time: Some(Utc::now()),
            duration_us: Some(1000),
            attributes: HashMap::new(),
            events: vec![],
            status: SpanStatus::Ok,
        }
    }

    #[tokio::test]
    async fn record_and_load_by_trace() {
        let store = make_store();
        let span = make_span("s1", "t1", None);
        store.record_span(&span).await.unwrap();

        let spans = store
            .load_spans_by_trace(&TraceId("t1".to_string()))
            .await
            .unwrap();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].span_id.0, "s1");
    }

    #[tokio::test]
    async fn load_child_spans() {
        let store = make_store();
        let parent = make_span("parent", "t1", None);
        let child1 = make_span("child1", "t1", Some("parent"));
        let child2 = make_span("child2", "t1", Some("parent"));

        store.record_span(&parent).await.unwrap();
        store.record_span(&child1).await.unwrap();
        store.record_span(&child2).await.unwrap();

        let children = store
            .load_child_spans(&SpanId("parent".to_string()))
            .await
            .unwrap();
        assert_eq!(children.len(), 2);
    }

    #[tokio::test]
    async fn load_spans_empty() {
        let store = make_store();
        let spans = store
            .load_spans_by_trace(&TraceId("nonexistent".to_string()))
            .await
            .unwrap();
        assert!(spans.is_empty());
    }

    #[tokio::test]
    async fn span_with_error_status() {
        let store = make_store();
        let mut span = make_span("s1", "t1", None);
        span.status = SpanStatus::Error("something failed".to_string());

        store.record_span(&span).await.unwrap();

        let spans = store
            .load_spans_by_trace(&TraceId("t1".to_string()))
            .await
            .unwrap();
        assert_eq!(
            spans[0].status,
            SpanStatus::Error("something failed".to_string())
        );
    }
}

//! Type-level tests for `append_batch_with_blobs` (Task 3 step 11).
//!
//! The connector-side call site lives in `crates/connector/src/session.rs`
//! and is wired in the slice-1b integration window (step 12, deferred). This
//! test drives the API directly with a hand-constructed
//! `SessionEvent::ToolResult` envelope to verify atomicity and the
//! `BlobReader` round-trip.

use chrono::Utc;
use nephila_core::session_event::{SessionEvent, ToolResultPayload};
use nephila_eventsourcing::envelope::{EventEnvelope, NewEventEnvelope};
use nephila_eventsourcing::id::{EventId, TraceId};
use nephila_eventsourcing::store::DomainEventStore;
use nephila_store::SqliteStore;
use nephila_store::blob::{BlobReader, SqliteBlobReader, prepare_blob};
use std::collections::HashMap;
use uuid::Uuid;

#[tokio::test]
async fn append_batch_with_blobs_round_trips() {
    let store = SqliteStore::open_in_memory(384).unwrap();
    let agg_id = Uuid::new_v4().to_string();

    let big = vec![b'x'; 300 * 1024];
    let prepared = prepare_blob(&big);
    let event = SessionEvent::ToolResult {
        tool_use_id: "tu1".into(),
        output: ToolResultPayload::Spilled {
            hash: prepared.hash.clone(),
            original_len: prepared.original_len,
            snippet: prepared.snippet.clone(),
        },
        is_error: false,
        ts: Utc::now(),
    };
    let env = EventEnvelope::new(NewEventEnvelope {
        id: EventId::new(),
        aggregate_type: "session".into(),
        aggregate_id: agg_id.clone(),
        event_type: event.kind().into(),
        payload: serde_json::to_value(&event).unwrap(),
        trace_id: TraceId("t".into()),
        outcome: None,
        timestamp: Utc::now(),
        context_snapshot: None,
        metadata: HashMap::new(),
    });
    let prepared_hash = prepared.hash.clone();
    let prepared_len = prepared.original_len;
    let seqs = store
        .append_batch_with_blobs(vec![env], vec![prepared])
        .await
        .unwrap();
    assert_eq!(seqs, vec![1]);

    let reader = SqliteBlobReader::new(store.read_pool());
    let bytes = reader.get(&prepared_hash).await.unwrap();
    assert_eq!(bytes.len(), prepared_len as usize);
    assert!(bytes.iter().all(|&b| b == b'x'));
}

#[tokio::test]
async fn append_batch_with_blobs_idempotent_on_duplicate_hash() {
    let store = SqliteStore::open_in_memory(384).unwrap();
    let agg_id = Uuid::new_v4().to_string();
    let payload = b"hello world".repeat(1000);
    let p = prepare_blob(&payload);

    fn env_for(agg_id: &str, hash: &str) -> EventEnvelope {
        let event = SessionEvent::ToolResult {
            tool_use_id: "tu".into(),
            output: ToolResultPayload::Spilled {
                hash: hash.into(),
                original_len: 11000,
                snippet: "hello".into(),
            },
            is_error: false,
            ts: Utc::now(),
        };
        EventEnvelope::new(NewEventEnvelope {
            id: EventId::new(),
            aggregate_type: "session".into(),
            aggregate_id: agg_id.into(),
            event_type: event.kind().into(),
            payload: serde_json::to_value(&event).unwrap(),
            trace_id: TraceId("t".into()),
            outcome: None,
            timestamp: Utc::now(),
            context_snapshot: None,
            metadata: HashMap::new(),
        })
    }

    let env1 = env_for(&agg_id, &p.hash);
    let env2 = env_for(&agg_id, &p.hash);
    let _ = store
        .append_batch_with_blobs(vec![env1], vec![p.clone()])
        .await
        .unwrap();
    let _ = store
        .append_batch_with_blobs(vec![env2], vec![p.clone()])
        .await
        .unwrap();

    // Both events present, single blob row.
    let events = store.load_events("session", &agg_id, 0).await.unwrap();
    assert_eq!(events.len(), 2);

    let reader = SqliteBlobReader::new(store.read_pool());
    let bytes = reader.get(&p.hash).await.unwrap();
    assert_eq!(bytes.len(), payload.len());
}

#[tokio::test]
async fn blob_reader_get_missing_returns_not_found() {
    let store = SqliteStore::open_in_memory(384).unwrap();
    let reader = SqliteBlobReader::new(store.read_pool());
    let result = reader.get("nonexistent").await;
    match result {
        Err(nephila_store::blob::BlobError::NotFound) => {}
        other => panic!("expected NotFound, got {other:?}"),
    }
}

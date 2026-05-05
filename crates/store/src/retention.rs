//! Default retention policy for terminated session aggregates.
//!
//! Ships the policy function; the daily-sweep scheduler that calls it lives
//! in `SessionRegistry`. The policy snapshots the final aggregate state,
//! then prunes everything older than `last_seq - RETENTION_TAIL`.

use chrono::Utc;
use nephila_eventsourcing::aggregate::EventSourced;
use nephila_eventsourcing::envelope::EventEnvelope;
use nephila_eventsourcing::snapshot::Snapshot;
use nephila_eventsourcing::store::{DomainEventStore, EventStoreError};

/// Number of trailing events to retain after a final snapshot is taken.
/// Lets consumers replay the recent past for diagnostics without having to
/// reconstruct from the snapshot.
pub const RETENTION_TAIL: u64 = 200;

/// Apply the default retention policy for a terminated session aggregate.
///
/// 1. Replay all events; build the final state.
/// 2. Save a snapshot at `last_seq`.
/// 3. Prune events with `sequence < last_seq - RETENTION_TAIL`.
///
/// Returns the number of pruned rows.
pub async fn apply_session_retention<S, A>(
    store: &S,
    aggregate_id: &str,
) -> Result<u64, EventStoreError>
where
    S: DomainEventStore,
    A: EventSourced + serde::Serialize,
    EventEnvelope: ApplyEnvelope<A>,
{
    let agg_type = A::aggregate_type();
    let events = store.load_events(agg_type, aggregate_id, 0).await?;
    if events.is_empty() {
        return Ok(0);
    }
    let mut state = A::default_state();
    let mut last_seq = 0u64;
    for env in &events {
        state = <EventEnvelope as ApplyEnvelope<A>>::apply(env, state)?;
        last_seq = env.sequence;
    }

    let snap = Snapshot {
        aggregate_type: agg_type.into(),
        aggregate_id: aggregate_id.into(),
        sequence: last_seq,
        state: serde_json::to_value(&state)
            .map_err(|e| EventStoreError::Serialization(e.to_string()))?,
        timestamp: Utc::now(),
    };
    store.save_snapshot(&snap).await?;

    // Keep the last RETENTION_TAIL events; delete strictly older.
    // `prune_aggregate` deletes `WHERE sequence < before`, so to keep
    // sequences `(last_seq - RETENTION_TAIL + 1) ..= last_seq`, the cutoff
    // is `last_seq - RETENTION_TAIL + 1`.
    let prune_below = last_seq.saturating_sub(RETENTION_TAIL).saturating_add(1);
    if prune_below <= 1 {
        return Ok(0);
    }
    store
        .prune_aggregate(agg_type, aggregate_id, prune_below)
        .await
}

/// Glue trait — bridges the generic `EventSourced::apply` (which takes
/// `&Event`) with `EventEnvelope` (which carries the serialized payload).
/// Each aggregate that wants retention impls this once.
pub trait ApplyEnvelope<A: EventSourced> {
    fn apply(env: &EventEnvelope, state: A) -> Result<A, EventStoreError>;
}

impl ApplyEnvelope<nephila_core::session::Session> for EventEnvelope {
    fn apply(
        env: &EventEnvelope,
        state: nephila_core::session::Session,
    ) -> Result<nephila_core::session::Session, EventStoreError> {
        state
            .apply_envelope(env)
            .map_err(|e| EventStoreError::Serialization(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SqliteStore;
    use nephila_core::id::AgentId;
    use nephila_core::session::Session;
    use nephila_core::session_event::SessionEvent;
    use nephila_eventsourcing::id::{EventId, TraceId};
    use std::collections::HashMap;
    use uuid::Uuid;

    fn envelope(event: &SessionEvent, agg_id: &str) -> EventEnvelope {
        EventEnvelope {
            id: EventId::new(),
            aggregate_type: "session".into(),
            aggregate_id: agg_id.into(),
            sequence: 0,
            event_type: event.kind().into(),
            payload: serde_json::to_value(event).unwrap(),
            trace_id: TraceId("t".into()),
            outcome: None,
            timestamp: Utc::now(),
            context_snapshot: None,
            metadata: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn retention_keeps_tail_and_prunes_head() {
        let store = SqliteStore::open_in_memory(384).unwrap();
        let session_id = Uuid::new_v4();
        let agent_id = AgentId::new();
        let agg_id = session_id.to_string();

        // Seed 1000 events: 1 SessionStarted + 998 AssistantMessages + 1 SessionEnded.
        let mut events = Vec::with_capacity(1000);
        events.push(SessionEvent::SessionStarted {
            session_id,
            agent_id,
            model: None,
            working_dir: std::path::PathBuf::from("/tmp"),
            mcp_endpoint: "mcp".into(),
            resumed: false,
            ts: Utc::now(),
        });
        for i in 0..998 {
            events.push(SessionEvent::AssistantMessage {
                message_id: format!("m{i}"),
                seq_in_message: 0,
                delta_text: "x".into(),
                is_final: false,
                truncated: false,
                ts: Utc::now(),
            });
        }
        events.push(SessionEvent::SessionEnded { ts: Utc::now() });

        let envelopes: Vec<_> = events.iter().map(|e| envelope(e, &agg_id)).collect();
        let _ = store.append_batch(envelopes).await.unwrap();

        let pruned = apply_session_retention::<_, Session>(&store, &agg_id)
            .await
            .unwrap();
        // 1000 events total; retention keeps last 200 (seqs 801..=1000),
        // prunes 1..=800 = 800 rows.
        assert_eq!(pruned, 1000 - RETENTION_TAIL);

        let remaining = store.load_events("session", &agg_id, 0).await.unwrap();
        assert_eq!(remaining.len() as u64, RETENTION_TAIL);
        assert_eq!(remaining[0].sequence, 1000 - RETENTION_TAIL + 1);
        assert_eq!(remaining.last().unwrap().sequence, 1000);
    }
}

//! Broadcast bookkeeping for `subscribe_after`.
//!
//! Each `(aggregate_type, aggregate_id)` gets its own bounded
//! `tokio::sync::broadcast` channel. The writer thread publishes to the
//! sender after committing each batch; subscribers consume via
//! `broadcast::Receiver` joined with a backfill from disk.

use dashmap::DashMap;
use nephila_eventsourcing::envelope::EventEnvelope;
#[cfg(any(test, feature = "test-seam"))]
use std::sync::Arc;
use tokio::sync::broadcast;

const BROADCAST_CAPACITY: usize = 4096;

#[derive(Default)]
pub struct BroadcastRegistry {
    senders: DashMap<(String, String), broadcast::Sender<EventEnvelope>>,
}

impl BroadcastRegistry {
    /// Lookup-or-create the sender for an aggregate. Always returns a sender
    /// with at least one slot in the channel; new subscribers attach via
    /// `Sender::subscribe`.
    pub fn sender_for(&self, agg_type: &str, agg_id: &str) -> broadcast::Sender<EventEnvelope> {
        let key = (agg_type.to_owned(), agg_id.to_owned());
        if let Some(existing) = self.senders.get(&key) {
            return existing.value().clone();
        }
        let entry = self
            .senders
            .entry(key)
            .or_insert_with(|| broadcast::channel(BROADCAST_CAPACITY).0);
        entry.value().clone()
    }
}

impl std::fmt::Debug for BroadcastRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BroadcastRegistry")
            .field("aggregates", &self.senders.len())
            .finish()
    }
}

/// Test-only hooks injected into `subscribe_after` between listener-attach and
/// head-snapshot. Used by the listener-first ordering test in
/// `tests/subscribe_after_ordering.rs`.
///
/// Gated behind `cfg(any(test, feature = "test-seam"))` so it is absent
/// from release builds (per ADR-0002).
#[cfg(any(test, feature = "test-seam"))]
#[derive(Default, Clone)]
pub struct SubscribeAfterHooks {
    pub pre_head_snapshot: Option<Arc<dyn Fn() + Send + Sync>>,
}

#[cfg(any(test, feature = "test-seam"))]
impl std::fmt::Debug for SubscribeAfterHooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubscribeAfterHooks")
            .field("pre_head_snapshot", &self.pre_head_snapshot.is_some())
            .finish()
    }
}

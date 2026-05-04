//! Observability counters and histograms for the slice-1b store primitives.
//!
//! The codebase doesn't yet use a centralised metrics backend (no `metrics`
//! crate, no `prometheus`); existing observability flows through `tracing`.
//! These helpers wrap `tracing::info!` events with a stable target string so
//! a future metrics backend can scrape them without touching call sites.

use tracing::info;

const TARGET: &str = "nephila_store::metrics";

#[inline]
pub fn record_lagged_recovery(aggregate_type: &str, aggregate_id: &str, dropped: u64) {
    info!(
        target: TARGET,
        metric = "subscribe_after.lagged_recovery_total",
        aggregate_type = aggregate_type,
        aggregate_id = aggregate_id,
        dropped = dropped,
        "subscribe_after lag recovery",
    );
}

#[inline]
pub fn record_backfill_rows(aggregate_type: &str, aggregate_id: &str, rows: u64) {
    info!(
        target: TARGET,
        metric = "subscribe_after.backfill_rows",
        aggregate_type = aggregate_type,
        aggregate_id = aggregate_id,
        rows = rows,
        "subscribe_after backfill",
    );
}

#[inline]
pub fn record_head_lag(aggregate_type: &str, aggregate_id: &str, head_lag: u64) {
    info!(
        target: TARGET,
        metric = "subscribe_after.head_lag",
        aggregate_type = aggregate_type,
        aggregate_id = aggregate_id,
        head_lag = head_lag,
        "subscribe_after head lag",
    );
}

#[inline]
pub fn record_append_batch_size(aggregate_type: &str, aggregate_id: &str, size: u64) {
    info!(
        target: TARGET,
        metric = "append_batch.size",
        aggregate_type = aggregate_type,
        aggregate_id = aggregate_id,
        size = size,
        "append_batch size",
    );
}

#[inline]
pub fn record_session_event_truncated(aggregate_id: &str) {
    info!(
        target: TARGET,
        metric = "session_event.payload_truncated",
        aggregate_id = aggregate_id,
        "session event truncated",
    );
}

#[inline]
pub fn record_session_event_blob_spilled(aggregate_id: &str, original_len: u64) {
    info!(
        target: TARGET,
        metric = "session_event.blob_spilled",
        aggregate_id = aggregate_id,
        original_len = original_len,
        "session event spilled to blob store",
    );
}

#[inline]
pub fn record_session_respawn(aggregate_id: &str) {
    info!(
        target: TARGET,
        metric = "session.respawn_total",
        aggregate_id = aggregate_id,
        "session respawn (slice 4 wires this)",
    );
}

#[inline]
pub fn record_session_fallback_to_session_id(aggregate_id: &str) {
    info!(
        target: TARGET,
        metric = "session.fallback_to_session_id",
        aggregate_id = aggregate_id,
        "session fallback to session_id (slice 4 wires this)",
    );
}

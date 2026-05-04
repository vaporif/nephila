//! Lagged-recovery wrapper around `subscribe_after`.
//!
//! Slice 1b consumers (TUI pump, supervisor) wrap their `subscribe_after`
//! call with `resilient_subscribe`, which retries on `Lagged` by re-subscribing
//! from the last seen sequence. If the retry budget is exhausted within a
//! window the wrapper escalates to `EventStoreError::PersistentLag`.
//!
//! The retry counter is sticky across cooldowns to prevent livelock under
//! sustained burst traffic — only a `QUIET_PERIOD` of clean `recv`s resets it.

use crate::metrics;
use futures::Stream;
use futures::StreamExt;
use nephila_eventsourcing::envelope::EventEnvelope;
use nephila_eventsourcing::store::{DomainEventStore, EventStoreError};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::time::sleep;
use tracing::warn;

#[derive(Debug, Clone, Copy)]
pub struct RetryConfig {
    pub retry_window: Duration,
    pub quiet_period: Duration,
    pub cooldown: Duration,
    pub soft_limit: u32,
    pub hard_limit: u32,
}

impl RetryConfig {
    pub const DEFAULT: Self = Self {
        retry_window: Duration::from_secs(30),
        quiet_period: Duration::from_secs(10),
        cooldown: Duration::from_secs(30),
        soft_limit: 3,
        hard_limit: 6,
    };
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[derive(Debug)]
struct RetryState {
    cfg: RetryConfig,
    lagged_retries_in_window: u32,
    window_start: Instant,
    last_clean_recv: Option<Instant>,
}

impl RetryState {
    fn new(cfg: RetryConfig) -> Self {
        Self {
            cfg,
            lagged_retries_in_window: 0,
            window_start: Instant::now(),
            last_clean_recv: None,
        }
    }

    fn note_clean_recv(&mut self) {
        let now = Instant::now();
        if let Some(prev) = self.last_clean_recv
            && now.duration_since(prev) >= self.cfg.quiet_period
        {
            self.lagged_retries_in_window = 0;
            self.window_start = now;
        }
        self.last_clean_recv = Some(now);
    }

    fn note_lagged(&mut self) {
        let now = Instant::now();
        if now.duration_since(self.window_start) > self.cfg.retry_window {
            self.window_start = now;
        }
        self.lagged_retries_in_window += 1;
    }

    fn over_hard_limit(&self) -> bool {
        self.lagged_retries_in_window >= self.cfg.hard_limit
    }

    fn over_soft_limit(&self) -> bool {
        self.lagged_retries_in_window >= self.cfg.soft_limit
    }
}

/// Streams events through `subscribe_after`, transparently re-subscribing
/// from the last seen sequence on `Lagged`. Surfaces `PersistentLag` once
/// the retry budget is exhausted.
pub fn resilient_subscribe<S>(
    store: Arc<S>,
    agg_type: String,
    agg_id: String,
    since: u64,
) -> impl Stream<Item = Result<EventEnvelope, EventStoreError>> + Send + 'static
where
    S: DomainEventStore + Send + Sync + 'static,
{
    resilient_subscribe_with_config(store, agg_type, agg_id, since, RetryConfig::DEFAULT)
}

/// Test-friendly variant exposing the retry config knobs.
pub fn resilient_subscribe_with_config<S>(
    store: Arc<S>,
    agg_type: String,
    agg_id: String,
    since: u64,
    cfg: RetryConfig,
) -> impl Stream<Item = Result<EventEnvelope, EventStoreError>> + Send + 'static
where
    S: DomainEventStore + Send + Sync + 'static,
{
    let (tx, rx) = mpsc::channel::<Result<EventEnvelope, EventStoreError>>(64);
    tokio::spawn(async move {
        let mut last_seq = since;
        let mut retry = RetryState::new(cfg);
        'outer: loop {
            let mut stream = match store.subscribe_after(&agg_type, &agg_id, last_seq).await {
                Ok(s) => s,
                Err(e) => {
                    let _ = tx.send(Err(e)).await;
                    return;
                }
            };
            while let Some(item) = stream.next().await {
                match item {
                    Ok(env) => {
                        last_seq = env.sequence;
                        retry.note_clean_recv();
                        if tx.send(Ok(env)).await.is_err() {
                            return;
                        }
                    }
                    Err(EventStoreError::Lagged(n)) => {
                        retry.note_lagged();
                        metrics::record_lagged_recovery(&agg_type, &agg_id, n);
                        if retry.over_hard_limit() {
                            warn!(
                                target: "nephila_store::resilient_subscribe",
                                aggregate_type = %agg_type,
                                aggregate_id = %agg_id,
                                retries = retry.lagged_retries_in_window,
                                "persistent lag — escalating",
                            );
                            let _ = tx
                                .send(Err(EventStoreError::PersistentLag(
                                    retry.lagged_retries_in_window,
                                )))
                                .await;
                            return;
                        }
                        if retry.over_soft_limit() {
                            sleep(cfg.cooldown).await;
                        }
                        continue 'outer;
                    }
                    Err(other) => {
                        let _ = tx.send(Err(other)).await;
                        return;
                    }
                }
            }
            // Underlying stream ended cleanly. Stop.
            return;
        }
    });
    let mut rx = rx;
    futures::stream::poll_fn(move |cx| rx.poll_recv(cx))
}

#[cfg(test)]
mod retry_state_tests {
    use super::*;

    #[test]
    fn lagged_count_does_not_reset_when_window_rolls_without_quiet_period() {
        let mut state = RetryState::new(RetryConfig {
            retry_window: Duration::from_millis(50),
            quiet_period: Duration::from_secs(60),
            cooldown: Duration::from_millis(0),
            soft_limit: 100,
            hard_limit: 1000,
        });
        for _ in 0..5 {
            state.note_lagged();
        }
        assert_eq!(state.lagged_retries_in_window, 5);

        std::thread::sleep(Duration::from_millis(60));
        state.note_lagged();
        assert_eq!(
            state.lagged_retries_in_window, 6,
            "counter must NOT reset when window rolls without QUIET_PERIOD",
        );
    }

    #[test]
    fn quiet_period_clean_recvs_reset_counter() {
        let mut state = RetryState::new(RetryConfig {
            retry_window: Duration::from_secs(60),
            quiet_period: Duration::from_millis(10),
            cooldown: Duration::from_millis(0),
            soft_limit: 100,
            hard_limit: 1000,
        });
        for _ in 0..5 {
            state.note_lagged();
        }
        state.note_clean_recv();
        assert_eq!(state.lagged_retries_in_window, 5);

        std::thread::sleep(Duration::from_millis(20));
        state.note_clean_recv();
        assert_eq!(state.lagged_retries_in_window, 0);
    }

    #[test]
    fn soft_and_hard_limits_trigger_at_thresholds() {
        let mut state = RetryState::new(RetryConfig {
            retry_window: Duration::from_secs(60),
            quiet_period: Duration::from_secs(60),
            cooldown: Duration::from_millis(0),
            soft_limit: 3,
            hard_limit: 6,
        });
        for _ in 0..2 {
            state.note_lagged();
        }
        assert!(!state.over_soft_limit());
        state.note_lagged();
        assert!(state.over_soft_limit());
        for _ in 0..3 {
            state.note_lagged();
        }
        assert!(state.over_hard_limit());
    }
}

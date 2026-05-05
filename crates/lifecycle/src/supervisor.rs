use std::collections::VecDeque;
use std::time::{Duration, Instant};

use nephila_core::config::SupervisionConfig;

/// Counts **crashes** (not turn exits) within a sliding window.
///
/// With the streaming `ClaudeCodeSession`, the only signal that increments
/// this tracker is `SessionEvent::SessionCrashed` — turn-level events
/// (`TurnAborted`, `TurnCompleted`) are no-ops because the process keeps
/// running across turns. Defaults assume each crash takes ~30s of recovery
/// time (kill old child, acquire lockfile, respawn, replay).
///
/// Call `reset()` after a clean `SessionEnded` so a long-running session
/// that retired without crashes does not leak crash credits to the next
/// session.
#[derive(Debug)]
pub struct RestartTracker {
    window: Duration,
    max_restarts: usize,
    timestamps: VecDeque<Instant>,
}

impl RestartTracker {
    #[must_use]
    pub fn new(config: SupervisionConfig) -> Self {
        Self {
            window: Duration::from_secs(config.restart_window_secs),
            max_restarts: config.max_restarts as usize,
            timestamps: VecDeque::new(),
        }
    }

    fn prune_window(&mut self, now: Instant) {
        while let Some(&front) = self.timestamps.front() {
            if now.duration_since(front) > self.window {
                self.timestamps.pop_front();
            } else {
                break;
            }
        }
    }

    /// Record a crash. Returns `true` if the restart is allowed (within the
    /// configured limit), `false` if the limit has been exceeded.
    pub fn record_restart(&mut self) -> bool {
        let now = Instant::now();
        self.prune_window(now);

        if self.timestamps.len() >= self.max_restarts {
            return false;
        }

        self.timestamps.push_back(now);
        true
    }

    /// Drop all tracked crash timestamps.
    ///
    /// Called on a clean `SessionEnded` so the next session starts with a
    /// full credit budget.
    pub fn reset(&mut self) {
        self.timestamps.clear();
    }

    #[must_use]
    pub fn restart_count(&self) -> usize {
        self.timestamps.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn within_limit() {
        let config = SupervisionConfig {
            default_strategy: "one_for_one".into(),
            max_restarts: 3,
            restart_window_secs: 60,
            max_agent_depth: 3,
        };
        let mut tracker = RestartTracker::new(config);

        assert!(tracker.record_restart());
        assert_eq!(tracker.restart_count(), 1);

        assert!(tracker.record_restart());
        assert_eq!(tracker.restart_count(), 2);

        assert!(tracker.record_restart());
        assert_eq!(tracker.restart_count(), 3);

        assert!(!tracker.record_restart());
        assert_eq!(tracker.restart_count(), 3);
    }

    #[test]
    fn reset_clears_credit_pool() {
        let config = SupervisionConfig {
            default_strategy: "one_for_one".into(),
            max_restarts: 2,
            restart_window_secs: 600,
            max_agent_depth: 3,
        };
        let mut tracker = RestartTracker::new(config);
        assert!(tracker.record_restart());
        assert!(tracker.record_restart());
        assert!(!tracker.record_restart(), "third must hit limit pre-reset");

        tracker.reset();
        assert_eq!(tracker.restart_count(), 0);
        assert!(
            tracker.record_restart(),
            "post-reset, restarts must be allowed again"
        );
    }
}

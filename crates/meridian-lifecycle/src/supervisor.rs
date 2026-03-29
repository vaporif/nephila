use std::collections::VecDeque;
use std::time::{Duration, Instant};

use meridian_core::config::SupervisionConfig;

pub struct RestartTracker {
    config: SupervisionConfig,
    timestamps: VecDeque<Instant>,
}

impl RestartTracker {
    pub fn new(config: SupervisionConfig) -> Self {
        Self {
            config,
            timestamps: VecDeque::new(),
        }
    }

    /// Record a restart attempt. Returns `true` if the restart is allowed
    /// (within the configured limit), `false` if the limit has been exceeded.
    pub fn record_restart(&mut self) -> bool {
        let now = Instant::now();
        let window = Duration::from_secs(self.config.restart_window_secs);

        while let Some(&front) = self.timestamps.front() {
            if now.duration_since(front) > window {
                self.timestamps.pop_front();
            } else {
                break;
            }
        }

        if self.timestamps.len() >= self.config.max_restarts as usize {
            return false;
        }

        self.timestamps.push_back(now);
        true
    }

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
        };
        let mut tracker = RestartTracker::new(config);

        assert!(tracker.record_restart());
        assert_eq!(tracker.restart_count(), 1);

        assert!(tracker.record_restart());
        assert_eq!(tracker.restart_count(), 2);

        assert!(tracker.record_restart());
        assert_eq!(tracker.restart_count(), 3);

        // exceeds limit
        assert!(!tracker.record_restart());
        assert_eq!(tracker.restart_count(), 3);
    }
}

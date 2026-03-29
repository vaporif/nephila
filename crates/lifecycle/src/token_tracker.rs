use meridian_core::config::LifecycleConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenBand {
    Normal,
    Warn,
    Critical,
}

pub struct TokenTracker {
    config: LifecycleConfig,
    used: u64,
    remaining: u64,
    drain_started: bool,
}

impl TokenTracker {
    pub fn new(config: LifecycleConfig) -> Self {
        Self {
            config,
            used: 0,
            remaining: config.context_window_size,
            drain_started: false,
        }
    }

    pub fn report(&mut self, used: u64, remaining: u64) {
        self.used = used;
        self.remaining = remaining;
    }

    pub fn usage_pct(&self) -> u8 {
        let total = self.used + self.remaining;
        if total == 0 {
            return 0;
        }
        ((self.used as u128 * 100) / total as u128) as u8
    }

    pub fn band(&self) -> TokenBand {
        let pct = self.usage_pct();
        if pct >= self.config.token_critical_pct {
            TokenBand::Critical
        } else if pct >= self.config.token_warn_pct {
            TokenBand::Warn
        } else {
            TokenBand::Normal
        }
    }

    pub fn should_prepare_reset(&self) -> bool {
        self.usage_pct() >= self.config.context_threshold_pct
    }

    pub fn should_force_kill(&self) -> bool {
        if self.drain_started {
            return false;
        }
        self.usage_pct() >= self.config.token_force_kill_pct
    }

    pub fn mark_drain_started(&mut self) {
        self.drain_started = true;
    }

    pub fn report_interval(&self) -> u32 {
        match self.band() {
            TokenBand::Normal => self.config.token_report_interval_normal,
            TokenBand::Warn => self.config.token_report_interval_warn,
            TokenBand::Critical => self.config.token_report_interval_critical,
        }
    }

    pub fn used(&self) -> u64 {
        self.used
    }

    pub fn remaining(&self) -> u64 {
        self.remaining
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> LifecycleConfig {
        LifecycleConfig {
            context_threshold_pct: 80,
            context_window_size: 200_000,
            token_warn_pct: 60,
            token_critical_pct: 75,
            token_force_kill_pct: 85,
            token_report_interval_normal: 10,
            token_report_interval_warn: 3,
            token_report_interval_critical: 1,
            hang_timeout_secs: 300,
            drain_timeout_secs: 60,
        }
    }

    #[test]
    fn band_transitions() {
        let mut tracker = TokenTracker::new(test_config());

        // 50% usage -> Normal
        tracker.report(500, 500);
        assert_eq!(tracker.band(), TokenBand::Normal);
        assert_eq!(tracker.usage_pct(), 50);

        // 60% usage -> Warn
        tracker.report(600, 400);
        assert_eq!(tracker.band(), TokenBand::Warn);
        assert_eq!(tracker.usage_pct(), 60);

        // 75% usage -> Critical
        tracker.report(750, 250);
        assert_eq!(tracker.band(), TokenBand::Critical);
        assert_eq!(tracker.usage_pct(), 75);

        // 90% usage -> still Critical
        tracker.report(900, 100);
        assert_eq!(tracker.band(), TokenBand::Critical);
        assert_eq!(tracker.usage_pct(), 90);
    }

    #[test]
    fn threshold_detection() {
        let mut tracker = TokenTracker::new(test_config());

        tracker.report(70_000, 30_000);
        assert_eq!(tracker.usage_pct(), 70);
        assert!(!tracker.should_prepare_reset());

        tracker.report(80_000, 20_000);
        assert_eq!(tracker.usage_pct(), 80);
        assert!(tracker.should_prepare_reset());

        tracker.report(90_000, 10_000);
        assert!(tracker.should_prepare_reset());
    }

    #[test]
    fn force_kill() {
        let mut tracker = TokenTracker::new(test_config());

        tracker.report(80_000, 20_000);
        assert!(!tracker.should_force_kill());

        tracker.report(85_000, 15_000);
        assert!(tracker.should_force_kill());

        // drain suppresses force kill
        tracker.mark_drain_started();
        assert!(!tracker.should_force_kill());

        tracker.report(95_000, 5_000);
        assert!(!tracker.should_force_kill());
    }

    #[test]
    fn report_interval() {
        let mut tracker = TokenTracker::new(test_config());

        tracker.report(400, 600);
        assert_eq!(tracker.report_interval(), 10); // Normal

        tracker.report(650, 350);
        assert_eq!(tracker.report_interval(), 3); // Warn

        tracker.report(800, 200);
        assert_eq!(tracker.report_interval(), 1); // Critical
    }
}

use std::time::{Duration, Instant};

/// Bounded exponential delay for restoring an explicitly requested connection.
/// Disconnect and Exit clear the pending retry; successful connection resets it.
#[derive(Default)]
pub(crate) struct ConnectionRetry {
    failures: u32,
    retry_at: Option<Instant>,
}

impl ConnectionRetry {
    pub(crate) fn reset(&mut self) {
        self.failures = 0;
        self.retry_at = None;
    }

    /// Returns true when an Error state is due for another attempt. The initial
    /// delay is three seconds; repeated failures grow exponentially to one minute.
    pub(crate) fn due(&mut self, now: Instant, wanted: bool, in_error: bool) -> bool {
        if !wanted || !in_error || self.failures >= 5 {
            if !wanted {
                self.reset();
            }
            return false;
        }
        match self.retry_at {
            None => {
                self.retry_at = Some(now + Self::delay(self.failures));
                false
            }
            Some(at) if now >= at => {
                self.retry_at = None;
                true
            }
            Some(_) => false,
        }
    }

    pub(crate) fn failed(&mut self, now: Instant) {
        self.failures = self.failures.saturating_add(1);
        self.retry_at = Some(now + Self::delay(self.failures));
    }

    fn delay(failures: u32) -> Duration {
        Duration::from_secs(3u64.saturating_mul(1u64 << failures.min(5)).min(60))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_error_with_bounded_backoff_and_stop_when_user_disconnects() {
        let start = Instant::now();
        let mut retry = ConnectionRetry::default();
        assert!(!retry.due(start, true, true));
        assert!(!retry.due(start + Duration::from_secs(2), true, true));
        assert!(retry.due(start + Duration::from_secs(3), true, true));
        retry.failed(start + Duration::from_secs(3));
        assert!(!retry.due(start + Duration::from_secs(8), true, true));
        assert!(retry.due(start + Duration::from_secs(9), true, true));
        retry.failed(start + Duration::from_secs(9));
        assert!(!retry.due(start + Duration::from_secs(20), true, true));
        assert!(retry.due(start + Duration::from_secs(21), true, true));

        retry.failed(start + Duration::from_secs(21));
        assert!(!retry.due(start + Duration::from_secs(22), false, true));
        assert!(!retry.due(start + Duration::from_secs(500), true, true));
        assert!(retry.due(start + Duration::from_secs(503), true, true));
    }

    #[test]
    fn retry_delay_caps_at_one_minute() {
        assert_eq!(ConnectionRetry::delay(0), Duration::from_secs(3));
        assert_eq!(ConnectionRetry::delay(1), Duration::from_secs(6));
        assert_eq!(ConnectionRetry::delay(4), Duration::from_secs(48));
        assert_eq!(ConnectionRetry::delay(5), Duration::from_secs(60));
        assert_eq!(ConnectionRetry::delay(30), Duration::from_secs(60));
    }
    #[test]
    fn repeated_failures_exhaust_budget_until_user_disconnects() {
        let now = Instant::now();
        let mut retry = ConnectionRetry::default();
        for _ in 0..5 { retry.failed(now); }
        assert!(!retry.due(now + Duration::from_secs(600), true, true));
        assert!(!retry.due(now, false, true));
        assert!(!retry.due(now, true, true));
        assert!(retry.due(now + Duration::from_secs(3), true, true));
    }
}

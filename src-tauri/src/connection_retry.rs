use std::time::{Duration, Instant};

/// Capped exponential delay for restoring an explicitly requested connection.
/// Disconnect and Exit clear the pending retry; successful connection resets it.
#[derive(Default)]
pub(crate) struct ConnectionRetry {
    failures: u32,
    retry_at: Option<Instant>,
}

impl ConnectionRetry {
    /// Only a confirmed network epoch grants a fresh budget; data refresh does not.
    pub(crate) fn network_changed(&mut self, wanted: bool) {
        if wanted {
            self.reset();
        }
    }
    pub(crate) fn reset(&mut self) {
        self.failures = 0;
        self.retry_at = None;
    }

    /// Returns true when an Error state is due for another attempt. The initial
    /// delay is three seconds; repeated failures grow exponentially to one minute.
    pub(crate) fn due(&mut self, now: Instant, wanted: bool, in_error: bool) -> bool {
        self.due_with_budget(now, wanted, in_error, true)
    }

    /// Connection intent outlives a retry burst. Internet can return without an
    /// adapter/route change, so keep one attempt per minute after five failures.
    /// Cleanup uses `due` instead and retains its finite retry budget.
    pub(crate) fn due_connection(&mut self, now: Instant, wanted: bool, in_error: bool) -> bool {
        self.due_with_budget(now, wanted, in_error, false)
    }

    fn due_with_budget(
        &mut self,
        now: Instant,
        wanted: bool,
        in_error: bool,
        bounded: bool,
    ) -> bool {
        if !wanted || !in_error || (bounded && self.failures >= 5) {
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
        for _ in 0..5 {
            retry.failed(now);
        }
        assert!(!retry.due(now + Duration::from_secs(600), true, true));
        assert!(!retry.due(now, false, true));
        assert!(!retry.due(now, true, true));
        assert!(retry.due(now + Duration::from_secs(3), true, true));
    }
    #[test]
    fn late_network_rearms_exhausted_budget_but_never_cancels_disconnect() {
        let now = Instant::now();
        for seconds in [30, 120, 600] {
            let mut retry = ConnectionRetry::default();
            for _ in 0..5 {
                retry.failed(now);
            }
            let returned = now + Duration::from_secs(seconds);
            assert!(!retry.due(returned, true, true));
            retry.network_changed(true);
            assert!(!retry.due(returned, true, true));
            assert!(retry.due(returned + Duration::from_secs(3), true, true));
            retry.network_changed(false);
            assert!(!retry.due(returned + Duration::from_secs(600), false, true));
        }
    }

    #[test]
    fn same_topology_outage_does_not_exhaust_connection_intent_or_create_a_retry_storm() {
        let start = Instant::now();
        for outage in [30, 120, 600, 3600] {
            let mut retry = ConnectionRetry::default();
            let mut attempts = Vec::new();
            for seconds in 0..=outage + 60 {
                let now = start + Duration::from_secs(seconds);
                if retry.due_connection(now, true, true) {
                    attempts.push(seconds);
                    if seconds < outage {
                        retry.failed(now);
                    } else {
                        retry.reset(); // Successful connection resets the owner.
                        assert!(!retry.due_connection(now + Duration::from_secs(600), true, false));
                        break;
                    }
                }
            }
            assert!(attempts
                .last()
                .is_some_and(|at| *at >= outage && *at <= outage + 60));
            assert_eq!(attempts[0], 3);
            for pair in attempts.windows(2) {
                assert!((6..=60).contains(&(pair[1] - pair[0])));
                if pair[0] >= 93 {
                    assert_eq!(pair[1] - pair[0], 60);
                }
            }
        }
    }

    #[test]
    fn durable_retries_stop_on_disconnect_and_restart_with_the_initial_delay() {
        let now = Instant::now();
        let mut retry = ConnectionRetry::default();
        for _ in 0..20 {
            retry.failed(now);
        }
        assert!(!retry.due_connection(now + Duration::from_secs(600), false, true));
        assert!(!retry.due_connection(now + Duration::from_secs(600), true, true));
        assert!(!retry.due_connection(now + Duration::from_secs(602), true, true));
        assert!(retry.due_connection(now + Duration::from_secs(603), true, true));
    }
}

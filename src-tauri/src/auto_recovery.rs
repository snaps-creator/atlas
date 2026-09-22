//! React to new outbound failures without waiting for scheduled URL tests.
#[derive(Default)]
pub struct Cooldown { last: Option<std::time::Instant> }
impl Cooldown {
    pub fn allow(&mut self, now: std::time::Instant) -> bool {
        if self.last.is_some_and(|last| now.duration_since(last) < std::time::Duration::from_secs(30)) {
            return false;
        }
        self.last = Some(now);
        true
    }
}
#[derive(Default)]
pub struct Trigger { last: Option<String> }
impl Trigger {
    pub fn observe(&mut self, selected: &str, lines: &[String]) -> bool {
        let latest = lines.iter().rev().find(|line| {
            line.contains("dial ATLAS") && line.contains("connect error:") &&
                (line.contains("context deadline exceeded") || line.contains("i/o timeout") || line.contains("connection refused"))
        }).cloned();
        let changed = latest.is_some() && latest != self.last;
        self.last = latest;
        changed && matches!(selected, "AUTO" | "FAILOVER")
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn first_failure_is_immediate_but_failure_storm_is_bounded() {
        let mut gate = Cooldown::default();
        let start = std::time::Instant::now();
        assert!(gate.allow(start));
        for seconds in 1..30 { assert!(!gate.allow(start + std::time::Duration::from_secs(seconds))); }
        assert!(gate.allow(start + std::time::Duration::from_secs(30)));
    }
    #[test]
    fn first_failure_triggers_once_manual_choice_is_preserved() {
        let mut t = Trigger::default();
        let a = vec!["time=1 [TCP] dial ATLAS error: host connect error: i/o timeout".into()];
        assert!(t.observe("AUTO", &a));
        assert!(!t.observe("AUTO", &a));
        let b = vec!["time=2 [TCP] dial ATLAS error: host connect error: context deadline exceeded".into()];
        assert!(!t.observe("manual-node", &b));
        assert!(!t.observe("AUTO", &b));
        let c = vec!["time=3 [TCP] dial ATLAS error: host connect error: connection refused".into()];
        assert!(t.observe("FAILOVER", &c));
    }
}

//! Small, session-local history. No timers, I/O, persisted settings or packet hooks.
use std::{collections::VecDeque, time::{Duration, Instant}};

const FRESH: Duration = Duration::from_secs(90);
const LIMIT: usize = 12;
#[derive(Clone, Default)]
pub(crate) struct History(VecDeque<Sample>);
#[derive(Clone)]
struct Sample { at: Instant, control: usize, delay: Option<u64>, alive: bool }
impl History {
    pub fn record(&mut self, now: Instant, control: usize, delay: Option<u64>, alive: bool) {
        self.0.retain(|s|now.saturating_duration_since(s.at)<FRESH);
        if self.0.len()==LIMIT { self.0.pop_front(); }
        self.0.push_back(Sample{at:now,control,delay,alive});
    }
    /// A fast independent control cannot erase a repeated slow direction.
    /// One isolated slow sample cannot trigger an automatic switch.
    pub fn slow_streak(&self, now: Instant, threshold: u64) -> u8 {
        (0..2).map(|control|self.0.iter().rev()
            .filter(|s|s.control==control && now.saturating_duration_since(s.at)<FRESH)
            .take_while(|s|s.alive && s.delay.is_some_and(|d|d>threshold)).count() as u8).max().unwrap_or(0)
    }
    pub fn summary(&self, now: Instant) -> serde_json::Value {
        let fresh: Vec<_>=self.0.iter().filter(|s|now.saturating_duration_since(s.at)<FRESH).collect();
        let mut delays:Vec<_>=fresh.iter().filter_map(|s|s.delay).collect();delays.sort_unstable();
        serde_json::json!({"samples":fresh.len(),"failures":fresh.iter().filter(|s|!s.alive).count(),
            "medianMs":delays.get(delays.len()/2),"spreadMs":delays.last().zip(delays.first()).map(|(max,min)|max-min),
            "windowSeconds":FRESH.as_secs(),"measurement":"http_via_node_ms"})
    }
    /// Repeated degradation is a ranking hint, never proof of a dead server.
    pub fn penalty(&self, now: Instant, threshold: u64) -> u8 {
        let failures=self.0.iter().filter(|s|now.saturating_duration_since(s.at)<FRESH && !s.alive).count();
        if failures>=2 {2} else if self.slow_streak(now,threshold)>=3 {1} else {0}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn fast_control_does_not_hide_three_slow_samples_on_other_control() {
        let now=Instant::now();let mut h=History::default();
        for _ in 0..3 {h.record(now,0,Some(50),true);h.record(now,1,Some(600),true);}
        assert_eq!(h.slow_streak(now,150),3);assert_eq!(h.penalty(now,150),1);
        h.record(now,1,Some(80),true);assert_eq!(h.slow_streak(now,150),0);
    }
    #[test] fn isolated_spike_and_expired_history_do_not_penalize_a_path() {
        let now=Instant::now();let mut h=History::default();
        h.record(now,0,Some(900),true);assert_eq!(h.penalty(now,150),0);
        for _ in 0..3 {h.record(now,0,Some(700),true);}
        assert_eq!(h.slow_streak(now+FRESH,150),0);assert_eq!(h.penalty(now+FRESH,150),0);
        for _ in 0..100 {h.record(now,0,Some(50),true);}
        assert_eq!(h.0.len(),LIMIT);
    }
}

//! A single bounded probe may run without owning the service command loop.
//! Dropping/resetting the receiver discards stale results after reconfiguration.
use std::sync::mpsc::{self, Receiver, TryRecvError};

#[derive(Default)]
pub struct Probe {
    pending: Option<Receiver<bool>>,
}
impl Probe {
    pub fn start(&mut self, check: impl FnOnce() -> bool + Send + 'static) -> bool {
        if self.pending.is_some() {
            return false;
        }
        let (tx, rx) = mpsc::channel();
        if std::thread::Builder::new().name("atlas-core-health".into()).spawn(move || {
            let _ = tx.send(check());
        }).is_err() {
            return false;
        }
        self.pending = Some(rx);
        true
    }
    pub fn poll(&mut self) -> Option<bool> {
        let result = match self.pending.as_ref()?.try_recv() {
            Ok(value) => Some(value),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(false),
        };
        if result.is_some() { self.pending = None; }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stalled_probe_does_not_block_commands_or_spawn_more_probes() {
        let (release, wait) = mpsc::channel();
        let mut probe = Probe::default();
        assert!(probe.start(move || { wait.recv().unwrap(); true }));
        for _ in 0..1000 {
            assert_eq!(probe.poll(), None);
            assert!(!probe.start(|| panic!("duplicate probe")));
        }
        // A session can end without waiting for a stalled API request.
        drop(probe);
        release.send(()).unwrap();
    }
    #[test]
    fn completed_and_failed_probes_can_be_replaced() {
        let mut probe = Probe::default();
        for expected in [true, false, true] {
            assert!(probe.start(move || expected));
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                if let Some(result) = probe.poll() { assert_eq!(result, expected); break; }
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
    }
}

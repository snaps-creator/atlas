use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
#[derive(Default)]
struct State {
    current: Option<Arc<AtomicBool>>,
    connecting: bool,
    stopped: bool,
    pending_settings: usize,
}
#[derive(Clone, Default)]
pub struct Cancellation(Arc<Mutex<State>>);
pub(crate) struct Connecting(Cancellation, Arc<AtomicBool>);
pub(crate) struct SettingsChange(Cancellation);
impl Cancellation {
    pub(crate) fn connecting(&self, preemptible: bool) -> (Arc<AtomicBool>, Connecting) {
        let mut state = self.0.lock().unwrap_or_else(|e|e.into_inner());
        let token = Arc::new(AtomicBool::new(!preemptible || state.pending_settings == 0));
        if let Some(old) = state.current.replace(token.clone()) { old.store(false, Ordering::SeqCst); }
        state.connecting = preemptible;
        state.stopped = false;
        (token.clone(), Connecting(self.clone(), token))
    }
    /// Preempt only an unfinished connection. The same token also belongs to
    /// the established session, which must never be cancelled by saving settings.
    pub(crate) fn settings_change(&self) -> SettingsChange {
        let mut state = self.0.lock().unwrap_or_else(|e|e.into_inner());
        state.pending_settings += 1;
        if state.connecting {
            if let Some(token) = &state.current { token.store(false, Ordering::SeqCst); }
        }
        SettingsChange(self.clone())
    }
    pub(crate) fn settings_pending(&self) -> bool {
        self.0.lock().unwrap_or_else(|e|e.into_inner()).pending_settings != 0
    }
    pub fn cancel(&self) {
        let mut state = self.0.lock().unwrap_or_else(|e|e.into_inner());
        state.stopped = true;
        if let Some(token) = state.current.as_ref() {
            token.store(false, Ordering::SeqCst);
        }
    }
}
impl Connecting {
    /// If Start completed concurrently with a settings request, retain the
    /// established session. An explicit Disconnect/Quit still wins the race.
    pub(crate) fn complete(self) {
        let mut state=self.0.0.lock().unwrap_or_else(|e|e.into_inner());
        if state.current.as_ref().is_some_and(|token|Arc::ptr_eq(token,&self.1)) {
            state.connecting=false;
            if !state.stopped { self.1.store(true,Ordering::SeqCst); }
        }
    }
}
impl Drop for Connecting {
    fn drop(&mut self) {
        let mut state=self.0.0.lock().unwrap_or_else(|e|e.into_inner());
        if state.current.as_ref().is_some_and(|token|Arc::ptr_eq(token,&self.1)) {
            state.connecting=false;
        }
    }
}
impl Drop for SettingsChange {
    fn drop(&mut self) {
        self.0.0.lock().unwrap_or_else(|e|e.into_inner()).pending_settings -= 1;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_new_attempt_cannot_revive_a_cancelled_attempt() {
        let cancellation = Cancellation::default();
        let (old, old_attempt) = cancellation.connecting(true);
        cancellation.cancel();
        let (new, _new_attempt) = cancellation.connecting(true);
        drop(old_attempt);
        assert!(!old.load(Ordering::SeqCst));
        assert!(new.load(Ordering::SeqCst));
        cancellation.cancel();
        assert!(!new.load(Ordering::SeqCst));
    }
    #[test]
    fn settings_preempt_connect_but_never_an_established_session() {
        let cancellation = Cancellation::default();
        let (attempt, connecting) = cancellation.connecting(true);
        let first = cancellation.settings_change();
        let second = cancellation.settings_change();
        assert!(!attempt.load(Ordering::SeqCst));
        drop(connecting);
        drop(first);
        assert!(cancellation.settings_pending());
        let (blocked, blocked_attempt) = cancellation.connecting(true);
        assert!(!blocked.load(Ordering::SeqCst));
        drop(blocked_attempt);
        drop(second);
        assert!(!cancellation.settings_pending());
        let (connected, finished) = cancellation.connecting(true);
        drop(finished);
        let _save = cancellation.settings_change();
        assert!(connected.load(Ordering::SeqCst));
        cancellation.cancel();
        assert!(!connected.load(Ordering::SeqCst));
    }
    #[test]
    fn completed_start_survives_concurrent_save_but_does_not_override_quit() {
        let cancellation=Cancellation::default();
        let (token, attempt)=cancellation.connecting(true);
        let save=cancellation.settings_change();
        attempt.complete();
        assert!(token.load(Ordering::SeqCst));
        drop(save);
        let (token, attempt)=cancellation.connecting(true);
        let _save=cancellation.settings_change();
        cancellation.cancel();
        attempt.complete();
        assert!(!token.load(Ordering::SeqCst));
    }
    #[test]
    fn settings_never_cancel_reattachment_to_an_existing_service() {
        let cancellation=Cancellation::default();
        let _save=cancellation.settings_change();
        let (token, _attempt)=cancellation.connecting(false);
        let _second_save=cancellation.settings_change();
        assert!(token.load(Ordering::SeqCst));
        cancellation.cancel();
        assert!(!token.load(Ordering::SeqCst));
    }
    #[test]
    fn settings_interrupt_a_waiting_attempt_without_the_application_mutex() {
        use std::{thread, time::{Duration, Instant}, sync::mpsc};
        let cancellation = Cancellation::default();
        let state = Arc::new(Mutex::new(()));
        let (ready, receive) = mpsc::channel();
        let worker_state = state.clone();
        let worker_cancel = cancellation.clone();
        let worker = thread::spawn(move || {
            let _lock = worker_state.lock().unwrap();
            let (token, _attempt) = worker_cancel.connecting(true);
            ready.send(()).unwrap();
            let deadline=Instant::now()+Duration::from_secs(5);
            while token.load(Ordering::SeqCst) && Instant::now()<deadline {
                thread::sleep(Duration::from_millis(10));
            }
            assert!(!token.load(Ordering::SeqCst));
        });
        receive.recv_timeout(Duration::from_secs(2)).unwrap();
        let started=Instant::now();
        let _save=cancellation.settings_change();
        let _lock=state.lock().unwrap();
        assert!(started.elapsed()<Duration::from_secs(1));
        worker.join().unwrap();
    }
}

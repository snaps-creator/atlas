use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
#[derive(Clone, Default)]
pub struct Cancellation(Arc<Mutex<Option<Arc<AtomicBool>>>>);
impl Cancellation {
    pub fn begin(&self) -> Arc<AtomicBool> {
        let token = Arc::new(AtomicBool::new(true));
        let mut current = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(old) = current.replace(token.clone()) { old.store(false, Ordering::SeqCst); }
        token
    }
    pub fn cancel(&self) {
        if let Some(token) = self.0.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            token.store(false, Ordering::SeqCst);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_new_attempt_cannot_revive_a_cancelled_attempt() {
        let cancellation = Cancellation::default();
        let old = cancellation.begin();
        cancellation.cancel();
        let new = cancellation.begin();
        assert!(!old.load(Ordering::SeqCst));
        assert!(new.load(Ordering::SeqCst));
        cancellation.cancel();
        assert!(!new.load(Ordering::SeqCst));
    }
}

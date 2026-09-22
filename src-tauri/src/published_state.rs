//! UI state is independent of the serialized network operation lock.
use serde_json::Value;
use std::sync::{Arc, RwLock};

#[derive(Clone, Default)]
pub struct PublishedState(Arc<RwLock<Value>>);
impl PublishedState {
    pub fn get(&self) -> Value {
        self.0.read().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub fn set(&self, state: Value) {
        *self.0.write().unwrap_or_else(|e| e.into_inner()) = state;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn state_remains_readable_while_network_worker_is_blocked() {
        let network = std::sync::Mutex::new(());
        let _busy = network.lock().unwrap();
        let state = PublishedState::default();
        state.set(serde_json::json!({"status":"Connecting"}));
        let reader = state.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || tx.send(reader.get()).unwrap());
        assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap()["status"], "Connecting");
    }
}

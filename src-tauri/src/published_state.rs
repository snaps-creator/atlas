//! UI state is independent of the serialized network operation lock.
use serde_json::Value;
use std::sync::{Arc, RwLock};

#[derive(Clone)]
pub(crate) struct ReadSnapshot {
    pub revision: u64,
    pub settings: crate::model::Settings,
    pub status: String,
    pub client: crate::core::ApiClient,
    pub binary: std::path::PathBuf,
    pub directory: std::path::PathBuf,
}
/// Readers never acquire the command mutex. Only an Arc swap holds this lock;
/// network calls, serialization and settings copies happen outside it.
#[derive(Clone, Default)]
pub(crate) struct ReadState(Arc<RwLock<Option<Arc<ReadSnapshot>>>>);
impl ReadState {
    pub fn set(&self, snapshot: ReadSnapshot) {
        let snapshot=Arc::new(snapshot);
        *self.0.write().unwrap_or_else(|e|e.into_inner())=Some(snapshot);
    }
    pub fn get(&self) -> Result<Arc<ReadSnapshot>,String> {
        self.0.read().unwrap_or_else(|e|e.into_inner()).clone().ok_or("Atlas запускается".into())
    }
}

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
    fn live_readers_are_independent_of_a_blocked_network_command() {
        let command=std::sync::Mutex::new(());
        let _command_in_progress=command.lock().unwrap();
        let state=ReadState::default();
        let core=crate::core::Core::new(Default::default(),Default::default());
        state.set(ReadSnapshot{revision:7,settings:Default::default(),status:"Connected".into(),
            client:core.client(),binary:Default::default(),directory:Default::default()});
        let (tx,rx)=std::sync::mpsc::channel();
        for _ in 0..32 {
            let state=state.clone(); let tx=tx.clone();
            std::thread::spawn(move || { let snapshot=state.get().unwrap();tx.send((snapshot.revision,snapshot.status.clone())).unwrap(); });
        }
        for _ in 0..32 { assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap(),(7,"Connected".into())); }
        let old=state.get().unwrap();
        state.set(ReadSnapshot{revision:8,status:"Disconnecting".into(),..(*old).clone()});
        assert_eq!(old.status,"Connected");
        assert_eq!(state.get().unwrap().revision,8);
        assert_eq!(state.get().unwrap().status,"Disconnecting");
    }
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

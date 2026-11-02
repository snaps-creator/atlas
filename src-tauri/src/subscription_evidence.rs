//! Durable, credential-free refresh outcomes, independent of the event ring.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::PathBuf, sync::{Mutex, OnceLock}};
#[derive(Default)]
struct Ledger { path: Option<PathBuf>, entries: BTreeMap<String,Value> }
static LEDGER: OnceLock<Mutex<Ledger>> = OnceLock::new();
fn ledger() -> &'static Mutex<Ledger> { LEDGER.get_or_init(Default::default) }
pub(crate) fn identity(id: &str) -> String { format!("{:x}",Sha256::digest(id.as_bytes()))[..16].to_owned() }
pub(crate) fn init(directory: &std::path::Path) {
    let mut state=ledger().lock().unwrap_or_else(|e|e.into_inner());
    let path=directory.join("subscription-evidence.json");
    if std::fs::metadata(&path).is_ok_and(|m|m.len()<1024*1024) {
        state.entries=std::fs::read(&path).ok().and_then(|s|serde_json::from_slice(&s).ok()).unwrap_or_default();
        for value in state.entries.values_mut() { if value["outcome"]=="running" {value["outcome"]=json!("interrupted");} }
    }
    state.path=Some(path);
}
fn write(id: &str, value: Value) {
    let mut state=ledger().lock().unwrap_or_else(|e|e.into_inner());
    state.entries.insert(identity(id),value);
    while state.entries.len()>256 {
        let oldest=state.entries.iter().min_by_key(|(_,v)|v["startedAt"].as_u64().unwrap_or(0)).map(|(k,_)|k.clone()).unwrap();
        state.entries.remove(&oldest);
    }
    if let Some(path)=&state.path {
        let result=serde_json::to_vec(&state.entries).map_err(|e|e.to_string())
            .and_then(|bytes|std::fs::write(path,bytes).map_err(|e|e.to_string()));
        if let Err(error)=result { crate::incident_history::record("subscription_evidence_write_failed",json!({"error":error}),&[]); }
    }
}
pub(crate) struct Attempt { id: String, trigger: String, started: u64, finished: bool }
impl Attempt {
    pub(crate) fn start(id: &str, trigger: &str) -> Self {
        let attempt=Self{id:id.into(),trigger:trigger.into(),started:crate::model::now(),finished:false};
        write(id,json!({"startedAt":attempt.started,"trigger":trigger,"outcome":"running"}));
        attempt
    }
    pub(crate) fn finish(&mut self, outcome: &str, error: Option<&str>) {
        let error=error.map(|e|crate::support_report::redact(e,&[]).chars().take(2048).collect::<String>());
        write(&self.id,json!({"startedAt":self.started,"completedAt":crate::model::now(),"trigger":self.trigger,"outcome":outcome,"error":error}));
        self.finished=true;
    }
}
impl Drop for Attempt { fn drop(&mut self) { if !self.finished {self.finish("interrupted_or_not_applied",None);} } }
pub(crate) fn summary(settings: &crate::model::Settings) -> Value {
    let state=ledger().lock().unwrap_or_else(|e|e.into_inner());
    json!({"capturedAt":crate::model::now(),"subscriptions":settings.subscriptions.iter().map(|sub| {
        let id=identity(&sub.id);
        json!({"diagnosticId":id,"name":sub.name,"source":sub.source,"lastAppliedAt":sub.updated_at,
            "nodeCount":sub.servers.len(),"lastError":sub.error,"lastAttempt":state.entries.get(&id),
            "attemptCoverage":if state.entries.contains_key(&id){"recorded"}else{"unknown_before_this_build"}})
    }).collect::<Vec<_>>(),"meaning":"lastAppliedAt is saved subscription data time; downloaded does not mean applied. Missing attempt history is unknown, not success or failure."})
}
pub(crate) fn refresh_attempts(summary: &mut Value) {
    let state=ledger().lock().unwrap_or_else(|e|e.into_inner());
    if let Some(subscriptions)=summary["subscriptions"].as_array_mut() {
        for subscription in subscriptions {
            let attempt=subscription["diagnosticId"].as_str().and_then(|id|state.entries.get(id)).cloned();
            if let Some(attempt)=attempt {subscription["lastAttempt"]=attempt;subscription["attemptCoverage"]=json!("recorded");}
        }
    }
    summary["attemptsObservedAt"]=json!(crate::model::now());
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn applied_data_and_download_attempt_remain_distinct_and_survive_reload() {
        let directory=std::env::temp_dir().join(format!("atlas-sub-evidence-{}",uuid::Uuid::new_v4()));std::fs::create_dir_all(&directory).unwrap();init(&directory);
        let mut settings=crate::model::Settings::default();
        settings.subscriptions.push(crate::model::Subscription{id:"private-id".into(),name:"Office".into(),source:Default::default(),options:Default::default(),masked_url:String::new(),updated_at:123,error:None,servers:vec![]});
        assert_eq!(summary(&settings)["subscriptions"][0]["attemptCoverage"],"unknown_before_this_build");
        let mut attempt=Attempt::start("private-id","recovery");attempt.finish("failed",Some("Get https://private.test/secret failed"));init(&directory);
        let report=summary(&settings);
        assert_eq!(report["subscriptions"][0]["lastAppliedAt"],123);assert_eq!(report["subscriptions"][0]["lastAttempt"]["outcome"],"failed");
        assert!(!report.to_string().contains("private.test"));assert!(!report.to_string().contains("private-id"));
        std::fs::remove_dir_all(directory).unwrap();ledger().lock().unwrap().path=None;
    }
}

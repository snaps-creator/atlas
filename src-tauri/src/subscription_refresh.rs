//! Independent subscription downloads. Never hold App's command lock during HTTPS.
use crate::{Shared, published_state::ReadState, subscriptions, incident_history, model};
use serde_json::json;
use std::{collections::{HashMap, HashSet}, sync::{Arc, Mutex, OnceLock, atomic::{AtomicBool, Ordering}}, time::Duration};

static BUSY: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
fn busy() -> &'static Mutex<HashSet<String>> { BUSY.get_or_init(Default::default) }
pub(crate) fn active() -> Vec<String> { busy().lock().unwrap_or_else(|e|e.into_inner()).iter().cloned().collect() }
pub(crate) struct Lease(String);
impl Lease {
    pub fn take(id: &str) -> Option<Self> {
        busy().lock().unwrap_or_else(|e|e.into_inner()).insert(id.into()).then(|| Self(id.into()))
    }
}
impl Drop for Lease { fn drop(&mut self) { busy().lock().unwrap_or_else(|e|e.into_inner()).remove(&self.0); } }

fn reason(startup: bool, updated: u64, last_attempt: Option<u64>, now: u64, failed: bool) -> Option<&'static str> {
    if startup && last_attempt.is_none() { return Some("startup"); }
    if last_attempt.is_some_and(|at|now.saturating_sub(at)<120) { return None; }
    if failed { return Some("recovery"); }
    (now.saturating_sub(updated)>=1800).then_some("periodic")
}
fn failed_nodes(lines: &[String], now: u64) -> HashSet<String> {
    let mut failures = HashMap::new();
    for line in lines {
        let Some(raw) = line.strip_prefix("ATLAS_EVENT ") else { continue; };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else { continue; };
        if now.saturating_sub(v["at"].as_u64().unwrap_or(0))>60 { continue; }
        if v["kind"]=="active_node_sample" {
            if let Some(name)=v["name"].as_str() { failures.insert(name.to_owned(), v["responded"]==false); }
        }
        if v["kind"]=="recovery_result" && v["evidence"]["candidate"].is_null() {
            for result in v["evidence"]["results"].as_array().into_iter().flatten() {
                if result["passed"]==false {
                    if let Some(name)=result["name"].as_str() { failures.insert(name.to_owned(),true); }
                }
            }
        }
    }
    failures.into_iter().filter_map(|(name,failed)|failed.then_some(name)).collect()
}
pub(crate) fn start(shared: Shared, reads: ReadState, stop: Arc<AtomicBool>, done: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let initial: HashSet<_> = reads.get().map(|v|v.settings.subscriptions.iter().map(|s|s.id.clone()).collect()).unwrap_or_default();
        let mut outstanding = initial.clone();
        let mut attempts = HashMap::new();
        let (tx,rx) = std::sync::mpsc::channel();
        let mut in_flight = HashSet::new();
        let mut last_logs = 0;
        let mut failed = HashSet::new();
        while !stop.load(Ordering::SeqCst) {
            for id in rx.try_iter() { in_flight.remove(&id); outstanding.remove(&id); }
            let Ok(snapshot)=reads.get() else { std::thread::sleep(Duration::from_millis(200)); continue; };
            outstanding.retain(|id|snapshot.settings.subscriptions.iter().any(|s| &s.id==id));
            if outstanding.is_empty() { done.store(true,Ordering::SeqCst); }
            let now=model::now();
            if now.saturating_sub(last_logs)>=10 {
                failed = if matches!(snapshot.status.as_str(),"Connected"|"ProtectedPause") {
                    failed_nodes(&snapshot.client.logs().unwrap_or_default(),now)
                } else { HashSet::new() };
                last_logs=now;
            }
            for sub in &snapshot.settings.subscriptions {
                if in_flight.len()>=2 { break; }
                let failed = sub.servers.iter().any(|n|n["name"].as_str().is_some_and(|n|failed.contains(n)))
                    || (snapshot.status=="Error" && shared.try_lock().is_ok_and(|a|a.reconnect.load(Ordering::SeqCst)));
                let Some(trigger)=reason(outstanding.contains(&sub.id),sub.updated_at,attempts.get(&sub.id).copied(),now,failed) else { continue; };
                let Some(lease)=Lease::take(&sub.id) else { continue; };
                let id=sub.id.clone(); attempts.insert(id.clone(),now); in_flight.insert(id.clone());
                let (shared,reads,stop,tx)=(shared.clone(),reads.clone(),stop.clone(),tx.clone());
                std::thread::spawn(move || {
                    let started=std::time::Instant::now();
                    incident_history::record("subscription_refresh_started",json!({"subscription":id,"trigger":trigger}),&[]);
                    let result=(|| -> Result<(),String> {
                        let before=reads.get()?;
                        let url=keyring::Entry::new("AtlasVPN",&id).map_err(|_|"Хранилище Windows недоступно")?
                            .get_password().map_err(|_|"Ссылка подписки отсутствует в хранилище Windows")?;
                        let nodes=subscriptions::download(&url,before.status=="Connected")?;
                        let mut a=shared.lock().map_err(|_|"Состояние Atlas недоступно")?;
                        if stop.load(Ordering::SeqCst) { return Err("Atlas завершает работу".into()); }
                        if !subscriptions::refresh_is_current(before.settings.subscriptions.iter().find(|s|s.id==id),a.settings.subscriptions.iter().find(|s|s.id==id)) {
                            return Ok(()); // Deleted or superseded; never recreate/overwrite it.
                        }
                        a.install_subscription(id.clone(),url,nodes,None,None)?;
                        if trigger=="recovery" && a.status=="Error" && a.reconnect.load(Ordering::SeqCst) { a.connect()?; }
                        a.snapshot(); Ok(())
                    })();
                    incident_history::record("subscription_refresh_completed",json!({"subscription":id,"trigger":trigger,
                        "elapsedMs":started.elapsed().as_millis(),"success":result.is_ok(),"error":result.as_ref().err()}),&[]);
                    drop(lease);
                    if !stop.load(Ordering::SeqCst) {
                        if let Ok(mut a)=shared.lock() {
                            if let Err(error)=result { if let Some(sub)=a.settings.subscriptions.iter_mut().find(|s|s.id==id) { sub.error=Some(error); } }
                            a.snapshot();
                        }
                    }
                    let _=tx.send(id);
                });
            }
            std::thread::sleep(Duration::from_secs(2));
        }
        done.store(true,Ordering::SeqCst);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_schedules_retry_without_reloading_healthy_subscriptions() {
        assert_eq!(reason(true,100,None,101,false),Some("startup"));
        assert_eq!(reason(false,100,Some(100),180,true),None);
        assert_eq!(reason(false,100,Some(100),220,true),Some("recovery"));
        assert_eq!(reason(false,100,Some(100),220,false),None);
        assert_eq!(reason(false,100,Some(100),1900,false),Some("periodic"));
        let a=Lease::take("test-A").unwrap();
        assert!(Lease::take("test-A").is_none());
        let b=Lease::take("test-B").unwrap(); drop(a);
        assert!(Lease::take("test-A").is_some()); drop(b);
    }
    #[test]
    fn recovered_or_old_samples_do_not_trigger_downloads() {
        let lines=vec![format!("ATLAS_EVENT {}",json!({"kind":"active_node_sample","at":100,"name":"A","responded":false})),
            format!("ATLAS_EVENT {}",json!({"kind":"active_node_sample","at":110,"name":"A","responded":true}))];
        assert!(failed_nodes(&lines,120).is_empty());
        assert!(failed_nodes(&lines[..1],170).is_empty());
        assert!(failed_nodes(&lines[..1],120).contains("A"));
    }
}

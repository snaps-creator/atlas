//! Independent subscription downloads. Never hold App's command lock during HTTPS.
use crate::{Shared, published_state::ReadState, subscriptions, incident_history, model};
use serde_json::json;
use std::{collections::{HashMap, HashSet}, sync::{Arc, Mutex, OnceLock, atomic::{AtomicBool, Ordering}}, time::Duration};

static BUSY: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
fn busy() -> &'static Mutex<HashSet<String>> { BUSY.get_or_init(Default::default) }
static WAKE: OnceLock<(Mutex<bool>,std::sync::Condvar)> = OnceLock::new();
pub(crate) fn wake() {
    let (flag,signal)=WAKE.get_or_init(Default::default);
    *flag.lock().unwrap_or_else(|e|e.into_inner())=true;signal.notify_one();
}
fn wait() {
    let (flag,signal)=WAKE.get_or_init(Default::default);
    let guard=flag.lock().unwrap_or_else(|e|e.into_inner());
    let (mut guard,_)=signal.wait_timeout_while(guard,Duration::from_millis(500),|wake| !*wake).unwrap_or_else(|e|e.into_inner());
    *guard=false;
}
pub(crate) fn active() -> Vec<String> { busy().lock().unwrap_or_else(|e|e.into_inner()).iter().cloned().collect() }
pub(crate) struct Lease(String);
impl Lease {
    pub fn take(id: &str) -> Option<Self> {
        busy().lock().unwrap_or_else(|e|e.into_inner()).insert(id.into()).then(|| Self(id.into()))
    }
}
impl Drop for Lease { fn drop(&mut self) { busy().lock().unwrap_or_else(|e|e.into_inner()).remove(&self.0); } }

fn reason(startup: bool, updated: u64, last_attempt: Option<u64>, now: u64, failed: bool, interval_seconds: u64) -> Option<&'static str> {
    if last_attempt.is_some_and(|at|at<=now && now-at<120) { return None; }
    if failed { return Some("recovery"); }
    (updated==0 || updated>now || now.saturating_sub(updated)>=interval_seconds).then_some(if startup {"startup"} else {"periodic"})
}
pub(crate) fn start(shared: Shared, reads: ReadState, stop: Arc<AtomicBool>, done: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let initial: HashSet<_> = reads.get().map(|v|v.settings.subscriptions.iter().filter(|s|s.source==model::SubscriptionSource::Url).map(|s|s.id.clone()).collect()).unwrap_or_default();
        let mut outstanding = initial.clone();
        let mut attempts = HashMap::new();
        let (tx,rx) = std::sync::mpsc::channel();
        let mut in_flight = HashSet::new();
        let recovery_busy=Arc::new(AtomicBool::new(false));
        let mut recovery_operation=None;
        let mut notification_operation=None;
        while !stop.load(Ordering::SeqCst) {
            if crate::update_health::pending() {std::thread::sleep(Duration::from_millis(200));continue;}
            for id in rx.try_iter() { in_flight.remove(&id); outstanding.remove(&id); }
            let Ok(snapshot)=reads.get() else { std::thread::sleep(Duration::from_millis(200)); continue; };
            outstanding.retain(|id|snapshot.settings.subscriptions.iter().any(|s| &s.id==id));
            if outstanding.is_empty() { done.store(true,Ordering::SeqCst); }
            let now=model::now();
            let mut refreshing=false;
            if matches!(snapshot.status.as_str(),"Connected"|"ProtectedPause") {
                if let Ok(state)=snapshot.client.recovery_command("recovery_status",serde_json::Value::Null) {
                    refreshing=state["phase"]=="REFRESHING_SUBSCRIPTIONS";
                    let id=state["operationId"].as_str().map(str::to_owned);
                    let ticket=state["refreshTicket"].as_str().map(str::to_owned);
                    if state["phase"]=="REFRESHING_SUBSCRIPTIONS" && ticket.is_some() && ticket!=recovery_operation
                        && in_flight.is_empty() && !recovery_busy.swap(true,Ordering::SeqCst) {
                        recovery_operation=ticket.clone();
                        let (shared,reads,stop,busy)=(shared.clone(),reads.clone(),stop.clone(),recovery_busy.clone());
                        let job_id=ticket.unwrap();
                        std::thread::spawn(move || {
                            refresh_recovery(&shared,&reads,&stop,&job_id);
                            busy.store(false,Ordering::SeqCst);
                        });
                    }
                    if state["failureExhausted"]==true && id.is_some() && notification_operation!=id {
                        notification_operation=id;
                        crate::recovery_notification::show(state["subscriptionsUpdated"]==true);
                    }
                }
            }
            if refreshing || recovery_busy.load(Ordering::SeqCst) { std::thread::sleep(Duration::from_millis(200)); continue; }
            for sub in &snapshot.settings.subscriptions {
                if in_flight.len()>=2 { break; }
                if sub.source!=model::SubscriptionSource::Url {continue;}
                // Recovery downloads are owned by the explicit service incident.
                let failed = sub.error.is_some();
                let Some(trigger)=reason(outstanding.contains(&sub.id),sub.updated_at,attempts.get(&sub.id).copied(),now,failed,sub.options.refresh_interval_seconds()) else { continue; };
                let Some(lease)=Lease::take(&sub.id) else { continue; };
                let id=sub.id.clone(); attempts.insert(id.clone(),now); in_flight.insert(id.clone());
                let (shared,reads,stop,tx)=(shared.clone(),reads.clone(),stop.clone(),tx.clone());
                std::thread::spawn(move || {
                    let started=std::time::Instant::now();
                    incident_history::record("subscription_refresh_started",json!({"subscription":id,"trigger":trigger}),&[]);
                    let captured=reads.get();
                    let result=std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<(),String> {
                        let before=captured.as_ref().map_err(Clone::clone)?;
                        let url=keyring::Entry::new("AtlasVPN",&id).map_err(|_|"Хранилище Windows недоступно")?
                            .get_password().map_err(|_|"Ссылка подписки отсутствует в хранилище Windows")?;
                        let options=before.settings.subscriptions.iter().find(|s|s.id==id)
                            .map(|s|s.options.clone()).unwrap_or_default();
                        let nodes=subscriptions::download(&url,before.status=="Connected",&options)?;
                        let mut a=shared.lock().map_err(|_|"Состояние Atlas недоступно")?;
                        if stop.load(Ordering::SeqCst) { return Err("Atlas завершает работу".into()); }
                        if !subscriptions::refresh_is_current(before.settings.subscriptions.iter().find(|s|s.id==id),a.settings.subscriptions.iter().find(|s|s.id==id)) {
                            return Ok(()); // Deleted or superseded; never recreate/overwrite it.
                        }
                        let source=before.settings.subscriptions.iter().find(|s|s.id==id).ok_or("Подписка удалена")?.source;
                        a.install_subscription(id.clone(),url,nodes,None,None,source)?;
                        a.snapshot(); Ok(())
                    })).unwrap_or_else(|_|Err("Обновление подписки прервано; будет повторено".into()));
                    incident_history::record("subscription_refresh_completed",json!({"subscription":id,"trigger":trigger,
                        "elapsedMs":started.elapsed().as_millis(),"success":result.is_ok(),"error":result.as_ref().err()}),&[]);
                    if !stop.load(Ordering::SeqCst) {
                        if let Ok(mut a)=shared.lock() {
                            if let Err(error)=result {
                                let current=captured.as_ref().is_ok_and(|before|subscriptions::refresh_is_current(
                                    before.settings.subscriptions.iter().find(|s|s.id==id),a.settings.subscriptions.iter().find(|s|s.id==id)));
                                if let Some(sub)=a.settings.subscriptions.iter_mut().find(|s|s.id==id).filter(|_|current) {
                                    sub.error=Some(error);
                                    let settings=a.settings.clone();
                                    if a.store.save_source_status(&settings).is_err() {a.log("WARN","Не удалось сохранить состояние обновления подписки");}
                                }
                            }
                            a.snapshot();
                        }
                    }
                    drop(lease);
                    let _=tx.send(id);
                });
            }
            wait();
        }
        done.store(true,Ordering::SeqCst);
    });
}

fn refresh_recovery(shared: &Shared, reads: &ReadState, stop: &AtomicBool, operation: &str) {
    let Ok(before)=reads.get() else { return; };
    let connected=before.status=="Connected";
    let mut next=before.settings.clone();
    let sources:Vec<_>=before.settings.subscriptions.iter().filter(|sub|sub.source==model::SubscriptionSource::Url).collect();
    let mut all_ok=!sources.is_empty();
    let mut changed=false;
    let deadline=std::time::Instant::now()+Duration::from_secs(120);
    // Two downloads at a time, a bounded queue, and one atomic settings commit.
    for pair in sources.chunks(2) {
        if stop.load(Ordering::SeqCst) { return; }
        if std::time::Instant::now()>=deadline {all_ok=false;break;}
        let results=std::thread::scope(|scope| {
            let jobs:Vec<_>=pair.iter().map(|sub|scope.spawn(move || {
                let result=(|| {
                    let _lease=Lease::take(&sub.id).ok_or("Подписка уже обновляется")?;
                    let url=keyring::Entry::new("AtlasVPN",&sub.id).map_err(|_|"Хранилище Windows недоступно")?
                        .get_password().map_err(|_|"Нет URL-подписки")?;
                    subscriptions::download(&url,connected,&sub.options)
                })();
                (sub.id.clone(),result)
            })).collect();
            jobs.into_iter().filter_map(|job|job.join().ok()).collect::<Vec<_>>()
        });
        if results.len()!=pair.len() { all_ok=false; }
        for (id,result) in results {
            let sub=next.subscriptions.iter_mut().find(|s|s.id==id).unwrap();
            match result {
                Ok(downloaded) => {
                    if !downloaded.diagnostics.is_empty() {all_ok=false;sub.error=Some("Неполный ответ подписки; предыдущая версия сохранена".into());continue;}
                    if downloaded.not_modified {
                        sub.options=downloaded.options;sub.updated_at=model::now();sub.error=None;changed=true;continue;
                    }
                    let nodes:Vec<_>=downloaded.nodes.into_iter().filter(|node|sub.source.accepts(node)).collect();
                    if nodes.is_empty() && !downloaded.explicit_empty {all_ok=false;sub.error=Some("Источник не содержит серверов выбранного типа".into());continue;}
                    sub.servers=stable_names(&sub.servers,nodes,&id);
                    sub.options=downloaded.options; sub.updated_at=model::now(); sub.error=None; changed=true;
                }
                Err(_) => { all_ok=false; sub.error=Some("Автоматическое обновление подписки не выполнено".into()); }
            }
        }
    }
    if stop.load(Ordering::SeqCst) { return; }
    // Serialize with manual settings/subscription changes; never overwrite a new revision.
    if let Ok(mut app)=shared.lock() {
        if stop.load(Ordering::SeqCst) {return;}
        let current=before.client.recovery_command("recovery_status",serde_json::Value::Null).unwrap_or_default();
        if app.revision!=before.revision || current["refreshTicket"]!=operation || current["phase"]!="REFRESHING_SUBSCRIPTIONS" {
            all_ok=false;
        } else if changed {
            model::repository::reconcile_references(&app.settings,&mut next);
            if !["AUTO","FAILOVER"].contains(&next.selected.as_str()) && !next.servers().iter().any(|n|n["name"]==next.selected) {
                next.selected="AUTO".into();
            }
            if app.core.validate(&next).and_then(|_|app.save(next)).is_err() { all_ok=false; }
            app.snapshot();
        }
    } else { all_ok=false; }
    let _=before.client.recovery_command("recovery_refresh_result",json!({"ticket":operation,"success":all_ok}));
}

/// Preserve selections and favourites when a provider reorders identical nodes.
pub(crate) fn stable_names(old: &[serde_json::Value], nodes: Vec<serde_json::Value>, id: &str) -> Vec<serde_json::Value> {
    subscriptions::reconcile_nodes(id,nodes,old)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_schedules_retry_without_reloading_healthy_subscriptions() {
        assert_eq!(reason(true,100,None,101,false,1800),None);
        assert_eq!(reason(true,0,None,101,false,1800),Some("startup"));
        assert_eq!(reason(false,200,Some(200),100,false,1800),Some("periodic"));
        assert_eq!(reason(false,100,Some(100),180,true,1800),None);
        assert_eq!(reason(false,100,Some(100),220,true,1800),Some("recovery"));
        assert_eq!(reason(false,100,Some(100),220,false,1800),None);
        assert_eq!(reason(false,100,Some(100),1900,false,1800),Some("periodic"));
        assert_eq!(reason(false,100,Some(100),1900,false,7200),None);
        assert_eq!(reason(false,100,Some(100),7300,false,7200),Some("periodic"));
        assert_eq!(reason(false,100,Some(100),220,true,7200),Some("recovery"));
        let a=Lease::take("test-A").unwrap();
        assert!(Lease::take("test-A").is_none());
        let b=Lease::take("test-B").unwrap(); drop(a);
        assert!(Lease::take("test-A").is_some()); drop(b);
    }
    #[test]
    fn provider_reordering_preserves_names_and_distinct_credentials() {
        let old=vec![json!({"name":"favourite","server":"a","password":"one"}),json!({"name":"second","server":"a","password":"two"})];
        let new=vec![json!({"name":"renamed","server":"a","password":"two"}),json!({"name":"renamed","server":"a","password":"one"})];
        let nodes=stable_names(&old,new,"short");
        assert_eq!(nodes[0]["name"],"second"); assert_eq!(nodes[1]["name"],"favourite");
    }
}

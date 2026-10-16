//! Application-owned display measurements. One bounded batch, shared by timer,
//! navigation and manual requests. No window/React lifetime owns network work.
use crate::{latency, published_state::{ReadSnapshot, ReadState}};
use serde_json::{json, Value};
use std::{collections::BTreeMap, sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}}, time::{Duration, Instant}};
use tauri::Emitter;

pub const FRESH_FOR: Duration = Duration::from_millis(crate::probe_state::FRESH_MS);
const REFRESH_AFTER: Duration = Duration::from_secs(60);
const RETRY_AFTER: Duration = REFRESH_AFTER;

#[derive(Clone)]
struct Entry {
    identity: String,
    attempt: u64,
    pending: bool,
    due: Instant,
    completed: Option<Instant>,
    result: Value,
}
#[derive(Default)]
struct Catalog {
    entries: BTreeMap<String, Entry>,
    context: String,
    generation: u64,
    sequence: u64,
    batch_running: bool,
    revision: Option<u64>,
}
#[derive(Clone)]
struct Ticket { name: String, identity: String, generation: u64, attempt: u64 }

impl Catalog {
    fn invalidate(&mut self, now: Instant) {
        self.generation += 1;
        for entry in self.entries.values_mut() {
            entry.result = Value::Null;
            entry.completed = None;
            entry.pending = false;
            entry.due = now;
        }
    }
    fn reconcile(&mut self, identities: BTreeMap<String, String>, context: String, now: Instant) {
        if self.context != context {
            self.context = context;
            self.invalidate(now);
        }
        self.entries.retain(|name, entry| identities.get(name) == Some(&entry.identity));
        for (name, identity) in identities {
            self.entries.entry(name).or_insert(Entry {identity, attempt:0, pending:false,
                due:now, completed:None, result:Value::Null});
        }
    }
    fn request(&mut self, name: Option<&str>, force: bool, now: Instant) {
        for (key, entry) in &mut self.entries {
            if name.is_some_and(|name|name != key) || entry.pending {continue;}
            // Entering the page never defeats retry backoff or re-tests fresh data.
            if force { entry.due = now; }
        }
    }
    fn take(&mut self, now: Instant) -> Vec<Ticket> {
        if self.batch_running {return vec![];}
        let mut due: Vec<_> = self.entries.iter().filter(|(_,e)|!e.pending && now >= e.due)
            .map(|(name,e)|(e.due,name.clone())).collect();
        due.sort(); // Oldest first: a large catalogue cannot starve its tail.
        let tickets = due.into_iter().map(|(_,name)| {
            let entry = self.entries.get_mut(&name).unwrap();
            self.sequence += 1;
            entry.attempt = self.sequence;
            entry.pending = true;
            mark_pending(&mut entry.result);
            Ticket {name,identity:entry.identity.clone(),generation:self.generation,attempt:entry.attempt}
        }).collect::<Vec<_>>();
        self.batch_running = !tickets.is_empty();
        tickets
    }
    fn current(&self, ticket: &Ticket) -> bool {
        ticket.generation == self.generation && self.entries.get(&ticket.name).is_some_and(|e|
            e.identity == ticket.identity && e.attempt == ticket.attempt && e.pending)
    }
    fn accept(&mut self, ticket: &Ticket, result: &Value, now: Instant, wall_ms: u64) -> bool {
        if !self.current(ticket) {return false;}
        let entry = self.entries.get_mut(&ticket.name).unwrap();
        if result["status"] == "testing" {
            // Retry starts retain failures, never prior positive availability.
            mark_pending(&mut entry.result);
            entry.result["lastAttemptAt"] = json!(wall_ms);
            return true;
        }
        let mut value = result.clone();
        value["lastAttemptAt"] = entry.result.get("lastAttemptAt").cloned().unwrap_or(json!(wall_ms));
        if value.get("measuredAt").is_none() {value["measuredAt"] = json!(wall_ms);}
        value["pending"] = json!(false);
        value["lastSuccessAt"] = if value["status"] == "ok" {value["measuredAt"].clone()} else {entry.result["lastSuccessAt"].clone()};
        value["lastDelay"] = if value["status"] == "ok" {value["delay"].clone()} else {entry.result["lastDelay"].clone()};
        value["attemptId"] = json!(ticket.attempt);
        value["networkGeneration"] = json!(ticket.generation);
        value["measurement"] = json!("http_via_node_ms");
        entry.due = now + if value["status"] == "ok" {REFRESH_AFTER} else {RETRY_AFTER};
        entry.completed = Some(now);
        entry.pending = false;
        entry.result = value;
        true
    }
    fn view(&self, now: Instant) -> Value {
        let results: BTreeMap<_,_> = self.entries.iter().map(|(name,e)| {
            let mut value = e.result.clone();
            if value["status"] == "ok" && e.completed.is_some_and(|at| now.saturating_duration_since(at) >= FRESH_FOR) {
                value["status"] = json!("stale"); value["delay"] = Value::Null;
            }
            (name.clone(), value)
        }).collect();
        json!({"results":results,"identities":self.entries.iter().map(|(n,e)|(n.clone(),e.identity.clone())).collect::<BTreeMap<_,_>>(),
            "pending":self.entries.iter().filter(|(_,e)|e.pending).map(|(n,_)|n.clone()).collect::<Vec<_>>(),"generation":self.generation})
    }
}

fn is_failure(value: &Value) -> bool {
    matches!(value["status"].as_str(), Some("timeout"|"unreachable"|"error"))
}
fn mark_pending(value: &mut Value) {
    if !is_failure(value) { *value=json!({"status":"testing","delay":null,"attempts":0,
        "lastSuccessAt":value["lastSuccessAt"],"lastDelay":value["lastDelay"]}); }
    value["pending"]=json!(true);
    value["delay"]=Value::Null;
}
fn overlay_live(view: &mut Value, proxies: &Value) {
    let pending=view["pending"].as_array().cloned().unwrap_or_default();
    let identities=view["identities"].clone();
    for (name,value) in view["results"].as_object_mut().unwrap() {
        let record=&proxies["proxies"][name]["atlasHealth"];
        *value=if record.is_object() && record["probeId"]==identities[name] {record.clone()} else {Value::Null};
        if pending.iter().any(|n|n==name) {mark_pending(value);}
    }
    let mut selected="ATLAS";
    let mut seen=std::collections::HashSet::new();
    while seen.insert(selected) {
        if let Some(next)=proxies["proxies"][selected]["now"].as_str() {selected=next;} else {break;}
    }
    view["selected"]=if identities.get(selected).is_some() {json!(selected)} else {Value::Null};
}

#[derive(Clone, Default)]
pub(crate) struct Refresh(Arc<Mutex<Catalog>>);
fn identities(snapshot: &ReadSnapshot) -> BTreeMap<String,String> {
    snapshot.settings.servers().iter().filter_map(|node|node["name"].as_str()
        .map(|name|(name.to_owned(),latency::node_identity(node)))).collect()
}
impl Refresh {
    pub fn invalidate(&self) {self.0.lock().unwrap_or_else(|e|e.into_inner()).invalidate(Instant::now());}
    pub fn reconcile(&self, snapshot: &ReadSnapshot) {
        let context=format!("{}:{}:{}:{}",snapshot.status,snapshot.client.context_id(),
            serde_json::to_string(&snapshot.settings.dns).unwrap_or_default(),snapshot.settings.mode);
        let mut state=self.0.lock().unwrap_or_else(|e|e.into_inner());
        // ReadState is republished for status polling too. Hash network profiles
        // only on a configuration revision, not once per result or UI read.
        if state.revision==Some(snapshot.revision) && state.context==context {return;}
        state.reconcile(identities(snapshot),context,Instant::now());
        state.revision=Some(snapshot.revision);
    }
    pub fn view(&self, snapshot: &ReadSnapshot, name: Option<&str>, force: bool) -> Value {
        self.reconcile(snapshot);
        let mut state = self.0.lock().unwrap_or_else(|e|e.into_inner());
        state.request(name,force,Instant::now());
        let mut view = state.view(Instant::now());
        drop(state);
        if snapshot.status == "Connected" {
            // Connected UI reads the same authoritative evidence as recovery.
            // A scheduler cache must not resurrect success after a newer probe
            // failure, network epoch change, or controller loss.
            match snapshot.client.api("GET", "/proxies", None) {
                Ok(proxies) => overlay_live(&mut view, &proxies),
                Err(error) => {
                    for value in view["results"].as_object_mut().unwrap().values_mut() {
                        if !is_failure(value) { *value=json!({"status":"error","delay":null,"attempts":0,"error":error}); }
                    }
                }
            }
        }
        view
    }
    pub fn start(&self, reads: ReadState, stop: Arc<AtomicBool>, app: tauri::AppHandle) {
        self.spawn(reads,stop,Arc::new(move || {let _=app.emit("latency-updated",());}));
    }
    fn spawn(&self, reads: ReadState, stop: Arc<AtomicBool>, notify: Arc<dyn Fn()+Send+Sync>) -> std::thread::JoinHandle<()> {
        let refresh = self.clone();
        std::thread::spawn(move || {
            let mut last_tick = std::time::SystemTime::now();
            let mut worker: Option<std::thread::JoinHandle<()>> = None;
            while !stop.load(Ordering::SeqCst) {
                let wall = std::time::SystemTime::now();
                if wall.duration_since(last_tick).map_or(true, |d|d > Duration::from_secs(5)) {refresh.invalidate();}
                last_tick = wall;
                if let Ok(snapshot) = reads.get() {
                    refresh.reconcile(&snapshot);
                    if worker.as_ref().is_some_and(|w|w.is_finished()) {
                        let _ = worker.take().unwrap().join();
                        refresh.0.lock().unwrap_or_else(|e|e.into_inner()).batch_running = false;
                    }
                    if worker.is_none() && !crate::update_health::pending()
                        && matches!(snapshot.status.as_str(), "Connected"|"Disconnected"|"ProtectedPause"|"Error") {
                        let tickets = refresh.0.lock().unwrap_or_else(|e|e.into_inner()).take(Instant::now());
                        if !tickets.is_empty() {
                            let (refresh,reads,stop,notify)=(refresh.clone(),reads.clone(),stop.clone(),notify.clone());
                            worker = Some(std::thread::spawn(move || {
                                let names = tickets.iter().map(|t|t.name.clone()).collect::<Vec<_>>();
                                let by_name: BTreeMap<_,_> = tickets.iter().map(|t|(t.name.clone(),t)).collect();
                                let progress = |name: &str, result: &Value| {
                                    if stop.load(Ordering::SeqCst) {return;}
                                    if let Ok(current) = reads.get() {refresh.reconcile(&current);} else {return;}
                                    let Some(ticket) = by_name.get(name) else {return;};
                                    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64;
                                    let accepted = refresh.0.lock().unwrap_or_else(|e|e.into_inner()).accept(ticket,result,Instant::now(),at);
                                    if accepted {notify();}
                                };
                                let current = |name: &str| {
                                    if let Ok(latest)=reads.get() {refresh.reconcile(&latest);} else {return false;}
                                    !stop.load(Ordering::SeqCst) && by_name.get(name).is_some_and(|ticket|
                                        refresh.0.lock().unwrap_or_else(|e|e.into_inner()).current(ticket))
                                };
                                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    if snapshot.status == "Connected" {
                                        latency::batch_stream_cancel(snapshot.client.clone(),&names,&progress,&current)
                                    } else {
                                        latency::offline_batch_stream_cancel(snapshot.settings.clone(),snapshot.binary.clone(),snapshot.directory.clone(),&names,&progress,&current)
                                    }
                                })).unwrap_or_else(|_|Err("Фоновая проверка прервана; будет повторена".into()));
                                if let Err(error) = outcome {
                                    for name in &names {progress(name,&json!({"status":"error","delay":null,"attempts":0,"error":error}));}
                                }
                            }));
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            refresh.invalidate();
            // Native requests have bounded deadlines; their core/process RAII is
            // allowed to finish before this owner releases its worker.
            if let Some(worker) = worker {let _ = worker.join();}
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "75-second real-network scheduler acceptance; encrypted input opened read-only, isolated core only"]
    fn real_network_cycles() {
        let db=rusqlite::Connection::open_with_flags(std::env::var("ATLAS_BATCH_DATABASE").unwrap(),rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let encrypted:Vec<u8>=db.query_row("SELECT payload FROM state WHERE id=1",[],|r|r.get(0)).unwrap();
        let settings:crate::model::Settings=serde_json::from_slice(&crate::storage::crypt(&encrypted,false).unwrap()).unwrap();
        let count=settings.servers().len();assert!(count>0);
        let directory=std::env::temp_dir().join(format!("atlas-refresh-acceptance-{}",uuid::Uuid::new_v4()));std::fs::create_dir(&directory).unwrap();
        let binary=std::path::PathBuf::from(std::env::var_os("ATLAS_BATCH_CORE").unwrap());
        let core=crate::core::Core::new(binary.clone(),directory.clone());
        let reads=ReadState::default();reads.set(ReadSnapshot {revision:0,settings,status:"Disconnected".into(),client:core.client(),binary,directory:directory.clone()});
        let stop=Arc::new(AtomicBool::new(false));let refresh=Refresh::default();
        let worker=refresh.spawn(reads.clone(),stop.clone(),Arc::new(||{}));
        let begin=Instant::now();let mut attempts=BTreeMap::<String,std::collections::BTreeSet<u64>>::new();
        let mut first=None;let mut max_age=0;let mut max_pending=0;let mut max_observed_age=0;let mut stale_samples=0;let mut latest=Value::Null;
        while begin.elapsed()<Duration::from_secs(75) {
            latest=refresh.view(&reads.get().unwrap(),None,false);
            max_pending=max_pending.max(latest["pending"].as_array().unwrap().len());
            for (name,result) in latest["results"].as_object().unwrap() {
                if let Some(id)=result["attemptId"].as_u64() {attempts.entry(name.clone()).or_default().insert(id);first.get_or_insert(begin.elapsed().as_millis());}
                if let Some(at)=result["measuredAt"].as_u64() {
                    let wall=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
                    max_observed_age=max_observed_age.max(wall.saturating_sub(at));
                    if result["status"]=="stale" {stale_samples+=1;}
                }
                if result["status"]=="ok" {
                    let wall=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
                    max_age=max_age.max(wall.saturating_sub(result["measuredAt"].as_u64().unwrap()));
                }
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        stop.store(true,Ordering::SeqCst);worker.join().unwrap();
        let mut statuses=BTreeMap::<String,usize>::new();for result in latest["results"].as_object().unwrap().values() {*statuses.entry(result["status"].as_str().unwrap_or("missing").into()).or_default()+=1;}
        let min_attempts=attempts.values().map(|v|v.len()).min().unwrap_or(0);
        let remaining=std::fs::read_dir(&directory).unwrap().count();
        println!("REAL_AUTO {}",json!({"nodes":count,"covered":attempts.len(),"minCompletedAttempts":min_attempts,"firstMs":first,
            "durationMs":begin.elapsed().as_millis(),"maxFreshAgeMs":max_age,"maxObservedAgeMs":max_observed_age,"staleSamples":stale_samples,"maxPending":max_pending,"finalStatuses":statuses,"remainingProbeDirectories":remaining}));
        assert_eq!(attempts.len(),count);assert!(min_attempts>=2);assert_eq!(remaining,0);std::fs::remove_dir(directory).unwrap();
    }
    fn catalog(now: Instant) -> Catalog {
        let mut c=Catalog::default();c.reconcile(BTreeMap::from([("a".into(),"config-a".into()),("b".into(),"config-b".into())]),"network".into(),now);c
    }
    #[test]
    fn every_node_retries_once_a_minute_regardless_of_previous_health() {
        let now=Instant::now();let mut c=Catalog::default();
        c.reconcile((0..126).map(|i|(format!("node-{i}"),format!("identity-{i}"))).collect(),"network".into(),now);
        let jobs=c.take(now);assert_eq!(jobs.len(),126);
        for (i,job) in jobs.iter().enumerate() {
            let result=if i%2==0 {json!({"status":"ok","delay":43})} else {json!({"status":"timeout","delay":null})};
            c.accept(job,&result,now,1000);
        }
        c.batch_running=false;
        for second in 1..60 {
            c.request(None,false,now+Duration::from_secs(second));
            assert!(c.take(now+Duration::from_secs(second)).is_empty());
        }
        assert_eq!(c.take(now+Duration::from_secs(60)).len(),126);
        assert!(c.take(now+Duration::from_secs(120)).is_empty());
    }
    #[test]
    fn startup_navigation_manual_and_many_ticks_share_one_queue() {
        let now=Instant::now();let mut c=catalog(now);let work=c.take(now);assert_eq!(work.len(),2);
        for i in 0..1000 {c.request(None,true,now+Duration::from_secs(i));assert!(c.take(now+Duration::from_secs(i)).is_empty());}
        assert_eq!(c.entries.len(),2);
        assert!(c.accept(&work[0],&json!({"status":"ok","delay":1501}),now,100));
        assert_eq!(c.view(now)["results"]["a"]["delay"],1501);
        assert_eq!(c.view(now)["pending"].as_array().unwrap().len(),1);
    }
    #[test]
    fn successive_cycles_failure_retry_and_expiry_use_controlled_time() {
        let now=Instant::now();let mut c=catalog(now);
        for cycle in 0..5 {
            let at=now+REFRESH_AFTER*cycle;let jobs=c.take(at);assert_eq!(jobs.len(),2);
            for job in &jobs {c.accept(job,&json!({"status":"ok","delay":43}),at,1000+cycle as u64*20000);}
            c.batch_running=false;assert!(c.take(at+REFRESH_AFTER-Duration::from_secs(1)).is_empty());
        }
        let at=now+REFRESH_AFTER*5;let jobs=c.take(at);
        c.accept(&jobs[0],&json!({"status":"timeout","delay":null}),at,101000);
        assert!(c.view(at)["results"]["a"]["delay"].is_null());
        assert_eq!(c.view(at)["results"]["a"]["lastSuccessAt"],81000);
        assert!(c.view(at+FRESH_FOR)["results"]["b"]["delay"].is_null());
    }
    #[test]
    fn profile_replacement_deletion_network_resume_reject_late_results() {
        let now=Instant::now();let mut c=catalog(now);let jobs=c.take(now);
        c.reconcile(BTreeMap::from([("a".into(),"new-config".into())]),"network".into(),now);
        for job in &jobs {assert!(!c.accept(job,&json!({"status":"ok","delay":5}),now,1));}
        c.batch_running=false;let jobs=c.take(now);c.invalidate(now);
        assert!(!c.accept(&jobs[0],&json!({"status":"ok","delay":5}),now,1));
        c.batch_running=false;assert_eq!(c.take(now).len(),1);
    }
    #[test]
    fn starting_or_reading_never_renews_a_success_and_failure_overrules_it() {
        let now=Instant::now();let mut c=catalog(now);let jobs=c.take(now);
        c.accept(&jobs[0],&json!({"status":"ok","delay":42}),now,1000);c.batch_running=false;
        let next=c.take(now+REFRESH_AFTER);
        c.accept(&next[0],&json!({"status":"testing"}),now+REFRESH_AFTER,21000);
        assert!(c.view(now+REFRESH_AFTER)["results"]["a"]["measuredAt"].is_null());
        assert!(c.view(now+REFRESH_AFTER)["results"]["a"]["delay"].is_null());
        assert!(c.view(now+FRESH_FOR)["results"]["a"]["delay"].is_null());
        c.accept(&next[0],&json!({"status":"error","delay":null}),now+REFRESH_AFTER,22000);
        assert!(c.view(now+REFRESH_AFTER)["results"]["a"]["delay"].is_null());
    }
    #[test]
    #[ignore = "65-second wall-clock lifecycle test with a loopback controller; no user data or VPN changes"]
    fn real_timer_cycles_without_a_page() {
        use std::io::{Read,Write};
        let listener=std::net::TcpListener::bind("127.0.0.1:0").unwrap();listener.set_nonblocking(true).unwrap();
        let port=listener.local_addr().unwrap().port();let stop=Arc::new(AtomicBool::new(false));let stopped=stop.clone();
        let calls=Arc::new(std::sync::atomic::AtomicUsize::new(0));let count=calls.clone();
        let server=std::thread::spawn(move || {
            while !stopped.load(Ordering::SeqCst) {
                if let Ok((mut stream,_))=listener.accept() {
                    // Windows inherits the listener's nonblocking mode. A
                    // partial request must wait for its remaining headers;
                    // otherwise the fixture closes a healthy connection.
                    stream.set_nonblocking(false).unwrap();
                    stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                    let mut bytes=Vec::new();let mut chunk=[0;1024];
                    while !bytes.windows(4).any(|w|w==b"\r\n\r\n") && bytes.len()<8192 {
                        let n=stream.read(&mut chunk).unwrap_or(0);if n==0 {break;}bytes.extend_from_slice(&chunk[..n]);
                    }
                    let request=String::from_utf8_lossy(&bytes);
                    let body=if request.contains("/delay?") {count.fetch_add(1,Ordering::SeqCst);json!({"delay":43})} else {json!({"proxies":{"a":{"type":"Socks5","extra":{format!("{}?atlas_probe=1",latency::DISPLAY_URL):{"alive":true}}}}})}.to_string();
                    let _=write!(stream,"HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body);
                } else {std::thread::sleep(Duration::from_millis(1));}
            }
        });
        let mut settings=crate::model::Settings::default();
        settings.subscriptions.push(crate::model::Subscription {source:crate::model::SubscriptionSource::Url,
            id:"fixture".into(),name:"fixture".into(),masked_url:String::new(),options:Default::default(),updated_at:0,error:None,
            servers:vec![json!({"name":"a","server":"127.0.0.1","type":"socks5","port":12345})]});
        let client=crate::core::ApiClient::loopback_fixture(port).with_probe_settings(&settings);
        let reads=ReadState::default();reads.set(ReadSnapshot {revision:0,settings,status:"Connected".into(),
            client,binary:Default::default(),directory:Default::default()});
        let refresh=Refresh::default();let notified=Arc::new(std::sync::atomic::AtomicUsize::new(0));let events=notified.clone();
        let worker=refresh.spawn(reads.clone(),stop.clone(),Arc::new(move || {events.fetch_add(1,Ordering::SeqCst);}));
        let begin=Instant::now();let mut first=None;let mut max_age=0;
        while begin.elapsed()<Duration::from_secs(65) {
            let view=refresh.view(&reads.get().unwrap(),None,false);
            if let Some(at)=view["results"]["a"]["measuredAt"].as_u64().filter(|_|view["results"]["a"]["status"]=="ok") {
                first.get_or_insert(begin.elapsed().as_millis());
                let wall=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
                max_age=max_age.max(wall.saturating_sub(at));
                assert_eq!(view["results"]["a"]["delay"],43,"{view}");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        stop.store(true,Ordering::SeqCst);worker.join().unwrap();server.join().unwrap();
        assert_eq!(calls.load(Ordering::SeqCst),2);
        assert!(max_age>=59000 && max_age<FRESH_FOR.as_millis() as u64);
        println!("AUTO_CYCLES {}",json!({"durationMs":begin.elapsed().as_millis(),"firstMs":first,"requests":calls.load(Ordering::SeqCst),"maxAgeMs":max_age,"events":notified.load(Ordering::SeqCst)}));
    }
    #[test]
    fn pending_keeps_failure_and_old_completion_cannot_renew_evidence() {
        let now=Instant::now();let mut c=catalog(now);let first=c.take(now);
        c.accept(&first[0],&json!({"status":"unreachable","errorKind":"timeout","delay":null,"measuredAt":100}),now,200);
        c.batch_running=false;
        let retry=c.take(now+RETRY_AFTER);
        let view=c.view(now+Duration::from_secs(60));
        assert_eq!(view["results"]["a"]["status"],"unreachable");
        assert_eq!(view["results"]["a"]["pending"],true);
        assert_eq!(view["results"]["a"]["measuredAt"],100);
        assert!(!c.accept(&first[0],&json!({"status":"ok","delay":56}),now,500));
        c.accept(&retry[0],&json!({"status":"ok","delay":81,"measuredAt":300}),now,500);
        assert_eq!(c.view(now)["results"]["a"]["measuredAt"],300);
        assert_eq!(c.view(now)["results"]["a"]["pending"],false);
    }
    #[test]
    fn connected_view_uses_latest_core_failure_and_rejects_replaced_profile() {
        let mut view=json!({"results":{"a":{"status":"ok","delay":56}},"identities":{"a":"id"},"pending":["a"]});
        let proxies=json!({"proxies":{"ATLAS":{"now":"AUTO"},"AUTO":{"now":"a"},"a":{"atlasHealth":{
            "probeId":"id","status":"unreachable","errorKind":"timeout","delay":null,"measuredAt":100}}}});
        overlay_live(&mut view,&proxies);
        assert_eq!(view["selected"],"a");assert_eq!(view["results"]["a"]["status"],"unreachable");
        assert_eq!(view["results"]["a"]["pending"],true);
        view["pending"]=json!([]);view["identities"]["a"]=json!("replaced");overlay_live(&mut view,&proxies);
        assert!(view["results"]["a"].is_null());
    }
}

//! Service-owned recovery. Workers only propose; the command thread commits.
use crate::{core::ApiClient, model::Settings};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::{HashMap, HashSet}, sync::mpsc, time::{Duration, Instant}};

const QUARANTINE: u64 = 30;
fn remote_probe_failure(error: &str) -> bool {
    matches!(error,"Mihomo API: HTTP 503"|"Mihomo API: HTTP 504")
        || error.contains("Контрольный URL не подтвердил")
}
pub(crate) fn node_key(node: &Value) -> String {
    format!("{:x}", Sha256::digest(node.to_string().as_bytes()))
}
fn endpoint(node: &Value) -> String {
    format!("{}:{}",node["server"].as_str().unwrap_or(""),node["port"].as_u64().unwrap_or(0))
}
fn failed_endpoint(line: &str) -> Option<String> {
    if line.starts_with("ATLAS_EVENT") || !line.contains("dial ATLAS") || !line.contains("connect error:") { return None; }
    let before = line.split(" connect error:").next()?;
    Some(before.rsplit("error: ").next()?.trim().to_owned())
}
pub(crate) fn candidates(settings: &Settings, proxies: &Value, blocked: &HashSet<String>, _now: u64) -> Vec<String> {
    let mut nodes: Vec<_> = settings.servers().into_iter().filter(|n| !blocked.contains(&node_key(n))).collect();
    nodes.sort_by_key(|n| {
        let p=&proxies["proxies"][n["name"].as_str().unwrap_or("")];
        (p["alive"] != true,p["history"].as_array()
            .and_then(|h|h.last()).and_then(|v|v["delay"].as_u64()).filter(|d|*d > 0).unwrap_or(u64::MAX))
    });
    // Same IP:port can host different SNI/Reality keys/transports. Every
    // configured node must remain eligible for a fresh recovery check.
    nodes.into_iter().filter_map(|n|n["name"].as_str().map(str::to_owned)).collect()
}

struct Pending { rx: mpsc::Receiver<Value>, generation: u64, repair_only: bool }
struct Quarantine { probe_after: Instant, failures: u32 }
struct NodeSample { checked_at: Instant, primary_delay: Option<u64>, alive: bool }
struct Scheduled {
    rx: mpsc::Receiver<Vec<(String, Option<u64>, bool)>>,
    generation: u64,
    current: String,
    purpose: &'static str,
}
#[derive(Default)]
pub(crate) struct Recovery {
    generation: u64,
    route_revision: u64,
    last_line: Option<String>,
    blocked: HashMap<String,Quarantine>,
    network_epoch: Option<String>,
    pending: Option<Pending>,
    retry_at: Option<Instant>,
    needed: bool,
    scheduled: Option<Scheduled>,
    samples: HashMap<String,NodeSample>,
    last_active: Option<Instant>,
    active: Option<Scheduled>,
    slow_samples: u8,
    last_reserves: Option<Instant>,
    last_pool: Option<Instant>,
    pool_cursor: usize,
    observed_current: Option<String>,
    current_since: Option<Instant>,
    improvement: Option<(String,u8)>,
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl Recovery {
    pub(crate) fn path_epoch(&self) -> (u64,u64) { (self.generation,self.route_revision) }
    pub(crate) fn network_changed(&mut self) {
        self.blocked.clear();
        self.invalidate();
    }
    #[cfg(test)]
    pub(crate) fn propose_for_test(&mut self, value: Value) {
        let (tx,rx)=mpsc::channel();tx.send(value).unwrap();
        self.pending=Some(Pending{rx,generation:self.generation,repair_only:false});
    }
    #[cfg(test)]
    pub(crate) fn quarantined_count(&self) -> usize { self.blocked.len() }
    pub fn invalidate(&mut self) {
        self.cancelled.store(true,std::sync::atomic::Ordering::SeqCst);
        self.cancelled=Default::default();
        self.generation += 1; self.pending = None; self.needed = false; self.retry_at = None;
        self.scheduled = None; self.samples.clear(); self.last_active = None;
        self.last_reserves = None; self.last_pool = None; self.observed_current = None;
        self.current_since = None; self.improvement = None; self.active = None; self.slow_samples = 0;
    }
    fn poll_active(&mut self, client: &ApiClient, settings: &Settings, now: Instant) {
        let Some(job) = &self.active else { return; };
        let result = job.rx.try_recv();
        if matches!(result, Err(mpsc::TryRecvError::Empty)) { return; }
        let job = self.active.take().unwrap();
        if job.generation != self.generation { return; }
        let Ok(results) = result else { return; };
        for (name, delay, alive) in results {
            if self.observed_current.as_ref() != Some(&name) {
                self.observed_current = Some(name.clone()); self.current_since = Some(now);
                self.slow_samples = 0; self.improvement = None;
            }
            self.slow_samples = if delay.is_some_and(|d| d > settings.auto_search_ping_ms) {
                self.slow_samples.saturating_add(1)
            } else { 0 };
            if self.slow_samples == 0 {
                self.improvement = None; self.pool_cursor=0; self.last_pool=None;
            }
            if !alive {
                self.needed = true; self.retry_at = None;
                // Do not wait for a second diagnosis of the failed node.
                if let Some(node) = settings.servers().iter().find(|n|n["name"] == name) {
                    self.blocked.entry(node_key(node)).or_insert(Quarantine {
                        probe_after: now + Duration::from_secs(60), failures: 0 });
                }
            }
            client.event(json!({"at":crate::model::now(),"kind":"active_node_sample",
                "name":name,"delay":delay,"responded":alive,"slowSamples":self.slow_samples}));
            self.samples.insert(name, NodeSample {checked_at:now,primary_delay:delay,alive});
        }
    }
    fn monitor(&mut self, client: ApiClient, settings: &Settings, now: Instant) {
        if self.active.is_some() || self.last_active.is_some_and(|at|now.duration_since(at)<Duration::from_secs(10)) { return; }
        self.last_active = Some(now);
        let (tx,rx)=mpsc::channel();
        self.active=Some(Scheduled{rx,generation:self.generation,current:String::new(),purpose:"active"});
        let settings=settings.clone();
        std::thread::spawn(move || {
            let name = if matches!(settings.selected.as_str(),"AUTO"|"FAILOVER") {
                match client.api("GET",&format!("/proxies/{}",settings.selected),None) {
                    Ok(v) => v["now"].as_str().map(str::to_owned),
                    Err(e) => { client.event(json!({"at":crate::model::now(),"kind":"active_monitor_error","error":e})); None }
                }
            } else { Some(settings.selected) };
            let results = name.into_iter().filter_map(|name| {
                let probe=crate::latency::verified_probe(&client,&name,crate::latency::DISPLAY_URL);
                if let Err(error)=&probe {
                    if !remote_probe_failure(error) {
                        client.event(json!({"at":crate::model::now(),"kind":"active_monitor_error","error":error}));
                        return None;
                    }
                }
                let delay=probe.as_ref().ok().and_then(|v|v["delay"].as_u64());
                Some((name,delay,probe.is_ok()))
            }).collect();
            let _=tx.send(results);
        });
    }
    fn poll_scheduled(&mut self, client: &ApiClient, settings: &Settings, now: Instant) {
        let Some(pending) = &self.scheduled else { return; };
        let result = pending.rx.try_recv();
        if matches!(result, Err(mpsc::TryRecvError::Empty)) { return; }
        let pending = self.scheduled.take().unwrap();
        if pending.generation != self.generation { return; }
        let Ok(results) = result else { return; };
        for (name, delay, alive) in &results {
            self.samples.insert(name.clone(), NodeSample {
                checked_at: now, primary_delay: *delay, alive: *alive });
            if name == &pending.current && !alive { self.needed = true; }
        }
        if self.needed || self.pending.is_some() || self.observed_current.as_deref() != Some(&pending.current) || pending.purpose != "reserves" || !matches!(settings.selected.as_str(),"AUTO"|"FAILOVER") { return; }
        let Some(current) = self.samples.get(&pending.current).filter(|s|s.alive && s.checked_at.elapsed() < Duration::from_secs(90)) else { return; };
        let Some(current_delay) = current.primary_delay else { return; };
        if self.slow_samples < 3 || current_delay <= settings.auto_search_ping_ms {
            self.improvement = None;
            return;
        }
        let best = results.iter().filter_map(|(name,delay,alive)| {
            if name == &pending.current || !alive || self.blocked.contains_key(&settings.servers().iter()
                .find(|n|n["name"] == *name).map(node_key).unwrap_or_default()) { return None; }
            delay.map(|d|(name.as_str(),d))
        }).min_by_key(|(_,delay)|*delay);
        let Some((name, delay)) = best else { self.improvement=None; return; };
        let threshold = 30u64.max(current_delay / 5);
        if current_delay.saturating_sub(delay) < threshold {
            self.improvement=None; return;
        }
        let count = if self.improvement.as_ref().is_some_and(|(candidate,_)|candidate == name) {
            self.improvement.as_ref().unwrap().1.saturating_add(1)
        } else { 1 };
        self.improvement = Some((name.to_owned(),count));
        if count < 2 || self.current_since.is_none_or(|since|since.elapsed() < Duration::from_secs(60)) { return; }
        let group = settings.selected.as_str();
        self.route_revision=self.route_revision.wrapping_add(1);
        if client.api("PUT",&format!("/proxies/{group}"),Some(json!({"name":name}))).is_ok()
            && client.api("GET",&format!("/proxies/{group}"),None).is_ok_and(|v|v["now"] == name) {
            self.observed_current = Some(name.to_owned());
            self.current_since = Some(now);
            self.improvement = None;
            self.active=None; self.last_active=None; self.slow_samples=0;
            self.pool_cursor=0; self.last_pool=None;
            client.event(json!({"at":crate::model::now(),"kind":"automatic_optimization",
                "from":pending.current,"to":name,"improvementMs":current_delay-delay,
                "series":2,"heldSeconds":60}));
        }
    }
    fn schedule(&mut self, client: ApiClient, settings: &Settings, now: Instant) {
        if self.scheduled.is_some() || self.pending.is_some() || self.needed { return; }
        let automatic = matches!(settings.selected.as_str(),"AUTO"|"FAILOVER");
        let Some(current) = self.observed_current.clone() else { return; };
        if !automatic || self.slow_samples < 3 { return; }
        if self.last_reserves.is_some_and(|at|now.duration_since(at)<Duration::from_secs(2)) { return; }
        if self.last_pool.is_some_and(|at|now.duration_since(at)<Duration::from_secs(60)) { return; }
        let nodes=settings.servers();
        let mut backups:Vec<_>=nodes.iter().filter(|n|n["name"] != current && !self.blocked.contains_key(&node_key(n)))
            .filter_map(|n|n["name"].as_str().map(str::to_owned)).collect();
        backups.sort_by_key(|name|self.samples.get(name).filter(|s|s.alive).and_then(|s|s.primary_delay).unwrap_or(u64::MAX));
        let mut names=Vec::new();
        if let Some((name,_))=&self.improvement { names.push(name.clone()); }
        for _ in 0..backups.len().min(2) {
            if self.pool_cursor >= backups.len() { self.pool_cursor=0; self.last_pool=Some(now); break; }
            let name=backups[self.pool_cursor].clone(); self.pool_cursor+=1;
            if !names.contains(&name) { names.push(name); }
            if names.len()==2 { break; }
        }
        self.last_reserves=Some(now);
        let purpose="reserves";
        if names.is_empty() { return; }
        let (tx,rx)=mpsc::channel();
        self.scheduled=Some(Scheduled{rx,generation:self.generation,current,purpose});
        std::thread::spawn(move || {
            let mut results = Vec::with_capacity(names.len());
            // Health never launches the whole pool at once. Two network probes
            // leave room for a six-worker manual batch on the same controller.
            for pair in names.chunks(2) {
                results.extend(std::thread::scope(|scope| {
                    let jobs: Vec<_>=pair.iter().map(|name| {
                        let client=&client;
                        scope.spawn(move || {
                            let primary=crate::latency::verified_probe(client,name,crate::latency::DISPLAY_URL);
                            match primary {
                                Ok(value) => (name.clone(),value["delay"].as_u64(),true),
                                Err(_) => (name.clone(),None,
                                    crate::latency::verified_probe(client,name,crate::latency::SECONDARY_URL).is_ok())
                            }
                        })
                    }).collect();
                    jobs.into_iter().filter_map(|j|j.join().ok()).collect::<Vec<_>>()
                }));
            }
            let _=tx.send(results);
        });
    }
    pub fn tick(&mut self, client: ApiClient, settings: &Settings) {
        let now = Instant::now();
        if let Some(epoch) = crate::network_guard::default_route_signature() {
            if self.network_epoch.as_ref().is_some_and(|old|old != &epoch) {
                self.blocked.clear();
                self.invalidate();
                client.event(json!({"at":crate::model::now(),"kind":"network_epoch_changed","health":"prior network evidence discarded"}));
            }
            self.network_epoch = Some(epoch);
        }
        let servers = settings.servers();
        self.blocked.retain(|key,_| servers.iter().any(|n|node_key(n)==*key));
        let lines = client.logs().unwrap_or_default();
        let fresh = self.last_line.as_ref().and_then(|last|lines.iter().rposition(|l|l == last)).map_or(0,|i|i+1);
        for line in &lines[fresh..] {
            if let Some(peer) = failed_endpoint(line) {
                let matching: Vec<_> = servers.iter().filter(|n|endpoint(n) == peer).collect();
                if !matching.is_empty() {
                    // A single application connection error is not a failed
                    // health check, nor evidence against every node at this IP.
                    // The active monitor confirms failure before replacement.
                    client.event(json!({"at":crate::model::now(),"kind":"endpoint_connection_error",
                        "endpoint":peer,"matchingNodes":matching.len(),"trigger":line}));
                }
            }
        }
        self.last_line = lines.last().cloned();
        self.poll_active(&client,settings,now);
        self.poll_scheduled(&client,settings,now);
        if !self.needed && self.pending.as_ref().is_none_or(|p|p.repair_only) { self.monitor(client.clone(),settings,now); }
        let automatic = matches!(settings.selected.as_str(),"AUTO"|"FAILOVER")
            && settings.routing_mode != crate::model::RoutingMode::Direct;
        if !automatic { self.needed = false; }
        if self.needed && self.pending.as_ref().is_some_and(|p|p.repair_only) {
            // A quarantine diagnosis must never delay restoring the active path.
            self.pending = None;
        }
        if let Some(p) = &self.pending {
            match p.rx.try_recv() {
                Ok(report) => {
                    let generation = p.generation;
                    let repair_only = p.repair_only;
                    self.pending = None;
                    if generation != self.generation { return; }
                    for test in report["results"].as_array().into_iter().flatten() {
                        if let Some(node)=servers.iter().find(|n|n["name"] == test["name"]) {
                            let key = node_key(node);
                            if test["passed"] == true {
                                self.blocked.remove(&key);
                            } else if test["passed"] == false {
                                let failure = self.blocked.entry(key).or_insert(Quarantine {
                                    probe_after: now, failures: 0 });
                                failure.failures = failure.failures.saturating_add(1);
                                let wait = (QUARANTINE.saturating_mul(1u64 << failure.failures.min(2))).min(120);
                                failure.probe_after = now + Duration::from_secs(wait);
                            }
                        }
                    }
                    if repair_only {
                        client.event(json!({"at":crate::model::now(),"kind":"quarantine_recheck","evidence":report}));
                        return;
                    }
                    if !automatic { return; }
                    let selected = report["candidate"].as_str().filter(|name| settings.servers().iter().any(|n|
                        n["name"] == *name && !self.blocked.contains_key(&node_key(n))));
                    let group = settings.selected.as_str();
                    if selected.is_some() { self.route_revision=self.route_revision.wrapping_add(1); }
                    let result = selected.map(|name|client.api("PUT",&format!("/proxies/{group}"),Some(json!({"name":name}))).and_then(|_| {
                        let state=client.api("GET","/proxies",None)?;
                        if state["proxies"][group]["now"] != name { return Err("Ядро не подтвердило выбранную альтернативу".into()); }
                        Ok(json!({"confirmedSelection":name}))
                    }));
                    if result.as_ref().is_some_and(|r|r.is_ok()) {
                        self.needed = false;
                        self.observed_current = selected.map(str::to_owned);
                        self.current_since = Some(now); self.slow_samples=0;
                        self.last_active=None; self.improvement=None;
                        self.active=None; self.scheduled=None; self.pool_cursor=0; self.last_pool=None;
                    } else { self.needed = true; }
                    self.retry_at = Some(now + Duration::from_secs(30));
                    client.event(json!({"at":crate::model::now(),"kind":"recovery_result","evidence":report,
                        "selectorResult":result.map(|r|r.unwrap_or_else(|e|json!({"error":e}))),
                        "scope":"Replacement answered the control URL; old-node diagnosis is deferred"}));
                }
                Err(mpsc::TryRecvError::Disconnected) => { self.pending = None; self.needed = true; self.retry_at = Some(now + Duration::from_secs(30)); }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.pending.is_some() { return; }
        if !self.needed {
            if self.scheduled.is_some() { return; }
            if let Some(node) = servers.iter().find(|n| self.blocked.get(&node_key(n)).is_some_and(|b|b.probe_after <= now)) {
                let name = node["name"].as_str().unwrap_or("").to_owned();
                let (tx,rx) = mpsc::channel();
                self.pending = Some(Pending{rx,generation:self.generation,repair_only:true});
                // Mark the next attempt while this trial is in flight. No user
                // traffic uses this node until both control rounds succeed.
                if let Some(b) = self.blocked.get_mut(&node_key(node)) { b.probe_after = now + Duration::from_secs(QUARANTINE); }
                std::thread::spawn(move || { let _ = tx.send(verify_names(&client,&[name],&crate::latency::ENDPOINTS)); });
            } else {
                self.schedule(client,settings,now);
            }
            return;
        }
        if self.retry_at.is_some_and(|t|t>now) { return; }
        self.needed = false;
        self.retry_at = Some(now + Duration::from_secs(30));
        let (tx,rx) = mpsc::channel();
        let mut blocked: HashSet<_> = self.blocked.iter().filter(|(_,b)|b.probe_after > now).map(|(key,_)|key.clone()).collect();
        if let Some(node)=servers.iter().find(|n|n["name"].as_str()==self.observed_current.as_deref()) {
            blocked.insert(node_key(node));
        }
        let settings = settings.clone();
        client.event(json!({"at":crate::model::now(),"kind":"recovery_started","quarantinedNodes":self.blocked.len()}));
        self.pending = Some(Pending{rx,generation:self.generation,repair_only:false});
        let cancelled=self.cancelled.clone();
        let previous=self.observed_current.clone();
        std::thread::spawn(move || { let _ = tx.send(verify_replacement(&client,&settings,&blocked,previous.as_deref(),cancelled)); });
    }
}

// Return the first freshly verified replacement, without waiting for a slow
// peer or diagnosing the old node. At most two probes remain in flight.
fn verify_replacement(client: &ApiClient, settings: &Settings, blocked: &HashSet<String>, previous: Option<&str>, cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>) -> Value {
    use std::sync::{Arc, atomic::{AtomicBool, AtomicUsize, Ordering}};
    let proxies = match client.api("GET","/proxies",None) { Ok(v)=>v,Err(e)=>return json!({"error":e}) };
    let names=Arc::new(candidates(settings,&proxies,blocked,crate::model::now()));
    let next=Arc::new(AtomicUsize::new(0));
    let stop=Arc::new(AtomicBool::new(false));
    let (tx,rx)=mpsc::channel();
    for _ in 0..names.len().min(2) {
        let (names,next,stop,tx,client,cancelled)=(names.clone(),next.clone(),stop.clone(),tx.clone(),client.clone(),cancelled.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) && !cancelled.load(Ordering::SeqCst) {
                let Some(name)=names.get(next.fetch_add(1,Ordering::SeqCst)) else { break; };
                let result=crate::latency::verified_probe(&client,name,crate::latency::DISPLAY_URL);
                let passed=result.is_ok();
                let remote_failure=result.as_ref().err().is_some_and(|e|remote_probe_failure(e));
                if passed { stop.store(true,Ordering::SeqCst); }
                if tx.send(json!({"name":name,"passed":if passed {Some(true)} else if remote_failure {Some(false)} else {None},"result":result.unwrap_or_else(|e|json!({"error":e}))})).is_err() { break; }
                if !passed && !remote_failure { break; }
            }
        });
    }
    drop(tx);
    let mut results=Vec::new();
    for result in rx {
        let candidate=result["name"].clone();
        let passed=result["passed"]==true;
        results.push(result);
        if passed { return json!({"candidate":candidate,"results":results}); }
    }
    // Only after exhausting replacements, recheck the old path. A recovered
    // original node must not leave the client searching forever.
    if !cancelled.load(Ordering::SeqCst) {
        if let Some(name)=previous {
            if let Ok(result)=crate::latency::verified_probe(client,name,crate::latency::DISPLAY_URL) {
                results.push(json!({"name":name,"passed":true,"result":result}));
                return json!({"candidate":name,"results":results});
            }
        }
    }
    json!({"candidate":null,"results":results})
}

impl Drop for Recovery {
    fn drop(&mut self) { self.cancelled.store(true,std::sync::atomic::Ordering::SeqCst); }
}

pub(crate) fn verify(client: &ApiClient, settings: &Settings, blocked: &HashSet<String>, controls: &[&str]) -> Value {
    let proxies = match client.api("GET","/proxies",None) { Ok(p)=>p,Err(e)=>return json!({"error":e}) };
    let names: Vec<_> = candidates(settings,&proxies,blocked,crate::model::now()).into_iter().take(4).collect();
    verify_names(client, &names, controls)
}

pub(crate) fn verify_names(client: &ApiClient, names: &[String], controls: &[&str]) -> Value {
    let mut results = Vec::new();
    // Recovery stays responsive without competing with a manual pool check.
    for pair in names.chunks(2) {
        results.extend(std::thread::scope(|scope| {
            let jobs: Vec<_> = pair.iter().map(|name|scope.spawn(move || {
                let mut checks = Vec::new();
                for round in 0..2 {
                    let series = std::thread::scope(|scope| {
                        let jobs: Vec<_> = controls.iter().enumerate().map(|(control,url)|scope.spawn(move || {
                            let at = crate::model::now();
                            let result = crate::latency::verified_probe(client,name,url).unwrap_or_else(|e|json!({"error":e}));
                            json!({"round":round,"control":control,"at":at,"result":result})
                        })).collect();
                        jobs.into_iter().map(|job|job.join().unwrap_or(json!({"error":"Probe worker failed"}))).collect::<Vec<_>>()
                    });
                    let passed = series.iter().any(|c|c["result"]["expectedStatusMatched"] == true);
                    checks.extend(series);
                    // One blocked control host does not condemn the VPN node.
                    if !passed { return json!({"name":name,"passed":false,"checks":checks}); }
                    if round == 0 { std::thread::sleep(Duration::from_millis(500)); }
                }
                json!({"name":name,"passed":true,"checks":checks})
            })).collect();
            jobs.into_iter().map(|j|j.join().unwrap_or(json!({"error":"Probe worker failed"}))).collect::<Vec<_>>()
        }));
        if results.iter().any(|r|r["passed"] == true) { break; }
    }
    let candidate = results.iter().find(|r|r["passed"]==true).map(|r|r["name"].clone());
    json!({"candidate":candidate,"results":results})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replacement_is_returned_before_the_slow_candidate_finishes() {
        use std::{io::{Read,Write}, net::TcpListener, sync::{Arc,Mutex,atomic::{AtomicBool,Ordering}}};
        // Other HTTP tests can leave cancelled workers draining the shared
        // admission gate. Establish an idle gate before measuring this fixture;
        // queue time from a preceding test is not replacement-selection latency.
        let idle:Vec<_>=(0..8).map(|_|crate::query_admission::acquire(true).unwrap()).collect();
        drop(idle);
        let listener=TcpListener::bind("127.0.0.1:0").unwrap();
        let port=listener.local_addr().unwrap().port(); listener.set_nonblocking(true).unwrap();
        let stop=Arc::new(AtomicBool::new(false)); let stopped=stop.clone();
        let (release,wait)=mpsc::channel(); let wait=Arc::new(Mutex::new(wait));
        let slow_finished=Arc::new(AtomicBool::new(false));let observed_slow=slow_finished.clone();
        let server=std::thread::spawn(move || {
            while !stopped.load(Ordering::SeqCst) {
                if let Ok((mut socket,_))=listener.accept() {
                    let wait=wait.clone();
                    let slow_finished=slow_finished.clone();
                    std::thread::spawn(move || {
                        socket.set_nonblocking(false).unwrap();
                        socket.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                        let mut request=[0;4096]; let n=socket.read(&mut request).unwrap_or(0);
                        let request=String::from_utf8_lossy(&request[..n]);
                        if request.contains("/slow/delay?") { let _=wait.lock().unwrap().recv_timeout(Duration::from_secs(30));slow_finished.store(true,Ordering::SeqCst); }
                        let body=if request.contains("/delay?") { json!({"delay":40}) } else {
                            let health=json!({"alive":true,"extra":{crate::latency::DISPLAY_URL:{"alive":true}}});
                            json!({"proxies":{"slow":health.clone(),"fast":health}})
                        }.to_string();
                        let _=write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body);
                    });
                } else { std::thread::sleep(Duration::from_millis(5)); }
            }
        });
        let mut settings=Settings::default();
        settings.subscriptions.push(crate::model::Subscription { source: Default::default(), options: Default::default(),id:"fixture".into(),name:"fixture".into(),masked_url:String::new(),updated_at:0,error:None,
            servers:vec![json!({"name":"slow","server":"127.0.0.1","port":1}),json!({"name":"fast","server":"127.0.0.1","port":2})]});
        let (tx,rx)=mpsc::channel();
        std::thread::spawn(move || { let _=tx.send(verify_replacement(&ApiClient::loopback_fixture(port),&settings,&HashSet::new(),None,Default::default())); });
        let result=rx.recv_timeout(Duration::from_secs(3));
        let returned_before_slow=!observed_slow.load(Ordering::SeqCst);
        let _=release.send(()); stop.store(true,Ordering::SeqCst); server.join().unwrap();
        assert_eq!(result.unwrap()["candidate"],"fast");
        assert!(returned_before_slow,"Replacement waited for the blocked candidate");
    }
    #[test]
    fn controller_errors_do_not_quarantine_remote_nodes() {
        assert!(!remote_probe_failure("Mihomo API недоступен: timeout"));
        assert!(!remote_probe_failure("Сетевая служба занята. Повторите операцию."));
        assert!(remote_probe_failure("Mihomo API: HTTP 504"));
    }
    fn sample(recovery: &mut Recovery, settings: &Settings, delay: Option<u64>) {
        let (tx,rx)=mpsc::channel();
        tx.send(vec![("fixture".into(),delay,delay.is_some())]).unwrap();
        recovery.active=Some(Scheduled{rx,generation:recovery.generation,current:String::new(),purpose:"active"});
        let core=crate::core::Core::new(std::path::PathBuf::new(),std::path::PathBuf::new());
        recovery.poll_active(&core.client(),settings,Instant::now());
    }
    #[test]
    fn active_monitor_requires_three_slow_samples_and_resets_after_recovery() {
        let settings=Settings::default(); let mut recovery=Recovery::default();
        sample(&mut recovery,&settings,Some(151)); assert_eq!(recovery.slow_samples,1);
        sample(&mut recovery,&settings,Some(180)); assert_eq!(recovery.slow_samples,2);
        sample(&mut recovery,&settings,Some(160)); assert_eq!(recovery.slow_samples,3);
        sample(&mut recovery,&settings,Some(150)); assert_eq!(recovery.slow_samples,0);
        assert!(!recovery.needed);
    }
    #[test]
    fn custom_threshold_and_failure_are_independent() {
        let mut settings=Settings::default(); settings.auto_search_ping_ms=500;
        let mut recovery=Recovery::default();
        sample(&mut recovery,&settings,Some(450)); assert_eq!(recovery.slow_samples,0);
        recovery.retry_at=Some(Instant::now()+Duration::from_secs(60));
        sample(&mut recovery,&settings,None);
        assert!(recovery.needed); assert!(recovery.retry_at.is_none());
    }
    #[test]
    fn stale_monitor_result_cannot_override_new_session() {
        let mut recovery=Recovery::default(); let settings=Settings::default();
        let (tx,rx)=mpsc::channel(); tx.send(vec![("old".into(),None,false)]).unwrap();
        recovery.active=Some(Scheduled{rx,generation:1,current:String::new(),purpose:"active"});
        recovery.generation=2;
        let core=crate::core::Core::new(std::path::PathBuf::new(),std::path::PathBuf::new());
        recovery.poll_active(&core.client(),&settings,Instant::now());
        assert!(!recovery.needed); assert!(recovery.samples.is_empty());
    }
    #[test]
    fn quarantine_is_bound_to_node_configuration() {
        let mut s=Settings::default();
        s.subscriptions.push(crate::model::Subscription { source: Default::default(), options: Default::default(),id:"x".into(),name:"x".into(),masked_url:"".into(),updated_at:0,error:None,
            servers:vec![json!({"name":"Sweden","server":"1.2.3.4","port":443}),json!({"name":"Germany","server":"1.2.3.4","port":443}),json!({"name":"other","server":"5.6.7.8","port":443})]});
        let p=json!({"proxies":{"Sweden":{"alive":true},"Germany":{"alive":true},"other":{"alive":true}}});
        let first = node_key(&s.servers()[0]);
        assert_eq!(candidates(&s,&p,&HashSet::new(),0),vec!["Sweden","Germany","other"]);
        assert_eq!(candidates(&s,&p,&HashSet::from([first]),0),vec!["Germany","other"]);
        let both = s.servers()[..2].iter().map(node_key).collect();
        assert_eq!(candidates(&s,&p,&both,0),vec!["other"]);
        assert_eq!(failed_endpoint("[TCP] dial ATLAS error: 1.2.3.4:443 connect error: context deadline exceeded"),Some("1.2.3.4:443".into()));
        assert!(failed_endpoint("[TCP] dial DIRECT error: dns resolve failed").is_none());
        assert!(failed_endpoint("ATLAS_EVENT {\"trigger\":\"[TCP] dial ATLAS error: 1.2.3.4:443 connect error: timeout\"}").is_none());
    }
}

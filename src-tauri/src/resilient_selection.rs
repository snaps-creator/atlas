//! Service-owned recovery. Workers only propose; the command thread commits.
use crate::{core::ApiClient, model::Settings};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::{HashMap, HashSet}, sync::mpsc, time::{Duration, Instant}};

const QUARANTINE: u64 = 300;
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
    let mut endpoints = HashSet::new();
    nodes.into_iter().filter(|n| {
        // The histories are only a ranking hint. Four new checks below are required.
        endpoints.insert(endpoint(n))
    }).take(4).filter_map(|n|n["name"].as_str().map(str::to_owned)).collect()
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
    last_line: Option<String>,
    blocked: HashMap<String,Quarantine>,
    network_epoch: Option<String>,
    pending: Option<Pending>,
    retry_at: Option<Instant>,
    needed: bool,
    scheduled: Option<Scheduled>,
    samples: HashMap<String,NodeSample>,
    last_active: Option<Instant>,
    last_reserves: Option<Instant>,
    last_pool: Option<Instant>,
    pool_cursor: usize,
    observed_current: Option<String>,
    current_since: Option<Instant>,
    improvement: Option<(String,u8)>,
}
impl Recovery {
    #[cfg(test)]
    pub(crate) fn propose_for_test(&mut self, value: Value) {
        let (tx,rx)=mpsc::channel();tx.send(value).unwrap();
        self.pending=Some(Pending{rx,generation:self.generation,repair_only:false});
    }
    #[cfg(test)]
    pub(crate) fn quarantined_count(&self) -> usize { self.blocked.len() }
    pub fn invalidate(&mut self) {
        self.generation += 1; self.pending = None; self.needed = false;
        self.scheduled = None; self.samples.clear(); self.last_active = None;
        self.last_reserves = None; self.last_pool = None; self.observed_current = None;
        self.current_since = None; self.improvement = None;
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
        if pending.purpose != "reserves" || !matches!(settings.selected.as_str(),"AUTO"|"FAILOVER") { return; }
        let Some(current) = self.samples.get(&pending.current).filter(|s|s.alive && s.checked_at.elapsed() < Duration::from_secs(90)) else { return; };
        let Some(current_delay) = current.primary_delay else { return; };
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
        if count < 3 || self.current_since.is_none_or(|since|since.elapsed() < Duration::from_secs(120)) { return; }
        let group = settings.selected.as_str();
        if client.api("PUT",&format!("/proxies/{group}"),Some(json!({"name":name}))).is_ok()
            && client.api("GET",&format!("/proxies/{group}"),None).is_ok_and(|v|v["now"] == name) {
            self.observed_current = Some(name.to_owned());
            self.current_since = Some(now);
            self.improvement = None;
            client.event(json!({"at":crate::model::now(),"kind":"automatic_optimization",
                "from":pending.current,"to":name,"improvementMs":current_delay-delay,
                "series":3,"heldSeconds":120}));
        }
    }
    fn schedule(&mut self, client: ApiClient, settings: &Settings, now: Instant) {
        if self.scheduled.is_some() || self.pending.is_some() || self.needed { return; }
        let automatic = matches!(settings.selected.as_str(),"AUTO"|"FAILOVER");
        let current = if automatic {
            client.api("GET",&format!("/proxies/{}",settings.selected),None).ok()
                .and_then(|v|v["now"].as_str().map(str::to_owned))
        } else { Some(settings.selected.clone()) };
        let Some(current) = current.filter(|name|settings.servers().iter().any(|n|n["name"] == *name)) else { return; };
        if self.observed_current.as_deref() != Some(&current) {
            self.observed_current = Some(current.clone());
            self.current_since = Some(now);
            self.improvement = None;
        }
        let nodes = settings.servers();
        let mut names = Vec::new();
        let purpose = if self.last_active.is_none_or(|at|at.elapsed() >= Duration::from_secs(30)) {
            self.last_active=Some(now); names.push(current.clone()); "active"
        } else if automatic && self.last_reserves.is_none_or(|at|at.elapsed() >= Duration::from_secs(60)) {
            self.last_reserves=Some(now);
            names.push(current.clone());
            let mut backups: Vec<_> = nodes.iter().filter_map(|n|n["name"].as_str())
                .filter(|name|*name != current && !self.blocked.contains_key(&nodes.iter()
                    .find(|n|n["name"] == *name).map(node_key).unwrap_or_default()))
                .map(str::to_owned).collect();
            backups.sort_by_key(|name|self.samples.get(name).and_then(|s|s.primary_delay).unwrap_or(u64::MAX));
            names.extend(backups.into_iter().take(2)); "reserves"
        } else if self.last_pool.is_none_or(|at|at.elapsed() >= Duration::from_secs(settings.auto_test_interval_seconds)) {
            self.last_pool=Some(now);
            let count=nodes.len();
            for offset in 0..count.min(6) {
                let node=&nodes[(self.pool_cursor+offset)%count];
                if let Some(name)=node["name"].as_str() {
                    if !self.blocked.contains_key(&node_key(node)) { names.push(name.to_owned()); }
                }
            }
            if count > 0 { self.pool_cursor=(self.pool_cursor+6)%count; }
            "pool"
        } else { return; };
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
                    for node in matching { self.blocked.entry(node_key(node)).or_insert(Quarantine {
                        probe_after: now + Duration::from_secs(QUARANTINE), failures: 1 }); }
                    self.needed = true;
                    client.event(json!({"at":crate::model::now(),"kind":"endpoint_quarantined","endpoint":peer,"retryAfterSeconds":QUARANTINE,"release":"two successful probe rounds","trigger":line}));
                }
            }
        }
        self.last_line = lines.last().cloned();
        self.poll_scheduled(&client,settings,now);
        let automatic = matches!(settings.selected.as_str(),"AUTO"|"FAILOVER")
            && settings.routing_mode != crate::model::RoutingMode::Direct;
        if !automatic { self.needed = false; }
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
                                let wait = (QUARANTINE.saturating_mul(1u64 << failure.failures.min(3))).min(3600);
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
                    let result = selected.map(|name|client.api("PUT",&format!("/proxies/{group}"),Some(json!({"name":name}))).and_then(|_| {
                        let state=client.api("GET","/proxies",None)?;
                        if state["proxies"][group]["now"] != name { return Err("Ядро не подтвердило выбранную альтернативу".into()); }
                        Ok(json!({"confirmedSelection":name}))
                    }));
                    if result.as_ref().is_some_and(|r|r.is_ok()) {
                        self.needed = false;
                    } else { self.needed = true; }
                    self.retry_at = Some(now + Duration::from_secs(30));
                    client.event(json!({"at":crate::model::now(),"kind":"recovery_result","evidence":report,
                        "selectorResult":result.map(|r|r.unwrap_or_else(|e|json!({"error":e}))),
                        "scope":"Four control responses prove tested paths; subsequent real traffic is monitored separately"}));
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
        if self.scheduled.is_some() || self.retry_at.is_some_and(|t|t>now) { return; }
        self.needed = false;
        self.retry_at = Some(now + Duration::from_secs(30));
        let (tx,rx) = mpsc::channel();
        let blocked = self.blocked.iter().filter(|(_,b)|b.probe_after > now).map(|(key,_)|key.clone()).collect();
        let settings = settings.clone();
        client.event(json!({"at":crate::model::now(),"kind":"recovery_started","quarantinedNodes":self.blocked.len()}));
        self.pending = Some(Pending{rx,generation:self.generation,repair_only:false});
        std::thread::spawn(move || { let _ = tx.send(verify(&client,&settings,&blocked,&crate::latency::ENDPOINTS)); });
    }
}

pub(crate) fn verify(client: &ApiClient, settings: &Settings, blocked: &HashSet<String>, controls: &[&str]) -> Value {
    let proxies = match client.api("GET","/proxies",None) { Ok(p)=>p,Err(e)=>return json!({"error":e}) };
    let names = candidates(settings,&proxies,blocked,crate::model::now());
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
    fn quarantine_is_bound_to_node_configuration() {
        let mut s=Settings::default();
        s.subscriptions.push(crate::model::Subscription{id:"x".into(),name:"x".into(),masked_url:"".into(),updated_at:0,error:None,
            servers:vec![json!({"name":"Sweden","server":"1.2.3.4","port":443}),json!({"name":"Germany","server":"1.2.3.4","port":443}),json!({"name":"other","server":"5.6.7.8","port":443})]});
        let p=json!({"proxies":{"Sweden":{"alive":true},"Germany":{"alive":true},"other":{"alive":true}}});
        let first = node_key(&s.servers()[0]);
        assert_eq!(candidates(&s,&p,&HashSet::from([first]),0),vec!["Germany","other"]);
        let both = s.servers()[..2].iter().map(node_key).collect();
        assert_eq!(candidates(&s,&p,&both,0),vec!["other"]);
        assert_eq!(failed_endpoint("[TCP] dial ATLAS error: 1.2.3.4:443 connect error: context deadline exceeded"),Some("1.2.3.4:443".into()));
        assert!(failed_endpoint("[TCP] dial DIRECT error: dns resolve failed").is_none());
        assert!(failed_endpoint("ATLAS_EVENT {\"trigger\":\"[TCP] dial ATLAS error: 1.2.3.4:443 connect error: timeout\"}").is_none());
    }
}

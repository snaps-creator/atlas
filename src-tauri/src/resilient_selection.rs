//! Service-owned recovery. Workers only propose; the command thread commits.
use crate::{core::ApiClient, model::Settings};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::{HashMap, HashSet}, sync::mpsc, time::{Duration, Instant}};

const QUARANTINE: u64 = 30;
const PROBE_TIMEOUT: u64 = 1500;
const SEARCH_WORKERS: usize = 6;
const ACTIVE_CHECK_INTERVAL: Duration = Duration::from_secs(10);
const SLOW_CONFIRMATIONS: u8 = 3;
fn probe_timeout(settings: &Settings) -> u64 {
    // A user's permitted latency must not exceed the probe's own deadline.
    PROBE_TIMEOUT.max(settings.auto_search_ping_ms.saturating_add(250)).min(10000)
}
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all="SCREAMING_SNAKE_CASE")]
enum Phase {
    #[default] Disconnected, ConnectedHealthy, Degraded, Scanning, Switching,
    Verifying, RefreshingSubscriptions, Rescanning, RecoveryFailed, WaitingRetry,
}
struct Verification { rx: mpsc::Receiver<bool>, generation: u64, candidate: String, previous: Option<String> }
fn remote_probe_failure(error: &str) -> bool {
    matches!(error,"Mihomo API: HTTP 503"|"Mihomo API: HTTP 504")
        || error.contains("Контрольный URL не подтвердил")
}
fn confirm_remote_failure<T>(first: Result<T, String>, confirmation: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    if first.as_ref().err().is_some_and(|error| remote_probe_failure(error)) {
        confirmation()
    } else { first }
}
fn confirmed_probe(client: &ApiClient, name: &str, timeout: u64, cancelled: Option<&std::sync::atomic::AtomicBool>) -> Result<Value, String> {
    confirmed_probe_at(client,name,timeout,cancelled,0)
}
fn confirmed_probe_at(client: &ApiClient, name: &str, timeout: u64, cancelled: Option<&std::sync::atomic::AtomicBool>, control: usize) -> Result<Value,String> {
    let run=|index:usize|crate::latency::verified_probe_with(client,name,crate::latency::ENDPOINTS[index],timeout,cancelled)
        .map(|mut v|{v["qualityControl"]=json!(index);v});
    confirm_remote_failure(run(control), || {
        let result=run(1-control);
        client.event(json!({"at":crate::model::now(),"kind":"active_node_confirmation",
            "name":name,"responded":result.is_ok(),"control":"https"}));
        result
    })
}
pub(crate) fn node_key(node: &Value) -> String {
    format!("{:x}", Sha256::digest(crate::model::repository::runtime_node(node).to_string().as_bytes()))
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
    ranked_candidates(settings,proxies,blocked,&HashMap::new())
}
fn ranked_candidates(settings: &Settings, proxies: &Value, blocked: &HashSet<String>, quality: &HashMap<String,crate::path_quality::History>) -> Vec<String> {
    let now=Instant::now();
    let mut nodes: Vec<_> = settings.servers().into_iter().filter(|n| !blocked.contains(&node_key(n))).collect();
    nodes.sort_by_cached_key(|n| {
        let p=&proxies["proxies"][n["name"].as_str().unwrap_or("")];
        (quality.get(&node_key(n)).map_or(0,|h|h.penalty(now,settings.auto_search_ping_ms)),p["alive"] != true,p["history"].as_array()
            .and_then(|h|h.last()).and_then(|v|v["delay"].as_u64()).filter(|d|*d > 0).unwrap_or(u64::MAX))
    });
    // Credentials/transport are part of identity; display aliases are not.
    let mut identities=HashSet::new();
    let nodes:Vec<_>=nodes.into_iter().filter(|n| { let mut config=(*n).clone();
        if let Some(object)=config.as_object_mut() { object.remove("name"); }
        identities.insert(node_key(&config))
    }).collect();
    // Round-robin source/transport families within each evidence tier. A large
    // subscription cannot fill all six workers with the same kind of path.
    // Subscription IDs identify configurations, not proven independent networks.
    let sources:HashMap<_,_>=settings.subscriptions.iter().flat_map(|s|s.servers.iter()
        .filter_map(move |n|n["name"].as_str().map(|name|(name,s.id.as_str())))).collect();
    let mut seen:HashMap<(u8,bool,String),usize>=HashMap::new();
    let mut ordered:Vec<_>=nodes.into_iter().enumerate().map(|(index,n)| {
        let name=n["name"].as_str().unwrap_or("");
        let stream=&n["xray"]["outbounds"][0]["streamSettings"];
        let family=format!("{}|{}|{}|{}",sources.get(name).copied().unwrap_or(""),n["type"],stream["network"],stream["security"]);
        let penalty=quality.get(&node_key(&n)).map_or(0,|h|h.penalty(now,settings.auto_search_ping_ms));
        let unavailable=proxies["proxies"][name]["alive"]!=true;
        let count=seen.entry((penalty,unavailable,family)).or_default();let round=*count;*count+=1;
        ((penalty,unavailable,round,index),name.to_owned())
    }).collect();
    ordered.sort_by_key(|(rank,_)|*rank);ordered.into_iter().map(|(_,name)|name).collect()
}

struct Pending { rx: mpsc::Receiver<Value>, generation: u64, repair_only: bool }
struct Quarantine { probe_after: Instant, failures: u32 }
struct NodeSample { checked_at: Instant, primary_delay: Option<u64>, alive: bool }
struct Scheduled {
    rx: mpsc::Receiver<Vec<(String, Option<u64>, bool, usize)>>,
    generation: u64,
}
#[derive(Default)]
pub(crate) struct Recovery {
    runtime_settings: Option<std::sync::Arc<Settings>>,
    phase: Phase,
    operation: Option<String>,
    started: Option<Instant>,
    verification: Option<Verification>,
    refreshed: bool,
    refresh_success: bool,
    refresh_after: Option<Instant>,
    refresh_deadline: Option<Instant>,
    refresh_ticket: Option<String>,
    failures: u32,
    notified: bool,
    last_switch: Option<Instant>,
    attempted: HashSet<String>,
    generation: u64,
    route_revision: u64,
    last_line: Option<String>,
    last_environment_check: Option<Instant>,
    blocked: HashMap<String,Quarantine>,
    pending: Option<Pending>,
    retry_at: Option<Instant>,
    needed: bool,
    samples: HashMap<String,NodeSample>,
    quality: HashMap<String,crate::path_quality::History>,
    monitor_sequence: usize,
    last_active: Option<Instant>,
    active: Option<Scheduled>,
    slow_samples: u8,
    observed_current: Option<String>,
    current_since: Option<Instant>,
    improvement: Option<(String,u8)>,
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl Recovery {
    pub(crate) fn status(&self) -> Value { json!({"phase":self.phase,"operationId":self.operation,
        "configurationVersion":self.generation,"subscriptionsUpdated":self.refresh_success,
        "failureExhausted":self.notified,
        "refreshTicket":self.refresh_ticket,
        "active":self.observed_current.as_ref().and_then(|name|self.samples.get(name)).map(|s|
            json!({"delay":s.primary_delay,"alive":s.alive,"ageMs":s.checked_at.elapsed().as_millis(),"measurement":"http_via_node_ms"}))}) }
    fn transition(&mut self, client: &ApiClient, phase: Phase, kind: &str) {
        self.phase=phase;
        client.event(json!({"at":crate::model::now(),"kind":kind,"phase":phase,
            "operationId":self.operation,"configurationVersion":self.generation,
            "elapsedMs":self.started.map(|at|at.elapsed().as_millis())}));
    }
    pub(crate) fn refresh_finished(&mut self, id: &str, success: bool, client: &ApiClient) -> bool {
        if self.refresh_ticket.as_deref()!=Some(id) || self.phase!=Phase::RefreshingSubscriptions { return false; }
        self.refresh_ticket=None;
        self.refresh_success=success; self.refresh_deadline=None;
        self.needed=true; self.retry_at=None;
        self.transition(client,Phase::Rescanning,"SubscriptionRefreshCompleted"); true
    }
    /// Subscription commit invalidates samples but preserves the owning incident.
    pub(crate) fn configuration_changed(&mut self) {
        if self.phase!=Phase::RefreshingSubscriptions { self.invalidate(); return; }
        self.cancelled.store(true,std::sync::atomic::Ordering::SeqCst);
        self.cancelled=Default::default(); self.generation+=1;
        self.runtime_settings=None;
        self.pending=None; self.active=None; self.verification=None;
        self.samples.clear(); self.quality.clear(); self.monitor_sequence=0; self.blocked.clear(); self.last_active=None;
        self.attempted.clear();
    }
    fn exhausted(&mut self, client: &ApiClient, now: Instant, policy: &crate::model::RecoveryPolicy) {
        if !self.refreshed && self.refresh_after.is_none_or(|at|now>=at) {
            self.refreshed=true; self.refresh_success=false;
            self.refresh_ticket=Some(uuid::Uuid::new_v4().to_string());
            self.refresh_after=Some(now+Duration::from_secs(policy.refresh_interval_seconds));
            self.refresh_deadline=Some(now+Duration::from_secs(180));
            self.needed=false;
            self.transition(client,Phase::RefreshingSubscriptions,"SubscriptionRefreshStarted");
        } else {
            self.transition(client,Phase::RecoveryFailed,"RecoveryFailed");
            if !self.notified {
                client.event(json!({"at":crate::model::now(),"kind":"RecoveryNotification",
                    "operationId":self.operation,"subscriptionsUpdated":self.refresh_success}));
                self.notified=true;
            }
            self.failures=self.failures.saturating_add(1);
            self.retry_at=Some(now+Duration::from_secs(policy.retry_delay(self.failures)));
            self.needed=true;
            self.transition(client,Phase::WaitingRetry,"RecoveryRetryScheduled");
        }
    }
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
        self.runtime_settings=None;
        self.cancelled.store(true,std::sync::atomic::Ordering::SeqCst);
        self.cancelled=Default::default();
        self.generation += 1; self.pending = None; self.needed = false; self.retry_at = None;
        self.samples.clear(); self.quality.clear(); self.monitor_sequence=0; self.last_active = None;
        self.observed_current = None;
        self.current_since = None; self.improvement = None; self.active = None; self.slow_samples = 0;
        self.verification=None; self.operation=None; self.phase=Phase::Disconnected;
        self.attempted.clear();
        self.refreshed=false; self.refresh_deadline=None; self.failures=0; self.notified=false;
        self.refresh_ticket=None;
    }
    fn poll_active(&mut self, client: &ApiClient, settings: &Settings, now: Instant) {
        let Some(job) = &self.active else { return; };
        let result = job.rx.try_recv();
        if matches!(result, Err(mpsc::TryRecvError::Empty)) { return; }
        let job = self.active.take().unwrap();
        if job.generation != self.generation { return; }
        let Ok(results) = result else { return; };
        for (name, delay, alive, control) in results {
            if self.observed_current.as_ref() != Some(&name) {
                self.observed_current = Some(name.clone()); self.current_since = Some(now);
                self.slow_samples = 0; self.improvement = None;
            }
            let key=settings.servers().iter().find(|n|n["name"]==name).map(node_key).unwrap_or_else(||name.clone());
            let history=self.quality.entry(key).or_default();history.record(now,control,delay,alive);
            self.slow_samples=if alive {history.slow_streak(now,settings.auto_search_ping_ms)} else {0};
            let quality=history.summary(now);
            if self.slow_samples == 0 {
                self.improvement = None;
            }
            if !alive || (settings.auto_optimize && self.slow_samples >= SLOW_CONFIRMATIONS) {
                // Search immediately; only a verified replacement may change the route.
                self.needed = true;
                if self.operation.is_none() { self.phase=Phase::Degraded; }
                if self.operation.is_none() || (!alive && self.samples.get(&name).is_some_and(|s|s.alive)) { self.retry_at = None; }
                // Do not wait for a second diagnosis of the failed node.
                if let Some(node) = settings.servers().iter().find(|n|!alive && n["name"] == name) {
                    self.blocked.entry(node_key(node)).or_insert(Quarantine {
                        probe_after: now + Duration::from_secs(60), failures: 0 });
                }
            }
            client.event(json!({"at":crate::model::now(),"kind":"active_node_sample",
                "name":name,"delay":delay,"responded":alive,"slowSamples":self.slow_samples,"quality":quality,"control":control,
                "event":if !alive && delay.is_some() {"TUNNEL_FAILURE"} else if !alive {"TIMEOUT"} else if self.slow_samples>0 {"HIGH_LATENCY"} else {"HEALTHY"},
                "measurement":"http_via_node_ms","configurationVersion":self.generation,"operationId":self.operation}));
            self.samples.insert(name, NodeSample {checked_at:now,primary_delay:delay,alive});
            if alive && self.slow_samples==0 && self.operation.is_none() { self.phase=Phase::ConnectedHealthy; }
        }
    }
    fn monitor(&mut self, client: ApiClient, settings: &Settings, now: Instant) {
        if self.active.is_some() || self.last_active.is_some_and(|at|now.duration_since(at)<ACTIVE_CHECK_INTERVAL) { return; }
        self.last_active = Some(now);
        let control=self.monitor_sequence%2;self.monitor_sequence=self.monitor_sequence.wrapping_add(1);
        let (tx,rx)=mpsc::channel();
        self.active=Some(Scheduled{rx,generation:self.generation});
        let settings=settings.clone();
        let timeout=probe_timeout(&settings);
        let cancelled=self.cancelled.clone();
        let generation=self.generation;
        let operation=self.operation.clone();
        std::thread::spawn(move || {
            let name = if matches!(settings.selected.as_str(),"AUTO"|"FAILOVER") {
                match client.api("GET",&format!("/proxies/{}",settings.selected),None) {
                    Ok(v) => v["now"].as_str().map(str::to_owned),
                    Err(e) => { client.event(json!({"at":crate::model::now(),"kind":"active_monitor_error","error":e})); None }
                }
            } else { Some(settings.selected) };
            let results = name.into_iter().filter_map(|name| {
                let probe=confirmed_probe_at(&client,&name,timeout,Some(&cancelled),control);
                if let Err(error)=&probe {
                    if !remote_probe_failure(error) {
                        client.event(json!({"at":crate::model::now(),"kind":"active_monitor_error","event":"TRANSPORT_FAILURE",
                            "configurationVersion":generation,"operationId":operation,"error":error}));
                        return None;
                    }
                }
                let delay=probe.as_ref().ok().and_then(|v|v["delay"].as_u64());
                let traffic=probe.is_ok() && client.tunnel_responds(timeout);
                if probe.is_ok() && !traffic {
                    client.event(json!({"at":crate::model::now(),"kind":"ActiveConnectionDegraded","event":"TUNNEL_FAILURE",
                        "configurationVersion":generation,"operationId":operation,"reason":"Live proxy listener did not pass HTTP 204"}));
                }
                let measured_control=probe.as_ref().ok().and_then(|v|v["qualityControl"].as_u64()).unwrap_or(control as u64) as usize;
                Some((name,delay,traffic,measured_control))
            }).collect();
            let _=tx.send(results);
        });
    }
    pub fn tick(&mut self, client: ApiClient, settings: &Settings) {
        // Expand once per configuration generation, never on the packet path.
        let runtime=self.runtime_settings.get_or_insert_with(||
            std::sync::Arc::new(crate::transport_guard::runtime_settings(settings))).clone();
        let settings=runtime.as_ref();
        let now = Instant::now();
        // HealthState owns the debounced asynchronous network observer.
        // A second synchronous Windows query here would block pipe admission
        // and invalidate evidence before that observer confirms an epoch.
        let servers = settings.servers();
        if self.last_environment_check.is_none_or(|at|now.duration_since(at)>=Duration::from_secs(1)) {
        self.last_environment_check=Some(now);
        self.blocked.retain(|key,_| servers.iter().any(|n|node_key(n)==*key));
        let lines = client.logs().unwrap_or_default();
        let fresh = self.last_line.as_ref().and_then(|last|lines.iter().rposition(|l|l == last)).map_or(0,|i|i+1);
        for line in &lines[fresh..] {
            if let Some(peer) = failed_endpoint(line) {
                let matching: Vec<_> = servers.iter().filter(|n|endpoint(n) == peer).collect();
                if !matching.is_empty() {
                    if let Some(name)=self.observed_current.as_deref().filter(|name|matching.iter().any(|n|n["name"]==*name)) {
                        client.connection_failed(name,if line.contains("[UDP]") {"udp"} else if line.contains("[TCP]") {"tcp"} else {"unknown"});
                        // Request fresh evidence on the next existing monitor tick.
                        self.last_active=None;
                    }
                    // A single application connection error is not a failed
                    // health check, nor evidence against every node at this IP.
                    // The active monitor confirms failure before replacement.
                    client.event(json!({"at":crate::model::now(),"kind":"endpoint_connection_error",
                        "endpoint":peer,"matchingNodes":matching.len(),"trigger":line}));
                }
            }
        }
        self.last_line = lines.last().cloned();
        }
        if self.phase==Phase::RefreshingSubscriptions {
            if self.refresh_deadline.is_some_and(|at|now>=at) {
                let id=self.refresh_ticket.clone().unwrap_or_default();
                self.refresh_finished(&id,false,&client);
            } else { return; }
        }
        if let Some(job)=&self.verification {
            let result=job.rx.try_recv();
            if matches!(result,Err(mpsc::TryRecvError::Empty)) { return; }
            let job=self.verification.take().unwrap();
            if job.generation!=self.generation { return; }
            if result.unwrap_or(false) {
                self.needed=false; self.retry_at=None; self.failures=0; self.notified=false;
                self.observed_current=Some(job.candidate); self.last_switch=Some(now);
                self.current_since=Some(now); self.last_active=None; self.active=None;
                self.slow_samples=0; self.refreshed=false;
                self.transition(&client,Phase::ConnectedHealthy,"SwitchSucceeded");
                self.transition(&client,Phase::ConnectedHealthy,"RecoverySucceeded");
                self.operation=None;
            } else {
                if let Some(node)=settings.servers().iter().find(|n|n["name"]==job.candidate) {
                    self.blocked.insert(node_key(node),Quarantine{probe_after:now+Duration::from_secs(60),failures:1});
                }
                // Restore the previous selector only; health stays degraded until measured.
                if let Some(previous)=job.previous {
                    self.route_revision=self.route_revision.wrapping_add(1);
                    let _=client.api("PUT",&format!("/proxies/{}",settings.selected),Some(json!({"name":previous})));
                }
                self.needed=true; self.retry_at=None;
                self.transition(&client,Phase::Degraded,"SwitchFailed");
            }
        }
        self.poll_active(&client,settings,now);
        // All selection decisions now pass through the recovery operation.
        if self.verification.is_none() { self.monitor(client.clone(),settings,now); }
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
                            if test["localError"]!=true {
                                for evidence in test["controls"].as_array().into_iter().flatten() {
                                    if let (Some(control),Some(delay))=(evidence["qualityControl"].as_u64(),evidence["delay"].as_u64()) {
                                        self.quality.entry(key.clone()).or_default().record(now,control as usize,Some(delay),true);
                                    }
                                }
                            }
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
                    if report["error"].is_string() {
                        self.needed=true;self.retry_at=Some(now+Duration::from_secs(5));
                        self.transition(&client,Phase::WaitingRetry,"ProbeCoordinatorUnavailable");
                        return;
                    }
                    let selected = report["candidate"].as_str().filter(|name| settings.servers().iter().any(|n|
                        n["name"] == *name && !self.blocked.contains_key(&node_key(n))));
                    let group = settings.selected.as_str();
                    if selected.is_some() { self.route_revision=self.route_revision.wrapping_add(1); }
                    if selected.is_some() { self.active=None; self.last_active=None; }
                    if selected.is_some() { self.transition(&client,Phase::Switching,"CandidateSelected"); }
                    let result = selected.map(|name|client.api("PUT",&format!("/proxies/{group}"),Some(json!({"name":name}))).and_then(|_| {
                        let state=client.api("GET","/proxies",None)?;
                        if state["proxies"][group]["now"] != name { return Err("Ядро не подтвердило выбранную альтернативу".into()); }
                        Ok(json!({"confirmedSelection":name}))
                    }));
                    if result.as_ref().is_some_and(|r|r.is_ok()) {
                        let candidate=selected.unwrap().to_owned();
                        self.attempted.insert(candidate.clone());
                        let (tx,rx)=mpsc::channel();
                        self.verification=Some(Verification{rx,generation:self.generation,candidate:candidate.clone(),previous:self.observed_current.clone()});
                        self.transition(&client,Phase::Verifying,"SwitchStarted");
                        let (client,cancelled)=(client.clone(),self.cancelled.clone());
                        let group=settings.selected.clone();
                        let timeout=probe_timeout(settings);
                        let threshold=settings.auto_search_ping_ms;
                        std::thread::spawn(move || {
                            let ok=confirmed_probe(&client,&candidate,timeout,Some(&cancelled)).is_ok_and(|v|v["delay"].as_u64().is_some_and(|delay|delay<=threshold))
                                && client.tunnel_responds(timeout)
                                && client.api_cancellable("GET",&format!("/proxies/{group}"),None,Some(&cancelled)).is_ok_and(|v|v["now"]==candidate);
                            let _=tx.send(ok);
                        });
                    } else { self.exhausted(&client,now,&settings.recovery_policy); }
                    client.event(json!({"at":crate::model::now(),"kind":"recovery_result","evidence":report,
                        "operationId":self.operation,"configurationVersion":self.generation,
                        "selectorResult":result.map(|r|r.unwrap_or_else(|e|json!({"error":e}))),
                        "scope":"Replacement answered the control URL; old-node diagnosis is deferred"}));
                }
                Err(mpsc::TryRecvError::Disconnected) => { self.pending = None; self.needed = true; self.retry_at = Some(now + Duration::from_secs(30)); }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.pending.is_some() || self.verification.is_some() || self.phase==Phase::RefreshingSubscriptions { return; }
        // The application-owned minute refresh covers every node, including
        // quarantined nodes. Recovery only adds work when the active path needs it.
        if !self.needed { return; }
        if self.retry_at.is_some_and(|t|t>now) { return; }
        // Cooldown suppresses ping oscillation, never a confirmed dead path.
        if self.slow_samples>0 && self.last_switch.is_some_and(|at|now.duration_since(at)<Duration::from_secs(settings.recovery_policy.switch_cooldown_seconds)) { return; }
        if self.operation.is_none() {
            self.refresh_success=false;
            self.operation=Some(uuid::Uuid::new_v4().to_string()); self.started=Some(now);
            self.transition(&client,Phase::Degraded,"ActiveConnectionDegraded");
        }
        if self.phase==Phase::WaitingRetry { self.refreshed=false; self.attempted.clear(); }
        self.transition(&client,Phase::Scanning,"RecoveryStarted");
        self.needed = false;
        self.retry_at = Some(now + Duration::from_secs(30));
        let (tx,rx) = mpsc::channel();
        let mut blocked: HashSet<_> = self.blocked.iter().filter(|(_,b)|b.probe_after > now).map(|(key,_)|key.clone()).collect();
        blocked.extend(servers.iter().filter(|n|n["name"].as_str().is_some_and(|name|self.attempted.contains(name))).map(|n|node_key(n)));
        if let Some(node)=servers.iter().find(|n|n["name"].as_str()==self.observed_current.as_deref()) {
            blocked.insert(node_key(node));
        }
        let settings = settings.clone();
        client.event(json!({"at":crate::model::now(),"kind":"recovery_started","quarantinedNodes":self.blocked.len()}));
        self.pending = Some(Pending{rx,generation:self.generation,repair_only:false});
        let cancelled=self.cancelled.clone();
        let previous=self.observed_current.clone().filter(|name|!self.attempted.contains(name));
        let quality=self.quality.clone();
        std::thread::spawn(move || { let _ = tx.send(verify_replacement_with_quality(&client,&settings,&blocked,previous.as_deref(),cancelled,&quality)); });
    }
}

// Return the first freshly verified replacement, without waiting for a slow
// peer or diagnosing the old node. The shared admission gate caps network work.
#[cfg(test)]
fn verify_replacement(client: &ApiClient, settings: &Settings, blocked: &HashSet<String>, previous: Option<&str>, cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>) -> Value {
    verify_replacement_with_quality(client,settings,blocked,previous,cancelled,&HashMap::new())
}
fn verify_replacement_with_quality(client: &ApiClient, settings: &Settings, blocked: &HashSet<String>, previous: Option<&str>, cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>, quality: &HashMap<String,crate::path_quality::History>) -> Value {
    use std::sync::atomic::Ordering;
    let proxies = match client.api("GET","/proxies",None) { Ok(v)=>v,Err(e)=>return json!({"error":e}) };
    let variants: HashSet<_>=settings.servers().iter().filter(|n|crate::transport_guard::is_variant(n))
        .filter_map(|n|n["name"].as_str().map(str::to_owned)).collect();
    let (protected,ordinary): (Vec<_>,Vec<_>)=ranked_candidates(settings,&proxies,blocked,quality)
        .into_iter().partition(|name|variants.contains(name));
    let mut results=Vec::new();
    for (names,fragmented) in [(ordinary,false),(protected,true)] {
        if cancelled.load(Ordering::SeqCst) {return json!({"error":"CANCELLED","results":results});}
        if names.is_empty() {continue;}
        if fragmented {client.event(json!({"kind":"TransportFallbackStarted","at":crate::model::now(),
            "strategy":"tlshello_fragment","candidates":names.len(),"reason":"ordinary candidates did not pass"}));}
        let report=search_replacements(client,settings,names,cancelled.clone());
        results.extend(report["results"].as_array().into_iter().flatten().cloned());
        if report["error"].is_string() || report["candidate"].is_string() {
            let mut report=report;report["results"]=json!(results);report["transportFallback"]=json!(fragmented);return report;
        }
    }
    // Only after exhausting replacements, recheck the old path. A recovered
    // original node must not leave the client searching forever.
    if !cancelled.load(Ordering::SeqCst) {
        if let Some(name)=previous {
            if let Ok(result)=confirmed_probe(client,name,probe_timeout(settings),Some(&cancelled)) {
                if !result["delay"].as_u64().is_some_and(|d|d<=settings.auto_search_ping_ms) { return json!({"candidate":null,"results":results}); }
                results.push(json!({"name":name,"passed":true,"result":result}));
                return json!({"candidate":name,"results":results});
            }
        }
    }
    json!({"candidate":null,"results":results})
}

fn search_replacements(client: &ApiClient, settings: &Settings, names: Vec<String>, cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>) -> Value {
    use std::sync::{Arc, atomic::{AtomicBool, AtomicUsize, Ordering}};
    let names=Arc::new(names);
    let next=Arc::new(AtomicUsize::new(0));
    let stop=Arc::new(AtomicBool::new(false));
    let (tx,rx)=mpsc::sync_channel(SEARCH_WORKERS);
    let threshold=settings.auto_search_ping_ms;
    let timeout=probe_timeout(settings);
    let deadline=Instant::now()+Duration::from_millis((names.len().div_ceil(SEARCH_WORKERS) as u64*(2*(timeout+500))).max(4000));
    for _ in 0..names.len().min(SEARCH_WORKERS) {
        let (names,next,stop,tx,client,cancelled)=(names.clone(),next.clone(),stop.clone(),tx.clone(),client.clone(),cancelled.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) && !cancelled.load(Ordering::SeqCst) && Instant::now()<deadline {
                let Some(name)=names.get(next.fetch_add(1,Ordering::SeqCst)) else { break; };
                let mut result=confirmed_probe(&client,name,timeout,Some(&cancelled));
                let mut controls=Vec::new();if let Ok(v)=&result {controls.push(v.clone());}
                // A single lucky reply is insufficient to displace a live path.
                if result.as_ref().is_ok_and(|v|v["delay"].as_u64().is_some_and(|d|d<=threshold)) {
                    if stop.load(Ordering::SeqCst) || cancelled.load(Ordering::SeqCst) {break;}
                    result=confirmed_probe_at(&client,name,timeout,Some(&cancelled),1);
                    if let Ok(v)=&result {controls.push(v.clone());}
                }
                if cancelled.load(Ordering::SeqCst) {break;}
                let passed=result.as_ref().is_ok_and(|v|v["delay"].as_u64().is_some_and(|d|d<=threshold));
                let remote_failure=result.as_ref().err().is_some_and(|e|remote_probe_failure(e));
                let local_error=result.is_err() && !remote_failure;
                if passed { stop.store(true,Ordering::SeqCst); }
                if tx.send(json!({"name":name,"localError":local_error,"controls":controls,"passed":if passed {Some(true)} else if remote_failure {Some(false)} else {None},"result":result.unwrap_or_else(|e|json!({"error":e}))})).is_err() { break; }
                if local_error { break; }
            }
        });
    }
    drop(tx);
    let mut results=Vec::new();
    while let Ok(result)=rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        let candidate=result["name"].clone();
        let passed=result["passed"]==true;
        results.push(result);
        if passed { return json!({"candidate":candidate,"results":results}); }
    }
    stop.store(true,Ordering::SeqCst);
    if results.len()<names.len() || results.iter().any(|r|r["localError"]==true) {
        return json!({"error":"Проверка кандидатов не завершена из-за ошибки контроллера","results":results});
    }
    json!({"candidate":null,"results":results})
}

impl Drop for Recovery {
    fn drop(&mut self) { self.cancelled.store(true,std::sync::atomic::Ordering::SeqCst); }
}
#[cfg(test)]
#[path="recovery_tests.rs"]
mod scenarios;

pub(crate) fn verify(client: &ApiClient, settings: &Settings, blocked: &HashSet<String>, controls: &[&str]) -> Value {
    verify_progressive(client,settings,blocked,controls,None)
}
fn startup_identity(name: &str) -> String {format!("{:x}",Sha256::digest(name.as_bytes()))}
fn unseen_startup_window(names: &[String], attempted: &mut HashSet<String>, limit: usize) -> Vec<String> {
    if names.iter().all(|name|attempted.contains(&startup_identity(name))) {attempted.clear();}
    names.iter().filter(|name|!attempted.contains(&startup_identity(name))).take(limit).cloned().collect()
}
pub(crate) fn verify_progressive(client: &ApiClient, settings: &Settings, blocked: &HashSet<String>, controls: &[&str], cursor: Option<&std::path::Path>) -> Value {
    let proxies = match client.api("GET","/proxies",None) { Ok(p)=>p,Err(e)=>return json!({"error":e}) };
    let variants: HashSet<_>=settings.servers().iter().filter(|n|crate::transport_guard::is_variant(n))
        .filter_map(|n|n["name"].as_str().map(str::to_owned)).collect();
    let (protected,ordinary): (Vec<_>,Vec<_>)=candidates(settings,&proxies,blocked,crate::model::now())
        .into_iter().partition(|n|variants.contains(n));
    let mut inventory:Vec<_>=settings.servers().iter().map(|n|node_key(n)).collect();inventory.sort();
    let fingerprint=format!("{:x}",Sha256::digest(inventory.join("|").as_bytes()));
    let saved=cursor.and_then(|p|std::fs::read_to_string(p).ok()).and_then(|s|serde_json::from_str::<Value>(&s).ok()).unwrap_or(Value::Null);
    let load_attempted=|field:&str| -> HashSet<String> {
        if saved["inventory"]!=fingerprint {return HashSet::new();}
        saved[field].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect()
    };
    let mut attempted=load_attempted("attempted");let mut variant_attempted=load_attempted("variantAttempted");
    let names=unseen_startup_window(&ordinary,&mut attempted,8);
    let offset=attempted.len();
    let mut report=verify_names(client,&names,controls);
    let tested=report["results"].as_array().map_or(0,Vec::len);
    for name in names.iter().take(tested) {attempted.insert(startup_identity(name));}
    if !report["candidate"].is_string() && !protected.is_empty() {
        let variants=unseen_startup_window(&protected,&mut variant_attempted,4);
        let mut fallback=verify_names(client,&variants,controls);
        for name in variants.iter().take(fallback["results"].as_array().map_or(0,Vec::len)) {variant_attempted.insert(startup_identity(name));}
        report["results"].as_array_mut().unwrap().append(fallback["results"].as_array_mut().unwrap());
        report["candidate"]=fallback["candidate"].clone();report["transportFallback"]=json!(true);
    }
    if report["candidate"].is_string() {attempted.clear();variant_attempted.clear();}
    let next=attempted.len();
    report["search"]=json!({"ordinaryTotal":ordinary.len(),"offset":offset,"ordinaryTested":tested,"nextOffset":next,
        "meaning":"Bounded startup search continues on the next connection attempt; a failed window does not condemn the entire catalog."});
    if let Some(path)=cursor {
        if let Err(error)=std::fs::write(path,json!({"inventory":fingerprint,"offset":next,"attempted":attempted,"variantAttempted":variant_attempted}).to_string()) {report["cursorSaveError"]=json!(error.to_string());}
    }
    client.event(json!({"kind":"startup_candidate_search","at":crate::model::now(),"report":report}));
    report
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
    fn startup_search_reaches_ninth_node_after_a_fresh_service_client() {
        use std::{io::{Read,Write},net::TcpListener,sync::{Arc,atomic::{AtomicBool,Ordering}}};
        let listener=TcpListener::bind("127.0.0.1:0").unwrap();let port=listener.local_addr().unwrap().port();listener.set_nonblocking(true).unwrap();
        let stop=Arc::new(AtomicBool::new(false));let stopped=stop.clone();
        let worker=std::thread::spawn(move || {
            while !stopped.load(Ordering::SeqCst) {
                if let Ok((mut socket,_))=listener.accept() {
                    std::thread::spawn(move || {
                        socket.set_read_timeout(Some(Duration::from_secs(3))).unwrap();let mut request=[0;4096];let size=socket.read(&mut request).unwrap_or(0);let request=String::from_utf8_lossy(&request[..size]);
                        let failed=request.contains("/delay?") && !request.contains("/node-08/delay?");
                        let body=if request.contains("/delay?") {json!({"delay":40})} else {
                            let proxies:serde_json::Map<_,_>=(0..12).map(|index| {
                                let mut node=json!({"type":"Socks5","alive":false,"extra":{}});
                                for endpoint in crate::latency::ENDPOINTS {node["extra"][format!("{endpoint}?atlas_probe=1")]=json!({"alive":true});}
                                (format!("node-{index:02}"),node)
                            }).collect();json!({"proxies":proxies})
                        }.to_string();
                        let status=if failed {504}else{200};let _=write!(socket,"HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body);
                    });
                } else {std::thread::sleep(Duration::from_millis(2));}
            }
        });
        let mut settings=Settings::default();settings.subscriptions.push(crate::model::Subscription{id:"fixture".into(),name:"fixture".into(),masked_url:String::new(),updated_at:0,error:None,source:Default::default(),options:Default::default(),
            servers:(0..12).map(|i|json!({"name":format!("node-{i:02}"),"type":"socks5","server":"127.0.0.1","port":i+1})).collect()});
        let directory=std::env::temp_dir().join(format!("atlas-startup-search-{}",uuid::Uuid::new_v4()));std::fs::create_dir_all(&directory).unwrap();let cursor=directory.join("cursor.json");
        let first=verify_progressive(&ApiClient::loopback_fixture(port).with_probe_settings(&settings),&settings,&HashSet::new(),&crate::latency::ENDPOINTS,Some(&cursor));
        // Recreate all in-memory health state, just as a stopped service does.
        let second=verify_progressive(&ApiClient::loopback_fixture(port).with_probe_settings(&settings),&settings,&HashSet::new(),&crate::latency::ENDPOINTS,Some(&cursor));
        stop.store(true,Ordering::SeqCst);worker.join().unwrap();std::fs::remove_dir_all(directory).unwrap();
        assert!(!first["candidate"].is_string());assert_eq!(first["search"]["ordinaryTested"],8);
        assert_eq!(second["candidate"],"node-08");assert_eq!(second["search"]["offset"],8);
    }
    #[test]
    fn startup_retries_reach_a_working_node_beyond_the_first_window() {
        let names:Vec<_>=(0..25).map(|n|format!("node-{n}")).collect();
        let mut visited=HashSet::new();let mut attempted=HashSet::new();
        for _ in 0..4 {let batch=unseen_startup_window(&names,&mut attempted,8);visited.extend(batch.clone());attempted.extend(batch.iter().map(|name|startup_identity(name)));}
        assert_eq!(visited.len(),25);
        assert_eq!(unseen_startup_window(&[],&mut HashSet::new(),8),Vec::<String>::new());
        assert_eq!(unseen_startup_window(&["one".into()],&mut HashSet::new(),8),vec!["one"]);
        let mut attempted=HashSet::new();let first=unseen_startup_window(&names,&mut attempted,8);
        attempted.extend(first.iter().map(|name|startup_identity(name)));
        let reordered=names.iter().rev().cloned().collect::<Vec<_>>();
        let next=unseen_startup_window(&reordered,&mut attempted,8);
        assert!(next.iter().all(|name|!first.contains(name)),"Changing health ranks must not skip untested nodes");
    }
    #[test]
    fn confirmed_observer_epoch_discards_quarantine_and_stale_jobs() {
        let now=Instant::now();
        let mut recovery=Recovery::default();
        recovery.blocked.insert("old-node".into(),Quarantine{probe_after:now,failures:1});
        recovery.samples.insert("old-node".into(),NodeSample{checked_at:now,primary_delay:None,alive:false});
        recovery.quality.entry("old-node".into()).or_default().record(now,0,None,false);
        recovery.propose_for_test(json!({"candidate":"old-node"}));
        let previous_epoch=recovery.path_epoch();
        let cancelled=recovery.cancelled.clone();
        recovery.network_changed();
        assert!(cancelled.load(std::sync::atomic::Ordering::SeqCst));
        assert!(recovery.blocked.is_empty() && recovery.samples.is_empty());
        assert!(recovery.quality.is_empty());
        assert!(recovery.pending.is_none() && recovery.active.is_none());
        assert_ne!(recovery.path_epoch(),previous_epoch);
        assert_eq!(recovery.route_revision,0);
    }
    #[test]
    fn disabled_optimization_keeps_a_responding_active_server() {
        let settings=Settings::default();
        assert!(!settings.auto_optimize);
        let now=Instant::now();
        let mut recovery=Recovery::default();
        recovery.observed_current=Some("active".into());
        recovery.current_since=Some(now-Duration::from_secs(120));
        recovery.slow_samples=3;
        recovery.improvement=Some(("faster".into(),1));
        recovery.samples.insert("active".into(),NodeSample{checked_at:now,primary_delay:Some(500),alive:true});
        let (tx,rx)=mpsc::channel();
        tx.send(vec![("active".into(),Some(500),true,0)]).unwrap();
        recovery.active=Some(Scheduled{rx,generation:0});
        let core=crate::core::Core::new(Default::default(),Default::default());
        recovery.poll_active(&core.client(),&settings,now);
        assert_eq!(recovery.observed_current.as_deref(),Some("active"));
        assert_eq!(recovery.route_revision,0);
        assert!(!recovery.needed);
    }
    #[test]
    fn http_failure_with_working_https_is_not_a_dead_node() {
        assert_eq!(confirm_remote_failure(Err("Mihomo API: HTTP 504".into()), || Ok(98)),Ok(98));
        assert_eq!(confirm_remote_failure(Err("Mihomo API: HTTP 504".into()), || Err::<u64,_>("Mihomo API: HTTP 504".into())),Err("Mihomo API: HTTP 504".into()));
        assert_eq!(confirm_remote_failure(Ok(60), || panic!("healthy path needs no retry")),Ok(60));
        assert_eq!(confirm_remote_failure(Err::<u64,_>("controller unavailable".into()), || panic!("local errors must not probe remote nodes")),Err("controller unavailable".into()));
    }
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
                            let mut health=json!({"alive":true,"extra":{}});
                            for endpoint in crate::latency::ENDPOINTS {health["extra"][format!("{endpoint}?atlas_probe=1")]=json!({"alive":true});}
                            json!({"proxies":{"slow":health.clone(),"fast":health}})
                        }.to_string();
                        let _=write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body);
                    });
                } else { std::thread::sleep(Duration::from_millis(5)); }
            }
        });
        let mut settings=Settings::default();
        settings.subscriptions.push(crate::model::Subscription { source: Default::default(), options: Default::default(),id:"fixture".into(),name:"fixture".into(),masked_url:String::new(),updated_at:0,error:None,
            servers:vec![json!({"name":"slow","type":"socks5","server":"127.0.0.1","port":1}),json!({"name":"fast","type":"socks5","server":"127.0.0.1","port":2})]});
        let (tx,rx)=mpsc::channel();
        std::thread::spawn(move || { let _=tx.send(verify_replacement(&ApiClient::loopback_fixture(port).with_probe_settings(&settings),&settings,&HashSet::new(),None,Default::default())); });
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
        tx.send(vec![("fixture".into(),delay,delay.is_some(),0)]).unwrap();
        recovery.active=Some(Scheduled{rx,generation:recovery.generation});
        let core=crate::core::Core::new(std::path::PathBuf::new(),std::path::PathBuf::new());
        recovery.poll_active(&core.client(),settings,Instant::now());
    }
    #[test]
    fn active_monitor_requires_sustained_slow_samples() {
        let settings=Settings{auto_optimize:true,..Settings::default()}; let mut recovery=Recovery::default();
        sample(&mut recovery,&settings,Some(151)); assert_eq!(recovery.slow_samples,1); assert!(!recovery.needed);
        sample(&mut recovery,&settings,Some(180)); assert_eq!(recovery.slow_samples,2); assert!(!recovery.needed);
        sample(&mut recovery,&settings,Some(160)); assert_eq!(recovery.slow_samples,3); assert!(recovery.needed);
        sample(&mut recovery,&settings,Some(150)); assert_eq!(recovery.slow_samples,0);
        assert!(recovery.needed); // The incident is completed only by verification.
    }
    #[test]
    fn disabled_optimization_keeps_slow_path_but_recovers_dead_path() {
        let settings=Settings::default();let mut recovery=Recovery::default();
        sample(&mut recovery,&settings,Some(500));assert!(!recovery.needed);
        sample(&mut recovery,&settings,None);assert!(recovery.needed);
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
        let (tx,rx)=mpsc::channel(); tx.send(vec![("old".into(),None,false,0)]).unwrap();
        recovery.active=Some(Scheduled{rx,generation:1});
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
        assert_eq!(candidates(&s,&p,&HashSet::new(),0),vec!["Sweden","other"]);
        assert_eq!(candidates(&s,&p,&HashSet::from([first]),0),vec!["Germany","other"]);
        let both = s.servers()[..2].iter().map(node_key).collect();
        assert_eq!(candidates(&s,&p,&both,0),vec!["other"]);
        assert_eq!(failed_endpoint("[TCP] dial ATLAS error: 1.2.3.4:443 connect error: context deadline exceeded"),Some("1.2.3.4:443".into()));
        assert!(failed_endpoint("[TCP] dial DIRECT error: dns resolve failed").is_none());
        assert!(failed_endpoint("ATLAS_EVENT {\"trigger\":\"[TCP] dial ATLAS error: 1.2.3.4:443 connect error: timeout\"}").is_none());
    }
    #[test] fn candidates_interleave_existing_sources_and_transports_without_mutating_settings() {
        let mut s=Settings::default();
        let node=|name:&str,port:u16,network:&str|json!({"name":name,"type":"socks5","port":port,"xray":{"outbounds":[{"streamSettings":{"network":network}}]}});
        let sub=|id:&str,servers:Vec<Value>|crate::model::Subscription{source:Default::default(),options:Default::default(),id:id.into(),name:id.into(),masked_url:String::new(),updated_at:0,error:None,servers};
        s.subscriptions.push(sub("one",vec![node("a",1,"raw"),node("b",2,"raw"),node("c",3,"ws")]));
        s.subscriptions.push(sub("two",vec![node("d",4,"raw")]));
        let before=serde_json::to_value(&s).unwrap();
        assert_eq!(candidates(&s,&Value::Null,&HashSet::new(),0),vec!["a","c","d","b"]);
        let mut h=crate::path_quality::History::default();for _ in 0..3 {h.record(Instant::now(),0,Some(900),true);}
        let quality=HashMap::from([(node_key(&s.servers()[0]),h)]);
        assert_eq!(ranked_candidates(&s,&Value::Null,&HashSet::new(),&quality),vec!["b","c","d","a"]);
        assert_eq!(serde_json::to_value(&s).unwrap(),before);
    }
}

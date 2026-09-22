//! Service-owned recovery. Workers only propose; the command thread commits.
use crate::{core::ApiClient, model::Settings};
use serde_json::{json, Value};
use std::{collections::{HashMap, HashSet}, sync::mpsc, time::{Duration, Instant}};

const QUARANTINE: u64 = 300;
fn endpoint(node: &Value) -> String {
    format!("{}:{}",node["server"].as_str().unwrap_or(""),node["port"].as_u64().unwrap_or(0))
}
fn failed_endpoint(line: &str) -> Option<String> {
    if line.starts_with("ATLAS_EVENT") || !line.contains("dial ATLAS") || !line.contains("connect error:") { return None; }
    let before = line.split(" connect error:").next()?;
    Some(before.rsplit("error: ").next()?.trim().to_owned())
}
pub(crate) fn candidates(settings: &Settings, proxies: &Value, blocked: &HashSet<String>, _now: u64) -> Vec<String> {
    let mut nodes: Vec<_> = settings.servers().into_iter().filter(|n| !blocked.contains(&endpoint(n))).collect();
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

struct Pending { rx: mpsc::Receiver<Value>, generation: u64 }
#[derive(Default)]
pub(crate) struct Recovery {
    generation: u64,
    last_line: Option<String>,
    blocked: HashMap<String,Instant>,
    pending: Option<Pending>,
    retry_at: Option<Instant>,
    needed: bool,
    pinned_until: Option<Instant>,
}
impl Recovery {
    #[cfg(test)]
    pub(crate) fn propose_for_test(&mut self, value: Value) {
        let (tx,rx)=mpsc::channel();tx.send(value).unwrap();
        self.pending=Some(Pending{rx,generation:self.generation});
    }
    pub fn invalidate(&mut self) { self.generation += 1; self.pending = None; self.needed = false; self.pinned_until = None; }
    pub fn tick(&mut self, client: ApiClient, settings: &Settings) {
        let now = Instant::now();
        self.blocked.retain(|_,until|*until > now);
        let lines = client.logs().unwrap_or_default();
        let fresh = self.last_line.as_ref().and_then(|last|lines.iter().rposition(|l|l == last)).map_or(0,|i|i+1);
        for line in &lines[fresh..] {
            if let Some(peer) = failed_endpoint(line) {
                if settings.servers().iter().any(|n|endpoint(n) == peer) {
                    self.blocked.insert(peer.clone(), now + Duration::from_secs(QUARANTINE));
                    self.needed = true;
                    client.event(json!({"at":crate::model::now(),"kind":"endpoint_quarantined","endpoint":peer,"seconds":QUARANTINE,"trigger":line}));
                }
            }
        }
        self.last_line = lines.last().cloned();
        if !matches!(settings.selected.as_str(),"AUTO"|"FAILOVER") || settings.routing_mode == crate::model::RoutingMode::Direct {
            self.needed = false; return;
        }
        if let Some(p) = &self.pending {
            match p.rx.try_recv() {
                Ok(report) => {
                    let generation = p.generation;
                    self.pending = None;
                    if generation != self.generation { return; }
                    // Move on to other endpoints after failed controls; do not
                    // retry the same four candidates forever when histories are stale.
                    for test in report["results"].as_array().into_iter().flatten().filter(|v|v["passed"] == false) {
                        if let Some(node)=settings.servers().iter().find(|n|n["name"] == test["name"]) {
                            self.blocked.entry(endpoint(node)).or_insert(now + Duration::from_secs(60));
                        }
                    }
                    let selected = report["candidate"].as_str().filter(|name| settings.servers().iter().any(|n|
                        n["name"] == *name && !self.blocked.contains_key(&endpoint(n))));
                    let result = selected.map(|name|client.api("PUT","/proxies/ATLAS",Some(json!({"name":name}))).and_then(|_| {
                        let state=client.api("GET","/proxies",None)?;
                        if state["proxies"]["ATLAS"]["now"] != name { return Err("Ядро не подтвердило выбранную альтернативу".into()); }
                        Ok(json!({"confirmedSelection":name}))
                    }));
                    if result.as_ref().is_some_and(|r|r.is_ok()) {
                        self.pinned_until = Some(now + Duration::from_secs(QUARANTINE));
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
        if self.pinned_until.is_some_and(|until|until <= now) && self.pending.is_none() && !self.needed {
            match client.api("PUT","/proxies/ATLAS",Some(json!({"name":settings.selected}))) {
                Ok(_) => { self.pinned_until = None; client.event(json!({"at":crate::model::now(),"kind":"automatic_selection_resumed","group":settings.selected})); }
                Err(e) => { self.pinned_until = Some(now + Duration::from_secs(30)); client.event(json!({"at":crate::model::now(),"kind":"selector_restore_error","error":e})); }
            }
        }
        if !self.needed || self.pending.is_some() || self.retry_at.is_some_and(|t|t>now) { return; }
        self.needed = false;
        self.retry_at = Some(now + Duration::from_secs(30));
        let (tx,rx) = mpsc::channel();
        let blocked = self.blocked.keys().cloned().collect();
        let settings = settings.clone();
        client.event(json!({"at":crate::model::now(),"kind":"recovery_started","blockedEndpoints":self.blocked.keys().collect::<Vec<_>>()}));
        self.pending = Some(Pending{rx,generation:self.generation});
        std::thread::spawn(move || { let _ = tx.send(verify(&client,&settings,&blocked,&crate::latency::ENDPOINTS)); });
    }
}

pub(crate) fn verify(client: &ApiClient, settings: &Settings, blocked: &HashSet<String>, controls: &[&str;2]) -> Value {
    let proxies = match client.api("GET","/proxies",None) { Ok(p)=>p,Err(e)=>return json!({"error":e}) };
    let names = candidates(settings,&proxies,blocked,crate::model::now());
    let results = std::thread::scope(|scope| {
        let jobs: Vec<_> = names.iter().map(|name|scope.spawn(move || {
            let mut checks = Vec::new();
            for round in 0..2 {
                for (control,url) in controls.iter().enumerate() {
                    let at = crate::model::now();
                    let result = crate::latency::verified_probe(client,name,url).unwrap_or_else(|e|json!({"error":e}));
                    let ok = result["expectedStatusMatched"] == true;
                    checks.push(json!({"round":round,"control":control,"at":at,"result":result}));
                    if !ok { return json!({"name":name,"passed":false,"checks":checks}); }
                }
                if round == 0 { std::thread::sleep(Duration::from_millis(500)); }
            }
            json!({"name":name,"passed":true,"checks":checks})
        })).collect();
        jobs.into_iter().map(|j|j.join().unwrap_or(json!({"error":"Probe worker failed"}))).collect::<Vec<_>>()
    });
    let candidate = results.iter().find(|r|r["passed"]==true).map(|r|r["name"].clone());
    json!({"candidate":candidate,"results":results})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_labels_sharing_failed_endpoint_are_excluded() {
        let mut s=Settings::default();
        s.subscriptions.push(crate::model::Subscription{id:"x".into(),name:"x".into(),masked_url:"".into(),updated_at:0,error:None,
            servers:vec![json!({"name":"Sweden","server":"1.2.3.4","port":443}),json!({"name":"Germany","server":"1.2.3.4","port":443}),json!({"name":"other","server":"5.6.7.8","port":443})]});
        let p=json!({"proxies":{"Sweden":{"alive":true},"Germany":{"alive":true},"other":{"alive":true}}});
        assert_eq!(candidates(&s,&p,&HashSet::from(["1.2.3.4:443".into()]),0),vec!["other"]);
        assert_eq!(failed_endpoint("[TCP] dial ATLAS error: 1.2.3.4:443 connect error: context deadline exceeded"),Some("1.2.3.4:443".into()));
        assert!(failed_endpoint("[TCP] dial DIRECT error: dns resolve failed").is_none());
        assert!(failed_endpoint("ATLAS_EVENT {\"trigger\":\"[TCP] dial ATLAS error: 1.2.3.4:443 connect error: timeout\"}").is_none());
    }
}

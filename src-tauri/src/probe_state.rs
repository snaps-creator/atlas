//! Authoritative URL-test evidence. Core history is advisory, never UI truth.
use crate::{latency::node_identity, model::Settings};
use serde_json::{json, Value};
use std::{collections::HashMap, sync::{Arc, Mutex}};

// One minute between pool measurements, with room for a bounded batch to finish.
pub const FRESH_MS: u64 = 90_000;
pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}
#[derive(Default)]
pub struct State {
    generation: u64,
    sequence: u64,
    nodes: HashMap<String, Value>,
    records: HashMap<(String, String), Value>,
    gates: HashMap<(String,String), Arc<Mutex<()>>>,
    connections: HashMap<String,Value>,
}
pub struct Ticket {
    generation: u64,
    sequence: u64,
    key: (String, String),
    pub gate: Arc<Mutex<()>>,
    pub node: Value,
}
impl State {
    pub fn suspend(&mut self) -> HashMap<String,Value> {
        let nodes=std::mem::take(&mut self.nodes); self.invalidate(); nodes
    }
    pub fn restore(&mut self, nodes: HashMap<String,Value>) {
        self.invalidate(); self.nodes=nodes;
    }
    pub fn configure(&mut self, settings: &Settings) {
        self.invalidate();
        self.nodes = settings.servers().into_iter().filter_map(|n|
            n["name"].as_str().map(|name| (name.to_owned(), n.clone()))).collect();
    }
    pub fn invalidate(&mut self) {
        self.generation += 1;
        self.records.clear();
        self.connections.clear();
        self.nodes.clear();
        // Keep only live gates across reloads: old requests may still finish.
        self.gates.retain(|_,gate|Arc::strong_count(gate)>1);
    }
    pub fn begin(&mut self, name: &str, endpoint: &str) -> Result<Ticket, String> {
        let node = self.nodes.get(name).ok_or("Проверяемый сервер отсутствует в текущей конфигурации")?.clone();
        self.sequence += 1;
        let key = (name.to_owned(), endpoint.to_owned());
        let previous = self.records.get(&key).filter(|v| matches!(v["status"].as_str(), Some("unreachable"|"error"))).cloned();
        self.records.insert(key.clone(), json!({"status":"testing","delay":null,"attempts":1,"pending":true,
            "operationId":self.sequence,"generation":self.generation,"probeId":node_identity(&node),
            "endpoint":endpoint,"startedAt":now_ms(),"scope":"tcp_http","udp":"untested"}));
        let record=self.records.get_mut(&key).unwrap();
        // Starting work is not new network evidence. Retain the last failure
        // while advancing the operation fence, so an older success cannot win.
        if let Some(previous) = previous {
            for field in ["status", "error", "errorKind", "measuredAt"] {
                record[field] = previous[field].clone();
            }
        }
        record["outbound"]=json!(target_name(&node));
        if node["type"]=="xray" && node["xraySimple"]!=true {
            record["outboundTag"]=json!(profile_outbound(&node["xray"]).map(|(_,tag)|tag));
            record["scope"]=json!("tcp_http_profile_outbound");
        }
        let gate=self.gates.entry(key.clone()).or_default().clone();
        Ok(Ticket { generation:self.generation, sequence:self.sequence, key, node, gate })
    }
    pub fn current(&self, ticket: &Ticket) -> bool {
        ticket.generation == self.generation && self.records.get(&ticket.key)
            .is_some_and(|v|v["operationId"] == ticket.sequence)
    }
    pub fn finish(&mut self, ticket: &Ticket, result: &Result<u64,String>) -> Result<Value,String> {
        if !self.current(ticket) { return Err("CANCELLED: результат проверки устарел".into()); }
        let record = self.records.get_mut(&ticket.key).unwrap();
        record["pending"] = json!(false);
        record.as_object_mut().unwrap().remove("error");
        record.as_object_mut().unwrap().remove("errorKind");
        record["measuredAt"] = json!(now_ms());
        match result {
            Ok(delay) => {record["status"]=json!("ok"); record["delay"]=json!(delay);}
            Err(error) => {
                record["status"]=json!(if error.contains("HTTP 504") {"unreachable"} else {"error"});
                record["delay"]=Value::Null;
                record["error"]=json!(error);
                record["errorKind"]=json!(if error.contains("HTTP 504") {"timeout"}
                    else if error.contains("HTTP 503") {"proxy_or_transport"}
                    else if error.starts_with("RESPONSE:") {"unexpected_response"}
                    else if error.starts_with("ROUTE:") {"outbound_unverified"} else {"controller"});
            }
        }
        Ok(record.clone())
    }
    pub fn annotate(&self, proxies: &mut Value) {
        if let Some(proxies) = proxies["proxies"].as_object_mut() {
            for (name, proxy) in proxies {
                proxy["atlasConnection"] = self.connections.get(name).cloned().unwrap_or(Value::Null);
                proxy["atlasHealth"] = self.records.get(&(name.clone(), crate::latency::DISPLAY_URL.into()))
                    .cloned().map(|mut v| {
                        v["connection"]=self.connections.get(name).cloned().unwrap_or(Value::Null);
                        if v["status"] == "ok" && now_ms().saturating_sub(v["measuredAt"].as_u64().unwrap_or(0)) >= FRESH_MS {
                            v["status"]=json!("stale"); v["delay"]=Value::Null;
                        }
                        v
                    }).unwrap_or(Value::Null);
            }
        }
    }
    pub fn connection_failed(&mut self,name:&str,transport:&str) {
        if self.nodes.contains_key(name) {
            self.connections.insert(name.into(),json!({"status":"error","transport":transport,"observedAt":now_ms(),
                "scope":"application_connection","error":"Ошибка пользовательского соединения; результат контрольного HTTP-запроса хранится отдельно"}));
        }
    }
}

/// Full profiles get a separate authenticated inbound pinned to this concrete
/// outbound. Their application routing and balancers remain untouched.
pub fn profile_outbound(profile: &Value) -> Option<(usize,String)> {
    let outbounds=profile["outbounds"].as_array()?;
    let index=outbounds.iter().position(|o|matches!(o["protocol"].as_str(),
        Some("vless"|"vmess"|"trojan"|"shadowsocks"|"socks"|"http"|"wireguard"|"hysteria")))?;
    let tag=outbounds[index]["tag"].as_str().map(str::to_owned).unwrap_or_else(|| {
        let mut tag="atlas-probe-outbound".to_owned();
        while outbounds.iter().any(|o|o["tag"]==tag) {tag.push('_');}
        tag
    });
    Some((index,tag))
}
pub fn target_name(node: &Value) -> String {
    if node["type"]=="xray" && node["xraySimple"]!=true {
        format!("atlas-probe-{}",node_identity(node))
    } else {node["name"].as_str().unwrap_or_default().to_owned()}
}
pub fn validate_route(node: &Value) -> Result<(),String> {
    let reject = || Err("ROUTE: прохождение через конкретный прокси не подтверждено".into());
    if matches!(node["type"].as_str(), None|Some("direct"|"reject"|"dns"|"compatible"|"select"|"url-test"|"fallback"|"load-balance")) { return reject(); }
    if node["type"] != "xray" { return Ok(()); }
    let profile=&node["xray"];
    let Some(outbounds)=profile["outbounds"].as_array() else {return reject();};
    let Some(first)=outbounds.first() else {return reject();};
    if node["xraySimple"] == true {
        if matches!(first["protocol"].as_str(),None|Some("freedom"|"blackhole"|"dns"|"loopback")) {return reject();}
        return Ok(());
    }
    if profile_outbound(profile).is_none() {return reject();}
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn settings(port:u16)->Settings {
        let mut s=Settings::default();
        s.subscriptions.push(crate::model::Subscription {source:Default::default(), options:Default::default(),
            id:"test".into(), name:"test".into(), masked_url:String::new(), updated_at:0,error:None,
            servers:vec![json!({"name":"node","type":"socks5","server":"127.0.0.1","port":port})]}); s
    }
    #[test] fn timeout_survives_pending_and_recovery_clears_failure_metadata() {
        let mut s=State::default();s.configure(&settings(1234));
        let endpoint=crate::latency::DISPLAY_URL;
        let success=s.begin("node",endpoint).unwrap();s.finish(&success,&Ok(56)).unwrap();
        let failed=s.begin("node",endpoint).unwrap();
        s.finish(&failed,&Err("Mihomo API: HTTP 504".into())).unwrap();
        let retry=s.begin("node",endpoint).unwrap();
        let mut view=json!({"proxies":{"node":{"history":[{"delay":56}]}}});s.annotate(&mut view);
        let record=&view["proxies"]["node"]["atlasHealth"];
        assert_eq!(record["status"],"unreachable");assert_eq!(record["errorKind"],"timeout");
        assert_eq!(record["pending"],true);assert_eq!(record["delay"],Value::Null);
        assert!(s.finish(&success,&Ok(56)).is_err());
        let recovered=s.finish(&retry,&Ok(81)).unwrap();
        assert_eq!(recovered["status"],"ok");assert_eq!(recovered["pending"],false);
        assert!(recovered.get("error").is_none());assert!(recovered.get("errorKind").is_none());
        assert!(s.finish(&failed,&Err("Mihomo API: HTTP 504".into())).is_err());
        let transport=s.begin("node",endpoint).unwrap();
        let error=s.finish(&transport,&Err("Mihomo API: HTTP 503".into())).unwrap();
        assert_eq!(error["status"],"error");assert_eq!(error["errorKind"],"proxy_or_transport");
        s.invalidate();s.annotate(&mut view);assert_eq!(view["proxies"]["node"]["atlasHealth"],Value::Null);
    }
    #[test] fn newer_failure_wins_and_configuration_never_inherits_success() {
        let mut s=State::default(); s.configure(&settings(1234));
        let a=s.begin("node","url").unwrap(); let b=s.begin("node","url").unwrap();
        assert_eq!(s.finish(&b,&Err("Mihomo API: HTTP 504".into())).unwrap()["delay"],Value::Null);
        assert!(s.finish(&a,&Ok(56)).is_err());
        let c=s.begin("node","url").unwrap();s.configure(&settings(2345));
        assert!(s.finish(&c,&Ok(56)).is_err()); assert!(s.records.is_empty());
    }
    #[test] fn same_url_serializes_but_independent_controls_do_not_block_each_other() {
        let mut s=State::default();s.configure(&settings(1234));
        let a=s.begin("node","primary").unwrap();let b=s.begin("node","primary").unwrap();
        let secondary=s.begin("node","secondary").unwrap();
        assert!(Arc::ptr_eq(&a.gate,&b.gate));
        assert!(!Arc::ptr_eq(&a.gate,&secondary.gate));
        assert!(!s.current(&a));assert!(s.current(&b));assert!(s.current(&secondary));
        s.configure(&settings(2345));
        let current=s.begin("node","primary").unwrap();
        assert!(Arc::ptr_eq(&a.gate,&current.gate));assert!(!s.current(&b));
    }
    #[test] fn direct_and_ambiguous_profile_cannot_certify_proxy() {
        assert!(validate_route(&json!({"type":"direct"})).is_err());
        let profile=json!({"name":"profile","type":"xray","xray":{"outbounds":[{"tag":"proxy","protocol":"vless"}],
            "routing":{"rules":[{"outboundTag":"direct"}]}}});
        assert!(validate_route(&profile).is_ok());
        assert_ne!(target_name(&profile),"profile");
        assert!(validate_route(&json!({"type":"xray","xraySimple":true,"xray":{"outbounds":[{"tag":"proxy","protocol":"vless"},{"protocol":"freedom"}]}})).is_ok());
    }
    #[test] fn shared_view_expires_success_and_keeps_udp_evidence_separate() {
        let mut s=State::default();s.configure(&settings(1234));
        let ticket=s.begin("node",crate::latency::DISPLAY_URL).unwrap();
        s.finish(&ticket,&Ok(56)).unwrap();
        s.connection_failed("node","udp");
        let mut view=json!({"proxies":{"node":{}}});s.annotate(&mut view);
        assert_eq!(view["proxies"]["node"]["atlasHealth"]["status"],"ok");
        assert_eq!(view["proxies"]["node"]["atlasHealth"]["connection"]["transport"],"udp");
        s.records.get_mut(&ticket.key).unwrap()["measuredAt"]=json!(now_ms()-FRESH_MS);
        s.annotate(&mut view);
        assert_eq!(view["proxies"]["node"]["atlasHealth"]["status"],"stale");
        assert_eq!(view["proxies"]["node"]["atlasHealth"]["delay"],Value::Null);
        let failed=s.begin("node",crate::latency::DISPLAY_URL).unwrap();
        s.finish(&failed,&Err("Mihomo API: HTTP 504".into())).unwrap();s.annotate(&mut view);
        assert_eq!(view["proxies"]["node"]["atlasHealth"]["status"],"unreachable");
        assert_eq!(view["proxies"]["node"]["atlasHealth"]["delay"],Value::Null);
    }
}

//! Session-only fallback adapters. Never persist them into subscriptions or rules.
use crate::model::Settings;
use serde_json::{json, Value};
use std::collections::HashSet;

pub(crate) const MARKER: &str = "atlasRecoveryVariant";
pub(crate) fn is_variant(node: &Value) -> bool { node[MARKER].is_string() }

/// Only a simple, unmodified TCP TLS/REALITY dialer is eligible. Server address,
/// credentials, SNI, certificate checks, flow and application transport survive.
/// Provider-defined chains, masks and fragmentation always take precedence.
pub(crate) fn variant(node: &Value) -> Option<Value> {
    if node["type"] != "xray" || node["xraySimple"] != true || is_variant(node) { return None; }
    let outbounds=node["xray"]["outbounds"].as_array()?;
    if outbounds.len()!=1 { return None; }
    let outbound=&outbounds[0];
    let stream=&outbound["streamSettings"];
    if !matches!(outbound["protocol"].as_str(),Some("vless"|"trojan"))
        || !matches!(stream["network"].as_str(),Some("tcp"|"raw"))
        || !matches!(stream["security"].as_str(),Some("tls"|"reality"))
        || stream.get("finalmask").is_some() || outbound.get("proxySettings").is_some()
        || stream["sockopt"].get("dialerProxy").is_some()
        || outbound["mux"]["enabled"]==true { return None; }
    let name=node["name"].as_str()?;
    let mut result=node.clone();
    result["name"]=json!(format!("{name} · защита"));
    result[MARKER]=json!(name);
    let stream=&mut result["xray"]["outbounds"][0]["streamSettings"];
    if !stream["sockopt"].is_object() { stream["sockopt"]=json!({}); }
    stream["sockopt"]["dialerProxy"]=json!("atlas-hello-fragment");
    result["xray"]["outbounds"].as_array_mut()?.push(json!({
        "tag":"atlas-hello-fragment", "protocol":"freedom",
        "settings":{"fragment":{"packets":"tlshello","length":"100-200","interval":"0"}}
    }));
    Some(result)
}

/// Same deterministic expansion for runtime, certified probes and recovery.
/// No sockets, requests or extra process here; variants share the Xray worker.
pub(crate) fn runtime_settings(settings: &Settings) -> Settings {
    let mut result=settings.clone();
    let nodes=settings.servers();
    let mut names: HashSet<_>=nodes.iter().filter_map(|n|n["name"].as_str().map(str::to_owned)).collect();
    // Keep the existing runtime ceiling; a large catalog remains connectable.
    let mut room=512usize.saturating_sub(nodes.iter().filter(|n|n["type"]=="xray").count());
    for sub in &mut result.subscriptions {
        let variants: Vec<_>=sub.servers.iter().filter_map(variant).filter(|n| {
            room>0 && names.insert(n["name"].as_str().unwrap().to_owned()) && {room-=1;true}
        }).collect();
        sub.servers.extend(variants);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn node() -> Value {
        crate::xray_config::uri("vless://00000000-0000-0000-0000-000000000001@example.com:443?security=tls&type=tcp&flow=xtls-rprx-vision&sni=tls.example#fixture").unwrap().unwrap()
    }
    #[test] fn fragment_changes_only_transport_dialer_and_internal_identity() {
        let original=node(); let mut guarded=variant(&original).unwrap();
        assert_eq!(guarded["xray"]["outbounds"][1]["settings"]["fragment"],json!({"packets":"tlshello","length":"100-200","interval":"0"}));
        guarded["xray"]["outbounds"].as_array_mut().unwrap().pop();
        guarded["xray"]["outbounds"][0]["streamSettings"].as_object_mut().unwrap().remove("sockopt");
        guarded["name"]=original["name"].clone(); guarded.as_object_mut().unwrap().remove(MARKER);
        assert_eq!(guarded,original);
    }
    #[test] fn skips_provider_chains_other_transports_and_existing_masks() {
        let original=node();
        for (field,value) in [("network",json!("xhttp")),("network",json!("ws")),("security",json!("none")),("finalmask",json!({})),("sockopt",json!({"dialerProxy":"provider"}))] {
            let mut n=original.clone();n["xray"]["outbounds"][0]["streamSettings"][field]=value;assert!(variant(&n).is_none());
        }
        let mut full=original.clone();full["xraySimple"]=json!(false);assert!(variant(&full).is_none());
        assert!(variant(&variant(&original).unwrap()).is_none());
    }
    #[test] fn expansion_is_ephemeral_idempotent_and_bounded() {
        let mut s=Settings::default();
        s.subscriptions.push(crate::model::Subscription{source:Default::default(),options:Default::default(),id:"fixture".into(),name:"fixture".into(),masked_url:String::new(),updated_at:0,error:None,servers:vec![node()]});
        let before=serde_json::to_value(&s).unwrap();let expanded=runtime_settings(&s);
        assert_eq!(expanded.servers().len(),2);assert_eq!(serde_json::to_value(&s).unwrap(),before);
        assert_eq!(runtime_settings(&expanded).servers(),expanded.servers());
        s.subscriptions[0].servers=(0..512).map(|i|{let mut n=node();n["name"]=json!(format!("node-{i}"));n}).collect();
        assert_eq!(runtime_settings(&s).servers().len(),512);
    }
}

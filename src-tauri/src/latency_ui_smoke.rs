//! Fixtures for the existing isolated packaged-UI acceptance mode only.
use serde_json::{json,Value};
pub fn enabled() -> bool { std::env::var("ATLAS_LATENCY_UI_SMOKE").as_deref()==Ok("1") }
pub fn reply(action:&str) -> Result<Value,String> {
    if !enabled() {return Err("Isolated UI smoke has no application session".into());}
    static START:std::sync::OnceLock<std::time::Instant>=std::sync::OnceLock::new();
    let age=START.get_or_init(std::time::Instant::now).elapsed().as_secs();
    let now=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    let ids=json!({"Low":"low-v1","High":"high-v1","Failed":"failed-v1"});
    match action {
        "snapshot"=>Ok(json!({"revision":1,"status":"Disconnected","running":false,"guardActive":false,"error":null,"duration":0,"logs":[],
            "settings":{"groups":[],"subscriptions":[{"id":"fixture","name":"Acceptance fixture","source":"URL","maskedUrl":"https://fixture.invalid/********","updatedAt":1,"error":null,
                "servers":[{"name":"Low","type":"vless","probeId":"low-v1"},{"name":"High","type":"vless","probeId":"high-v1"},{"name":"Failed","type":"vless","probeId":"failed-v1"}]}],
                "selected":"AUTO","defaultRoute":"DIRECT","rulesSemanticsVersion":2,"mode":"tun","routingMode":"rule","tunStack":"gvisor","dns":{"servers":[],"ipv6":false,"fakeIp":false},
                "startup":{"launchWithWindows":false,"autoConnect":false,"startInTray":false,"delaySeconds":0,"restoreConnection":false},"autoTestIntervalSeconds":300,"autoSearchPingMs":150,"theme":"light","wasConnected":false,"favorites":[]}})),
        "latency_state"=>{
            let success=|delay,at|json!({"status":"ok","delay":delay,"attempts":1,"measuredAt":at});
            let failure=json!({"status":"timeout","delay":null,"attempts":1,"measuredAt":now});
            let results=if age<2 {json!({})} else {json!({
                "Low":if age<5 {success(43,now)} else if age<8 {failure.clone()} else {success(55,now)},
                "High":if age<5 {success(1501,now)} else if age<8 || age>=11 {success(321,now)} else {success(321,now-31000)},
                "Failed":failure})};
            Ok(json!({"results":results,"identities":ids,"pending":[],"generation":1}))
        },
        "refresh_due"|"diagnostic_event"=>Ok(Value::Null),
        _=>Err("Action is not part of the read-only latency fixture".into()),
    }
}

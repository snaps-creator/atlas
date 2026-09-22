//! Incident export: passive evidence first, then explicitly requested bounded probes.
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    os::windows::process::CommandExt,
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

const LIMIT: u64 = 1024 * 1024;

#[derive(Clone)]
pub(crate) struct Capture {
    pub revision: u64,
    pub captured_at: u64,
    pub client: crate::core::ApiClient,
    pub secrets: Vec<String>,
    pub configuration: Value,
}
#[derive(Clone, Default)]
pub(crate) struct Access(std::sync::Arc<std::sync::RwLock<Option<Capture>>>);
impl Access {
    pub fn update(&self, revision: u64, settings: &crate::model::Settings, client: crate::core::ApiClient) {
        let Ok(mut cached) = self.0.try_write() else { return; };
        if let Some(cached) = cached.as_mut().filter(|v|v.revision == revision) {
            cached.client = client;
            cached.captured_at = crate::model::now();
            return;
        }
        let mut secrets = Vec::new();
        if let Ok(value) = serde_json::to_value(settings) { collect_secrets(&value,&mut secrets); }
        *cached = Some(Capture {revision,captured_at:crate::model::now(),client,secrets,configuration:configuration_evidence(settings)});
    }
    pub fn get(&self) -> Option<Capture> { self.0.try_read().ok().and_then(|v|v.clone()) }
}

pub(crate) fn privileged_snapshot() -> Result<String, String> {
    static BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if BUSY.swap(true,std::sync::atomic::Ordering::SeqCst) { return Err("Privileged collector already running".into()); }
    struct Release;
    impl Drop for Release { fn drop(&mut self) { BUSY.store(false,std::sync::atomic::Ordering::SeqCst); } }
    let _release = Release;
    let mut command = Command::new("powershell.exe");
    command.args(["-NoLogo","-NoProfile","-NonInteractive","-Command",include_str!("support_privileged.ps1")]);
    run_bounded(command, Duration::from_secs(24))
}
#[derive(Default)]
pub(crate) struct Recorder { last_log: Option<String>, counters: crate::interface_evidence::Counters,
    previous_connections: Option<std::collections::HashMap<String,u64>> }
impl Recorder {
    pub fn sample(&mut self, client: &crate::core::ApiClient) -> (Value, bool) {
        let started = crate::model::now();
        let proxies = client.api("GET", "/proxies", None).map(|v| {
            let p = &v["proxies"];
            let mut name = "ATLAS".to_owned();
            let mut chain = Vec::new();
            for _ in 0..8 {
                chain.push(name.clone());
                match p[&name]["now"].as_str() { Some(next) if !chain.iter().any(|n|n == next) => name = next.into(), _ => break }
            }
            let all = p.as_object();
            let members = p["AUTO"]["all"].as_array().cloned().unwrap_or_default();
            let healthy = members.iter().filter(|n|p[n.as_str().unwrap_or("")]["alive"] == true).count();
            let selected = all.and_then(|p|p.get(&name)).cloned().unwrap_or(Value::Null);
            // Full inventory is captured on incidents/export. Normal samples retain
            // selection and its URL history, not 124 repeated names every 15 seconds.
            serde_json::json!({"chain":chain,"selected":name,"selectedHealth":proxy_evidence(&serde_json::json!({"proxies":{name:selected}})),
                "poolMembers":members.len(),"poolAlive":healthy,"poolOther":members.len()-healthy})
        }).unwrap_or_else(|e|serde_json::json!({"error":e}));
        let mut incident = false;
        let logs = client.logs().map(|lines| {
            let position = self.last_log.as_ref().and_then(|last|lines.iter().rposition(|l|l == last));
            let gap = self.last_log.is_some() && position.is_none();
            let fresh = &lines[position.map_or(0, |i|i+1)..];
            incident = fresh.iter().any(|l| !l.starts_with("ATLAS_EVENT") && (l.contains("connect error:") || l.contains("resolve failed") || l.contains("can't resolve ip")));
            self.last_log = lines.last().cloned();
            serde_json::json!({"newLines":fresh.iter().rev().take(512).collect::<Vec<_>>(),
                "gap":gap,"omittedNewLines":fresh.len().saturating_sub(512),"evidence":failure_evidence(fresh)})
        }).unwrap_or_else(|e|serde_json::json!({"error":e}));
        let traffic = client.api("GET","/connections",None).map(|v| {
            let connections=v["connections"].as_array().cloned().unwrap_or_default();
            let mut next=std::collections::HashMap::new();
            let mut progress=Vec::new();
            for c in connections.iter().take(2048) {
                let id=c["id"].as_str().unwrap_or("").to_owned();let downloaded=c["download"].as_u64().unwrap_or(0);
                let increase=self.previous_connections.as_ref().map(|old|downloaded.saturating_sub(old.get(&id).copied().unwrap_or(0)));
                next.insert(id,downloaded);
                if increase.is_some_and(|n|n>0) && progress.len()<32 {
                    progress.push(serde_json::json!({"chains":c["chains"],"rule":c["rule"],"rulePayload":c["rulePayload"],
                        "process":c["metadata"]["process"],"host":c["metadata"]["host"],"downloadDelta":increase}));
                }
            }
            let baseline=self.previous_connections.is_none();self.previous_connections=Some(next);
            serde_json::json!({"baselineOnly":baseline,"connections":connections.len(),"downloadTotal":v["downloadTotal"],
                "uploadTotal":v["uploadTotal"],"observedProgress":progress,
                "scope":"At most 32 connections with received-byte growth; short connections between samples can be missed. A transfer is not proof of application-level success."})
        }).unwrap_or_else(|e|serde_json::json!({"error":e}));
        (serde_json::json!({"startedAt":started,"completedAt":crate::model::now(),"proxies":proxies,"logs":logs,"traffic":traffic,"interfaces":self.counters.sample()}), incident)
    }
}

pub(crate) fn automatic_incident(client: crate::core::ApiClient, secrets: Vec<String>, revision: u64) {
    let before = core_snapshot(Some(&client));
    let deadline = Instant::now() + Duration::from_secs(60);
    let (active, privileged, windows) = std::thread::scope(|scope| {
        let p = scope.spawn(||client.support_snapshot().unwrap_or_else(|e|serde_json::json!({"error":e})));
        let w = scope.spawn(|| {
            let mut command = Command::new("powershell.exe");
            command.args(["-NoLogo","-NoProfile","-NonInteractive","-Command",include_str!("support_snapshot.ps1")]);
            run_bounded(command, Duration::from_secs(25)).unwrap_or_else(|e|e)
        });
        let active = crate::support_probes::run(Some(client.clone()), deadline);
        (active,p.join().unwrap_or(Value::Null),w.join().unwrap_or_default())
    });
    crate::incident_history::record("incident_summary",serde_json::json!({"revision":revision,"startedAt":before["startedAt"],
        "completedAt":crate::model::now(),"failures":before["logs"]["evidence"],"http":active["http"],"upstreamDns":active["upstreamDns"]}),&secrets);
    crate::incident_history::record("automatic_incident",serde_json::json!({"revision":revision,"before":before,
        "active":active,"privileged":privileged,"windows":windows,"after":core_snapshot(Some(&client))}),&secrets);
}
fn core_snapshot(client: Option<&crate::core::ApiClient>) -> Value {
    let Some(c) = client else { return serde_json::json!({"error":"No captured core session"}); };
    let started = crate::model::now();
    let version = c.api("GET","/version",None).unwrap_or_else(|e|serde_json::json!({"error":e}));
    let proxies = c.api("GET","/proxies",None).map(|v|proxy_evidence(&v))
        .unwrap_or_else(|e|serde_json::json!({"error":e}));
    let logs = c.logs().map(|lines|serde_json::json!({"evidence":failure_evidence(&lines),"omittedLines":lines.len().saturating_sub(256),
        "lines":lines.iter().rev().take(256).collect::<Vec<_>>()}))
        .unwrap_or_else(|e|serde_json::json!({"error":e}));
    serde_json::json!({"startedAt":started,"completedAt":crate::model::now(),"version":version,"proxies":proxies,"logs":logs})
}

fn conclusions(parts: &serde_json::Map<String, Value>) -> Vec<Value> {
    let mut results = Vec::new();
    let active = parts.get("active").unwrap_or(&Value::Null);
    if let Some(http) = active["http"].as_array() {
        for proxy in http.iter().filter(|r|r["path"]=="local_mixed_proxy" && r["controlSucceeded"]==true) {
            if http.iter().any(|r|r["path"]=="windows_default_path" && r["endpoint"]==proxy["endpoint"] && r["controlSucceeded"]==false) {
                results.push(serde_json::json!({"code":"PATHS_DIFFER","confirmed":"Контрольный запрос успешен через локальный прокси, но не обычным путём Windows",
                    "scope":"Проверить TUN, DNS, WFP и различие правил; это не доказательство конкретного фильтра"}));
            }
        }
    }
    let successes = active["nodeTests"]["results"].as_array().into_iter().flatten().filter(|node| {
        node["checks"].as_array().into_iter().flatten().any(|c|c["result"]["delay"].as_u64().is_some())
    }).count();
    if successes > 0 { results.push(serde_json::json!({"code":"VPN_PATH_WORKS","confirmed":"Есть успешный запрос через VPN-узел на этом ПК","nodes":successes})); }
    if parts.get("before").is_some_and(|v|v["version"].get("error").is_some()) {
        results.push(serde_json::json!({"code":"LOCAL_CONTROL_ERROR","confirmed":"Запрос к локальному ядру завершился ошибкой",
            "scope":"Ошибка контроллера/IPC не является доказательством отказа VPN-серверов"}));
    }
    results.push(serde_json::json!({"code":"EVIDENCE_SCOPE","rule":"Таймаут не доказывает отказ удалённого сервера. Для различения silent drop по пути и отказа сервера нужны события блокировки или независимые данные другой стороны. Наличие фильтра не означает, что именно он заблокировал пакет."}));
    results
}

pub(crate) fn failure_evidence(lines: &[String]) -> Value {
    let lines: Vec<_> = lines.iter().filter(|line|!line.starts_with("ATLAS_EVENT")).collect();
    let tcp_dial = lines.iter().filter(|s| s.contains("dial tcp ") && s.contains("error:")).count();
    let established_read = lines.iter().filter(|s| s.contains("read tcp ") && s.contains("error:")).count();
    let ambiguous_connect = lines.iter().filter(|s| s.contains("connect error:") &&
        !s.contains("dial tcp ") && !s.contains("read tcp ")).count();
    serde_json::json!({"tcpDialErrors":tcp_dial,"establishedSocketReadErrors":established_read,
        "dnsResolutionErrors":lines.iter().filter(|s|s.contains("resolve failed") || s.contains("can't resolve ip")).count(),
        "explicitTlsOrCertificateErrors":lines.iter().filter(|s|s.contains("tls:") || s.contains("x509:") || s.contains("certificate verify")).count(),
        "explicitWebSocketUpgradeErrors":lines.iter().filter(|s|s.contains("websocket: bad handshake")).count(),
        "connectionRefusedErrors":lines.iter().filter(|s|s.contains("connection refused") || s.contains("actively refused")).count(),
        "unspecifiedConnectOrHandshakeErrors":ambiguous_connect,
        "scope":"Bounded core log window; not independent probes and not proof of provider outage",
        "interpretation":"Mihomo connect error includes TCP and TLS/VLESS handshake. A read tcp error means a socket had been established."})
}

pub(crate) fn configuration_evidence(settings: &crate::model::Settings) -> Value {
    let subscriptions: Vec<Value> = settings.subscriptions.iter().map(|sub| {
        let nodes: Vec<Value> = sub.servers.iter().map(|node| {
            let mut safe = serde_json::Map::new();
            let mut identity = node.clone();
            if let Some(fields) = identity.as_object_mut() { fields.remove("name"); fields.remove("country"); }
            safe.insert("configurationId".into(),Value::String(format!("{:x}",Sha256::digest(identity.to_string().as_bytes()))));
            for field in ["name", "type", "server", "port", "network", "tls", "flow",
                "client-fingerprint", "servername", "sni", "alpn", "packet-encoding", "interface-name", "dialer-proxy", "ip-version"] {
                if let Some(value) = node.get(field) { safe.insert(field.into(), value.clone()); }
            }
            Value::Object(safe)
        }).collect();
        serde_json::json!({"name":sub.name,"updatedAt":sub.updated_at,"nodes":nodes})
    }).collect();
    serde_json::json!({"subscriptions":subscriptions,"routingMode":settings.routing_mode,"defaultRoute":settings.default_route,
        "compiledRules":crate::rules::compile(settings).ok(),"dns":settings.dns,"tunStack":settings.tun_stack,
        "meaning":"Endpoints and transport settings, not credentials. Matching names do not imply matching endpoints."})
}

pub(crate) fn collect_secrets(value: &Value, result: &mut Vec<String>) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                let key = key.to_ascii_lowercase();
                if [
                    "password",
                    "uuid",
                    "token",
                    "secret",
                    "key",
                    "authorization",
                ]
                .iter()
                .any(|k| key.contains(k))
                {
                    if let Some(s) = value.as_str().filter(|s| !s.is_empty()) {
                        result.push(s.to_owned());
                    }
                }
                collect_secrets(value, result);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_secrets(value, result);
            }
        }
        _ => {}
    }
}

pub(crate) fn redact(text: &str, secrets: &[String]) -> String {
    let mut output = text.to_owned();
    let mut secrets = secrets.to_vec();
    secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
    for secret in secrets {
        if !secret.is_empty() {
            output = output.replace(&secret, "[REDACTED]");
        }
    }
    for (pattern, replacement) in [
        (r#"(?i)\b[a-z][a-z0-9+.-]*://[^\s<>\"']+"#, "[URL REDACTED]"),
        (
            r"(?i)\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b",
            "[UUID REDACTED]",
        ),
        (
            r#"(?i)(?:authorization|password|token|secret|api[_-]?key)\s*[\"']?\s*[:=]\s*(?:\"[^\"]*\"|'[^']*'|(?:Bearer\s+)?[^\s,;]+)"#,
            "[CREDENTIAL REDACTED]",
        ),
    ] {
        output = regex::Regex::new(pattern)
            .unwrap()
            .replace_all(&output, replacement)
            .into_owned();
    }
    if let Ok(profile) = std::env::var("USERPROFILE") {
        output = output.replace(&profile, "%USERPROFILE%");
    }
    output
}

fn reader(stream: impl Read + Send + 'static) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut data = Vec::new();
        let _ = stream.take(LIMIT).read_to_end(&mut data);
        let mut text = String::from_utf8_lossy(&data).into_owned();
        if data.len() == LIMIT as usize {
            text.push_str("\n[Output truncated]\n");
        }
        let _ = tx.send(text);
    });
    rx
}

fn run_bounded(mut command: Command, timeout: Duration) -> Result<String, String> {
    let child = command
        .creation_flags(0x08000000)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    collect_child(child, timeout)
}

fn collect_child(mut child: std::process::Child, timeout: Duration) -> Result<String, String> {
    let job = match crate::job::Job::attach(&child) {
        Ok(job) => job,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    let stdout = reader(child.stdout.take().unwrap());
    let stderr = reader(child.stderr.take().unwrap());
    let start = Instant::now();
    let result = loop {
        match child.try_wait() {
            Ok(Some(status)) => break format!("Collector exit: {status}"),
            Err(error) => break format!("Collector error: {error}"),
            _ if start.elapsed() >= timeout => {
                break "Collector timed out; partial report follows.".into()
            }
            _ => std::thread::sleep(Duration::from_millis(30)),
        }
    };
    drop(job); // Also close pipes held by descendants of this collector only.
    let _ = child.wait();
    Ok(format!(
        "{result}\n{}\n{}",
        stdout
            .recv_timeout(Duration::from_secs(1))
            .unwrap_or_default(),
        stderr
            .recv_timeout(Duration::from_secs(1))
            .unwrap_or_default()
    ))
}

pub(crate) fn save(
    context: String,
    client: Option<crate::core::ApiClient>,
    mut secrets: Vec<String>,
    access: Access,
) -> Result<bool, String> {
    static BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if BUSY.swap(true,std::sync::atomic::Ordering::SeqCst) { return Err("Отчёт уже собирается".into()); }
    struct Release;
    impl Drop for Release { fn drop(&mut self) { BUSY.store(false,std::sync::atomic::Ordering::SeqCst); } }
    let _release = Release;
    let path = rfd::FileDialog::new()
        .set_title("Сохранить диагностику Atlas")
        .set_file_name(format!("atlas-diagnostics-{}.txt", crate::model::now()))
        .add_filter("Текстовый отчёт", &["txt"])
        .save_file();
    let Some(path) = path else {
        return Ok(false);
    };
    let started = crate::model::now();
    let history = crate::incident_history::snapshot();
    let deadline = Instant::now() + Duration::from_secs(85);
    let (tx, rx) = mpsc::channel();
    let os_tx = tx.clone();
    std::thread::spawn(move || {
        let mut command = Command::new("powershell.exe");
        command.args(["-NoLogo","-NoProfile","-NonInteractive","-Command",include_str!("support_snapshot.ps1")]);
        let result = run_bounded(command,Duration::from_secs(25));
        let _ = os_tx.send(("windows",serde_json::json!({"text":result.unwrap_or_else(|e|format!("Windows snapshot unavailable: {e}"))})));
    });
    let privileged_tx = tx.clone();
    let privileged_client = client.clone();
    std::thread::spawn(move || {
        let result = privileged_client.map(|c|c.support_snapshot()).unwrap_or_else(||Err("No service client captured".into()));
        let _ = privileged_tx.send(("privileged",result.unwrap_or_else(|e|serde_json::json!({"error":e}))));
    });
    std::thread::spawn(move || {
        let _ = tx.send(("before",core_snapshot(client.as_ref())));
        if Instant::now() < deadline { let _ = tx.send(("active",crate::support_probes::run(client.clone(),deadline))); }
        if Instant::now() < deadline { let _ = tx.send(("after",core_snapshot(client.as_ref()))); }
    });
    let mut parts = serde_json::Map::new();
    while parts.len() < 5 {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok((name,value)) => { parts.insert(name.into(),value); }
            Err(_) => break,
        }
    }
    let missing: Vec<_> = ["windows","privileged","before","active","after"].into_iter().filter(|k|!parts.contains_key(*k)).collect();
    let final_context = access.get().map(|cached| {
        secrets.extend(cached.secrets);
        serde_json::json!({"capturedAt":cached.captured_at,"revision":cached.revision,"configuration":cached.configuration})
    }).unwrap_or_else(||serde_json::json!({"error":"No final cached context"}));
    let analysis = conclusions(&parts);
    let evidence = serde_json::to_string_pretty(&parts).map_err(|e|e.to_string())?;
    let text = format!(
        "Atlas incident report / schema 3\nVersion: {}\nStarted (Unix UTC): {started}\nCompleted: {}\n\
        Limited active probes requested by export. Selectors, routes, DNS settings and WFP policy were not changed.\n\
        Probe histories can change during URL tests and automatic recovery can still run. Samples are timestamped, not atomic.\n\
        Missing sections: {missing:?}\n\n=== Confirmed observations and interpretation ===\n{}\n\n\
        === Atlas state at request ===\n{context}\n\n=== Last cached configuration at completion (compare revisions) ===\n{final_context}\n\n=== History before export ===\n{history}\n\n=== Incident evidence ===\n{evidence}\n",
        env!("CARGO_PKG_VERSION"),
        crate::model::now(),serde_json::to_string_pretty(&analysis).map_err(|e|e.to_string())?
    );
    std::fs::write(path, format!("\u{feff}{}", redact(&text, &secrets)))
        .map_err(|e| format!("Не удалось сохранить отчёт: {e}"))?;
    Ok(true)
}

// Whitelist operational fields; never export node configuration or credentials.
fn proxy_evidence(value: &Value) -> Value {
    let mut nodes = serde_json::Map::new();
    if let Some(proxies) = value["proxies"].as_object() {
        for (name, node) in proxies {
            let mut safe = serde_json::Map::new();
            for field in ["type", "now", "all", "alive", "history", "udp", "testUrl", "interface", "dialer-proxy"] {
                if let Some(v) = node.get(field) { safe.insert(field.into(), v.clone()); }
            }
            if let Some(url) = node.get("testUrl").and_then(Value::as_str) {
                safe.insert("testUrlId".into(), Value::String(format!("{:x}", Sha256::digest(url.as_bytes()))));
            }
            if let Some(extra) = node.get("extra").and_then(Value::as_object) {
                let histories: Vec<Value> = extra.iter().map(|(url, state)| {
                    serde_json::json!({"testUrl":url,"testUrlId":format!("{:x}",Sha256::digest(url.as_bytes())),
                        "alive":state.get("alive"),"history":state.get("history")})
                }).collect();
                safe.insert("perUrlHealth".into(), Value::Array(histories));
            }
            nodes.insert(name.clone(), Value::Object(safe));
        }
    }
    Value::Object(nodes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn report_conclusions_do_not_turn_timeouts_or_http_errors_into_provider_outage() {
        let parts = serde_json::json!({"active":{"http":[
            {"path":"local_mixed_proxy","endpoint":"https://test","httpResponded":true,"controlSucceeded":false,"status":502},
            {"path":"windows_default_path","endpoint":"https://test","httpResponded":false,"controlSucceeded":false}],
            "nodeTests":{"results":[{"checks":[{"result":{"error":"Mihomo API: HTTP 504"}}]}]}}});
        let result = conclusions(parts.as_object().unwrap());
        assert_eq!(result.len(),1); // Only scope, no invented diagnosis.
        assert_eq!(result[0]["code"],"EVIDENCE_SCOPE");
        let mut parts = parts;
        parts["active"]["http"][0]["controlSucceeded"] = Value::Bool(true);
        parts["active"]["nodeTests"]["results"][0]["checks"][0]["result"] = serde_json::json!({"delay":0});
        let result = conclusions(parts.as_object().unwrap());
        assert!(result.iter().any(|v|v["code"]=="PATHS_DIFFER"));
        assert!(result.iter().any(|v|v["code"]=="VPN_PATH_WORKS"));
    }
    #[test]
    fn connect_wrapper_is_not_misreported_as_a_tcp_dial_failure() {
        let evidence = failure_evidence(&[
            "error: host connect error: context deadline exceeded".into(),
            "error: host connect error: read tcp 192.0.2.1:5->192.0.2.2:443: timeout".into(),
            "error: host connect error: dial tcp 192.0.2.2:443: i/o timeout".into(),
        ]);
        assert_eq!(evidence["tcpDialErrors"], 1);
        assert_eq!(evidence["establishedSocketReadErrors"], 1);
        assert_eq!(evidence["unspecifiedConnectOrHandshakeErrors"], 1);
    }
    #[test]
    fn endpoint_comparison_omits_credentials_and_subscription_urls() {
        let mut s = crate::model::Settings::default();
        s.subscriptions.push(crate::model::Subscription { id:"id".into(), name:"office".into(),
            masked_url:"https://private-subscription".into(), updated_at:123, error:None,
            servers:vec![serde_json::json!({"name":"node","type":"vless","server":"192.0.2.1","port":443,
                "uuid":"private-uuid","password":"private-pass","reality-opts":{"public-key":"private-key"}})] });
        let evidence = configuration_evidence(&s).to_string();
        assert!(evidence.contains("192.0.2.1"));
        assert!(evidence.contains("123"));
        assert!(!evidence.contains("private-"));
    }
    #[test]
    fn proxy_report_preserves_auto_choice_without_node_secrets() {
        let result = proxy_evidence(&serde_json::json!({"proxies":{
            "ATLAS":{"now":"AUTO", "secret":"hidden"},
            "AUTO":{"now":"node-b","all":["node-a","node-b"],"testUrl":"https://control.test"},
            "node-b":{"alive":true,"history":[{"delay":42}],"password":"hidden",
                "extra":{"https://control.test":{"alive":false,"history":[{"delay":0}],"password":"hidden"}}}
        }}));
        assert_eq!(result["ATLAS"]["now"], "AUTO");
        assert_eq!(result["AUTO"]["now"], "node-b");
        assert_eq!(result["node-b"]["history"][0]["delay"], 42);
        assert_eq!(result["node-b"]["perUrlHealth"][0]["alive"], false);
        assert_eq!(result["AUTO"]["testUrlId"], result["node-b"]["perUrlHealth"][0]["testUrlId"]);
        assert!(!result.to_string().contains("hidden"));
    }
    #[test]
    fn report_hides_credentials_but_keeps_network_evidence() {
        let mut secrets = vec![];
        collect_secrets(
            &serde_json::json!({"nodes":[{"password":"my-password", "uuid":"credential-id", "reality-opts":{"private-key":"private-material"}}]}),
            &mut secrets,
        );
        let output = redact("my-password credential-id private-material https://host/sub?token=abc vless://secret@host Authorization: Bearer xyz 192.168.1.1 DHCP 68 67", &secrets);
        for secret in [
            "my-password",
            "credential-id",
            "private-material",
            "host/sub",
            "secret@host",
            "xyz",
        ] {
            assert!(!output.contains(secret), "{secret}");
        }
        assert!(output.contains("192.168.1.1 DHCP 68 67"));
        let json_log = redact(
            r#"{"authorization": "Bearer hidden-auth", "token":"hidden-token", "password": "a password with spaces"}"#,
            &[],
        );
        for secret in ["hidden-auth", "hidden-token", "a password with spaces"] {
            assert!(!json_log.contains(secret), "{json_log}");
        }
    }
    #[test]
    fn stalled_collector_returns_partial_output_without_hanging() {
        let ready =
            std::env::temp_dir().join(format!("atlas-report-ready-{}", uuid::Uuid::new_v4()));
        let mut command = Command::new("powershell.exe");
        command.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[Console]::WriteLine('fixture snapshot'); [Console]::Out.Flush(); [IO.File]::WriteAllText($env:ATLAS_REPORT_TEST_READY, 'ready'); Start-Sleep -Seconds 60",
        ]);
        let mut child = command
            .env("ATLAS_REPORT_TEST_READY", &ready)
            .creation_flags(0x08000000)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        // CI may take several seconds to start PowerShell. A readiness marker
        // separates that startup from the stalled-collector behavior under test.
        let startup = Instant::now();
        while !ready.exists() {
            if startup.elapsed() >= Duration::from_secs(20) {
                let _ = child.kill();
                let _ = child.wait();
                panic!("fixture did not become ready before startup deadline");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let started = Instant::now();
        let output = collect_child(child, Duration::from_secs(2)).unwrap();
        std::fs::remove_file(ready).unwrap();
        assert!(output.contains("timed out"));
        assert!(output.contains("fixture snapshot"));
        assert!(started.elapsed() < Duration::from_secs(15));
    }

    #[test]
    fn collector_can_time_out_before_producing_any_output() {
        let mut command = Command::new("powershell.exe");
        command.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Start-Sleep -Seconds 60",
        ]);
        let output = run_bounded(command, Duration::ZERO).unwrap();
        assert!(output.contains("timed out"));
        assert!(!output.contains("fixture snapshot"));
    }

    #[test]
    fn snapshot_keeps_dhcp_and_service_data_when_a_section_fails() {
        // Replace every networking cmdlet. This test must never inspect the host.
        let stubs = r#"
function Get-CimInstance { param($ClassName,$Filter)
 if ($ClassName -eq 'Win32_Service') { [pscustomobject]@{Name='AtlasNetworkService'; State='Running'; ProcessId=123} }
 elseif ($ClassName -eq 'Win32_Process') { [pscustomobject]@{Name='mihomo.exe'; ProcessId=456; ParentProcessId=123} }
 else { [pscustomobject]@{Description='fixture'; DHCPEnabled=$true; DHCPServer='192.168.1.1'; DHCPLeaseObtained='2026-09-22T01:00:00'; DHCPLeaseExpires='2026-09-23T01:00:00'} }
}
function Get-NetAdapter { param([switch]$IncludeHidden) [pscustomobject]@{Name='fixture';Status='Up'} }
function Get-NetAdapterStatistics { [pscustomobject]@{Name='fixture';ReceivedPacketErrors=0} }
function Get-NetNeighbor { [pscustomobject]@{InterfaceIndex=4;IPAddress='192.168.1.1';State='Reachable'} }
function Get-NetConnectionProfile { [pscustomobject]@{InterfaceAlias='fixture';IPv4Connectivity='Internet'} }
function Get-Process { [pscustomobject]@{ProcessName='atlas-vpn';Id=123;CPU=0;Threads=@()} }
function Get-DnsClientServerAddress { [pscustomobject]@{InterfaceAlias='fixture';ServerAddresses=@('192.168.1.1')} }
function Get-NetRoute { throw 'fixture route unavailable' }
function Get-NetTCPConnection { param($State) [pscustomobject]@{OwningProcess=456;LocalAddress='192.168.1.2';LocalPort=53;RemoteAddress='192.0.2.1';State='SynSent'} }
function Get-NetUDPEndpoint { [pscustomobject]@{OwningProcess=456;LocalAddress='::';LocalPort=53} }
function Get-ItemProperty { param($Path) [pscustomobject]@{ProxyEnable=0;ProxyServer='127.0.0.1:7897'} }
function w32tm.exe { 'fixture time synchronization' }
function Find-NetRoute { param($RemoteIPAddress) [pscustomobject]@{InterfaceIndex=4;NextHop='192.168.1.1'} }
function Get-NetIPInterface { [pscustomobject]@{InterfaceAlias='fixture';Dhcp='Enabled'} }
function Get-WinEvent { param($FilterHashtable,$MaxEvents) [pscustomobject]@{Id=1001;Message='fixture DHCP event'} }
"#;
        let script = format!("{stubs}\n{}", include_str!("support_snapshot.ps1"));
        let mut command = Command::new("powershell.exe");
        command.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
        let output = run_bounded(command, Duration::from_secs(8)).unwrap();
        for expected in [
            "AtlasNetworkService",
            "mihomo.exe",
            "2026-09-23T01:00:00",
            "192.168.1.1",
            "fixture route unavailable",
            "Interface metrics and DHCP state",
            "fixture DHCP event",
        ] {
            assert!(output.contains(expected), "missing {expected}: {output}");
        }
    }
}

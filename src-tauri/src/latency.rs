use crate::{core::{ApiClient, Core}, model::Settings};
use serde::Serialize;
use serde_json::{json, Value};
use std::path::PathBuf;

pub const DISPLAY_URL: &str = "http://cp.cloudflare.com/generate_204";
pub const DISPLAY_TIMEOUT_MS: u64 = 10000;
pub(crate) const DISPLAY_WORKERS: usize = 64;
/// Opaque identity of actual node parameters, independent of UI preferences,
/// selected route and subscription refresh timestamps. Never expose credentials.
pub fn node_identity(node: &Value) -> String {
    use sha2::{Digest, Sha256};
    let mut node=node.clone();
    if let Some(fields)=node.as_object_mut() {fields.remove("extraParams");fields.remove("atlas");}
    format!("{:x}", Sha256::digest(node.to_string().as_bytes()))
}
// Use the same per-node URL measurement as Clash Verge, with bounded concurrency.
pub fn batch_stream(client: ApiClient, names: &[String], progress: &(dyn Fn(&str, &Value) + Sync)) -> Result<Value, String> {
    display_batch_stream(client, names, DISPLAY_URL, DISPLAY_TIMEOUT_MS, progress)
}

/// Run a disposable, unprivileged Mihomo instance for URL tests while Atlas
/// is disconnected. It binds only random loopback ports; it never enables TUN
/// or changes the user's proxy, routes, DNS settings, or WFP policy.
fn with_offline_core<T>(settings: Settings, binary: PathBuf, parent: PathBuf,
    run: impl FnOnce(ApiClient) -> Result<T, String>) -> Result<T, String> {
    static STARTUP: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    let _startup = STARTUP.get_or_init(|| std::sync::Mutex::new(())).lock()
        .map_err(|_| "Проверка узлов временно недоступна".to_owned())?;
    let directory = parent.join(format!("latency-probe-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).map_err(|e| format!("Каталог проверки: {e}"))?;
    let mut core = Core::new(binary, directory.clone());
    let mut probe_settings = settings;
    probe_settings.mode = "system".into();
    // A saved manual pin can be stale after subscription replacement. URL
    // tests address concrete nodes and do not depend on the top-level choice.
    probe_settings.selected = "AUTO".into();
    let result = core.use_ephemeral_ports()
        .and_then(|_| core.start(&probe_settings))
        .and_then(|_| run(core.client()));
    let stopped = core.stop();
    drop(core);
    // Keep the work directory as evidence if termination failed; removing it
    // while a probe core may still be alive would hide an incomplete cleanup.
    let removed = if stopped.is_ok() {
        std::fs::remove_dir_all(&directory).map_err(|e| format!("Очистка проверки: {e}"))
    } else { Ok(()) };
    match (result, stopped, removed) {
        (Ok(value), Ok(()), Ok(())) => Ok(value),
        (result, stopped, removed) => {
            let mut errors = Vec::new();
            if let Err(error) = result { errors.push(error); }
            if let Err(error) = stopped { errors.push(format!("Остановка проверочного ядра: {error}")); }
            if let Err(error) = removed { errors.push(error); }
            Err(errors.join("; "))
        }
    }
}

pub fn offline_batch_stream(settings: Settings, binary: PathBuf, parent: PathBuf,
    names: &[String], progress: &(dyn Fn(&str, &Value) + Sync)) -> Result<Value, String> {
    with_offline_core(settings, binary, parent, |client| batch_stream(client, names, progress))
}

pub fn offline_test(settings: Settings, binary: PathBuf, parent: PathBuf,
    name: &str) -> TestResult {
    with_offline_core(settings, binary, parent, |client| Ok(test(client, name)))
        .unwrap_or_else(|error| TestResult {
            status: "error".into(), delay: None, attempts: 0, error: Some(error),
        })
}
#[cfg(test)]
pub(crate) fn display_batch_at(client: ApiClient, names: &[String], endpoint: &str, timeout: u64) -> Result<Value, String> {
    display_batch_stream(client, names, endpoint, timeout, &|_, _| {})
}

fn display_batch_stream(client: ApiClient, names: &[String], endpoint: &str, timeout: u64,
    progress: &(dyn Fn(&str, &Value) + Sync)) -> Result<Value, String> {
    let proxies = client.api("GET", "/proxies", None).unwrap_or(Value::Null);
    let names = discovery_order(names, &proxies, endpoint);
    if let Some(broker) = client.broker_client() {
        return service_batch_stream(&broker, &names, endpoint, timeout, progress);
    }
    // The caller already queues the full list. Publish only starts and results.
    Ok(discover_batch(&names, timeout, &|name, deadline| {
        progress(name,&json!({"status":"testing","delay":null,"attempts":0}));
        display_probe(&client, name, endpoint, deadline)
    }, progress))
}

// One IPC request submits a set of independent tests; one poll collects their
// completed replies. Do not serialize a round trip for every in-flight node.
pub(crate) fn service_batch_stream(broker: &crate::broker::Broker, names: &[String], endpoint: &str, timeout: u64,
    progress: &(dyn Fn(&str, &Value) + Sync)) -> Result<Value,String> {
    use std::time::{Duration, Instant};
    let mut next = 0;
    let mut pending = std::collections::BTreeMap::<String,(String,Instant)>::new();
    let mut results = serde_json::Map::new();
    let mut publish = |name: &str, reply: Result<Value,String>| {
        let result = display_attempt(endpoint, |_| reply.clone().and_then(|value|
            value["delay"].as_u64().ok_or("Ядро не вернуло задержку".into())));
        let mut value = serde_json::to_value(result).unwrap();
        stamp_result(&mut value);
        progress(name, &value);
        results.insert(name.to_owned(), value);
    };
    while next < names.len() || !pending.is_empty() {
        let end = (next + DISPLAY_WORKERS - pending.len()).min(names.len());
        if end > next {
            let paths: Vec<_> = names[next..end].iter().map(|name|display_path(name, endpoint, timeout)).collect();
            let jobs = crate::query_admission::retry_busy(Instant::now()+Duration::from_secs(5), ||
                broker.call("delay_many", json!({"paths":paths})))?;
            let jobs = jobs.as_array().filter(|jobs|jobs.len()==end-next).ok_or("Некорректный ответ очереди проверок")?;
            for (name, job) in names[next..end].iter().zip(jobs) {
                if let Some(id) = job["id"].as_str() {
                    progress(name, &json!({"status":"testing","delay":null,"attempts":0}));
                    pending.insert(id.to_owned(), (name.clone(), Instant::now()));
                } else { publish(name, Err(job["error"].as_str().unwrap_or("Проверка не запущена").to_owned())); }
            }
            next = end;
        }
        if pending.is_empty() { continue; }
        let ids: Vec<_> = pending.keys().cloned().collect();
        let replies = crate::query_admission::retry_busy(Instant::now()+Duration::from_secs(5), ||
            broker.call("delay_results", json!({"ids":ids})))?;
        let replies = replies.as_array().ok_or("Некорректный ответ проверок")?;
        for reply in replies {
            if reply["done"] != true { continue; }
            let Some((name, _)) = reply["id"].as_str().and_then(|id|pending.remove(id)) else { continue; };
            publish(&name, match reply["error"].as_str() {
                Some(error) => Err(error.to_owned()), None => Ok(reply["value"].clone()),
            });
        }
        let expired: Vec<_> = pending.iter().filter(|(_,(_,at))|at.elapsed()>=Duration::from_secs(40))
            .map(|(id,_)|id.clone()).collect();
        for id in expired {
            let (name,_) = pending.remove(&id).unwrap();
            publish(&name, Err("Проверка сервера превысила время ожидания".into()));
        }
        if !pending.is_empty() && (next == names.len() || pending.len() == DISPLAY_WORKERS) {
            std::thread::sleep(Duration::from_millis(30));
        }
    }
    Ok(Value::Object(results))
}

fn stamp_result(value: &mut Value) {
    value["measuredAt"]=json!(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis());
    value["measurement"]=json!("http_via_node_ms");
}

fn discovery_order(names: &[String], proxies: &Value, endpoint: &str) -> Vec<String> {
    discovery_order_with_workers(names, proxies, endpoint, DISPLAY_WORKERS)
}

fn discovery_order_with_workers(names: &[String], proxies: &Value, endpoint: &str, workers: usize) -> Vec<String> {
    // Sample across the whole list: subscription tails must not sit behind
    // dozens of silently dropped connections, even on the first-ever check.
    let width = names.len().div_ceil(workers).max(1);
    let mut ordered = Vec::with_capacity(names.len());
    for offset in 0..width {
        for lane in 0..workers {
            if let Some(name) = names.get(lane * width + offset) { ordered.push(name.clone()); }
        }
    }
    let mut selected = "ATLAS";
    for _ in 0..8 {
        let Some(next) = proxies["proxies"][selected]["now"].as_str() else { break; };
        if next == selected { break; }
        selected = next;
    }
    ordered.sort_by_key(|name| if name == selected { 0 } else if
        proxies["proxies"][name]["extra"][endpoint]["alive"] == true { 1 } else { 2 });
    ordered
}

fn discover_batch(names: &[String], timeout: u64,
    probe: &(dyn Fn(&str, u64) -> TestResult + Sync),
    progress: &(dyn Fn(&str, &Value) + Sync)) -> Value {
    // One real request per node, with the full network deadline. A short pass
    // followed by full retries repeats connection setup and delays completion.
    run_display_batch(names, &|name|
        serde_json::to_value(probe(name, timeout)).unwrap(), progress)
}

fn run_display_batch(names: &[String], probe: &(dyn Fn(&str) -> Value + Sync),
    progress: &(dyn Fn(&str, &Value) + Sync)) -> Value {
    run_display_batch_with_workers(names, DISPLAY_WORKERS, probe, progress)
}

fn run_display_batch_with_workers(names: &[String], workers: usize, probe: &(dyn Fn(&str) -> Value + Sync),
    progress: &(dyn Fn(&str, &Value) + Sync)) -> Value {
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results = std::sync::Mutex::new(serde_json::Map::new());
    std::thread::scope(|scope| {
        // The shared admission gate reserves additional capacity for recovery.
        for _ in 0..names.len().min(workers) {
            let (next, results) = (&next, &results);
            scope.spawn(move || {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(name) = names.get(i) else { break; };
                    let mut value = probe(name);
                    stamp_result(&mut value);
                    results.lock().unwrap().insert(name.clone(), value.clone());
                    // Publish this node before waiting for any other worker.
                    progress(name, &value);
                }
            });
        }
    });
    Value::Object(results.into_inner().unwrap())
}
#[cfg(test)]
pub(crate) fn batch_at(client: ApiClient, names: &[String], endpoint: &str) -> Result<Value, String> {
    client.api("GET", "/version", None)?;
    let url: String = url::form_urlencoded::byte_serialize(endpoint.as_bytes()).collect();
    let delays = match client.api("GET", &format!("/group/AUTO/delay?timeout=5000&expected=204&url={url}"), None) {
        Ok(value) if value.is_object() => value,
        Ok(_) => return Err("Некорректный ответ групповой проверки".into()),
        Err(error) if error == "Mihomo API: HTTP 504" => json!({}),
        Err(error) => return Err(error),
    };
    let health = client.api("GET", "/proxies", None)?;
    let mut results = serde_json::Map::new();
    for name in names {
        let result = match delays.get(name).filter(|_|health["proxies"][name]["extra"][endpoint]["alive"] == true) {
            Some(delay) => {
                let delay = delay.as_u64().ok_or("Некорректная задержка ядра")?;
                json!({"status":"ok","delay":delay,"attempts":1})
            }
            None => json!({"status":"unreachable","delay":null,"attempts":1,
                "error":"Контрольный URL не подтвердил HTTP 204 через сервер: таймаут, ошибка соединения или другой HTTP-статус"}),
        };
        results.insert(name.clone(), result);
    }
    Ok(Value::Object(results))
}
// Independent provider; Android/Chromium also use this HTTP 204 control.
pub const SECONDARY_URL: &str = "https://www.google.com/generate_204";
pub const ENDPOINTS: [&str; 2] = [DISPLAY_URL, SECONDARY_URL];
pub fn expected_status(_endpoint: &str) -> u16 {
    204
}
pub(crate) fn verified_probe(client: &ApiClient, name: &str, endpoint: &str) -> Result<Value,String> {
    verified_probe_with(client, name, endpoint, DISPLAY_TIMEOUT_MS, None)
}
pub(crate) fn verified_probe_with(client: &ApiClient, name: &str, endpoint: &str, timeout: u64,
    cancelled: Option<&std::sync::atomic::AtomicBool>) -> Result<Value,String> {
    let started=std::time::Instant::now(); let id=uuid::Uuid::new_v4().to_string();
    client.event(json!({"at":crate::model::now(),"kind":"ProbeStarted","probeId":id,"name":name,"timeoutMs":timeout}));
    let mut result=verified_probe_inner(client,name,endpoint,timeout,cancelled);
    if let Ok(value)=&mut result { value["probeId"]=json!(id); }
    client.event(json!({"at":crate::model::now(),"kind":if result.as_ref().err().is_some_and(|e|e=="Mihomo API: HTTP 504") {"ProbeTimeout"} else {"ProbeCompleted"},
        "probeId":id,"name":name,"success":result.is_ok(),"elapsedMs":started.elapsed().as_millis(),"measurement":"http_via_node_ms"}));
    result
}
fn verified_probe_inner(client: &ApiClient, name: &str, endpoint: &str, timeout: u64,
    cancelled: Option<&std::sync::atomic::AtomicBool>) -> Result<Value,String> {
    let path = format!("/proxies/{}/delay?timeout={timeout}&expected={}&url={}",encode_name(name),expected_status(endpoint),encode_name(endpoint));
    let result = client.api_cancellable("GET",&path,None,cancelled)?;
    let proxies = client.api_cancellable("GET","/proxies",None,cancelled)?;
    // Mihomo returns a delay even when expected HTTP status did not match.
    // Its per-URL alive flag includes that status check; the delay alone does not.
    let health = &proxies["proxies"][name]["extra"][endpoint];
    if health["alive"] != true {
        client.event(json!({"kind":"control_status_rejected","at":crate::model::now(),"name":name,
            "controlHost":url::Url::parse(endpoint).ok().and_then(|u|u.host_str().map(str::to_owned)),
            "expectedStatus":expected_status(endpoint),"returnedDelay":result["delay"],"health":health}));
        return Err(format!("Контрольный URL не подтвердил ожидаемый HTTP {} (per-URL health); returnedDelay={}, perUrlAlive={}",
            expected_status(endpoint),result["delay"],health["alive"]));
    }
    if result["delay"].as_u64().is_none() { return Err("Ядро не вернуло задержку".into()); }
    Ok(json!({"delay":result["delay"],"expectedStatusMatched":true,"health":health}))
}
pub(crate) fn verified_batch(client: ApiClient, names: &[String], endpoint: &str) -> serde_json::Map<String,Value> {
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results = std::sync::Mutex::new(serde_json::Map::new());
    std::thread::scope(|scope| {
        for _ in 0..names.len().min(6) {
            let (client,next,results)=(&client,&next,&results);
            scope.spawn(move || loop {
                let index=next.fetch_add(1,std::sync::atomic::Ordering::Relaxed);
                let Some(name)=names.get(index) else { break; };
                let result=verified_probe(client,name,endpoint)
                    .unwrap_or_else(|error|json!({"error":error}));
                results.lock().unwrap().insert(name.clone(),result);
            });
        }
    });
    results.into_inner().unwrap()
}
#[derive(Serialize, Debug)]
pub struct TestResult {
    pub status: String,
    pub delay: Option<u64>,
    pub attempts: u8,
    pub error: Option<String>,
}
pub fn test(client: ApiClient, name: &str) -> TestResult {
    let mut result = display_probe(&client, name, DISPLAY_URL, DISPLAY_TIMEOUT_MS);
    if result.status == "unreachable" && client.api("GET", "/version", None).is_err() {
        result.status = "error".into();
        result.error = Some("Связь с ядром прервалась во время проверки".into());
    }
    result
}
pub(crate) fn display_probe(client: &ApiClient, name: &str, endpoint: &str, timeout: u64) -> TestResult {
    display_attempt(endpoint, |url| {
        let path = display_path(name, url, timeout);
        client.api("GET", &path, None).and_then(|v|v["delay"].as_u64().ok_or("Ядро не вернуло задержку".into()))
    })
}
fn display_path(name: &str, endpoint: &str, timeout: u64) -> String {
    format!("/proxies/{}/delay?timeout={timeout}&url={}", encode_name(name), encode_name(endpoint))
}
fn display_attempt(endpoint: &str, mut probe: impl FnMut(&str)->Result<u64,String>) -> TestResult {
    match probe(endpoint) {
        Ok(delay) => TestResult {status:"ok".into(),delay:Some(delay),attempts:1,error:None},
        Err(error) => TestResult {status:if error=="Mihomo API: HTTP 504" {"timeout"} else if error=="Mihomo API: HTTP 503" {"unreachable"} else {"error"}.into(),
            delay:None,attempts:1,error:Some(error)},
    }
}
pub(crate) fn encode_name(name: &str) -> String {
    // Form encoding uses '+' for spaces, but a URL path requires '%20'.
    url::form_urlencoded::byte_serialize(name.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Subscription;

    // Frozen scheduling policy from the working tree before this fix. Used only
    // by the opt-in network comparison, never by application code.
    fn before_batch(client: ApiClient, names: &[String], progress: &(dyn Fn(&str, &Value) + Sync)) -> Result<Value,String> {
        let proxies = client.api("GET", "/proxies", None).unwrap_or(Value::Null);
        let names = discovery_order_with_workers(names, &proxies, DISPLAY_URL, 6);
        let next = std::sync::atomic::AtomicUsize::new(0);
        let results = std::sync::Mutex::new(serde_json::Map::new());
        for name in &names { progress(name, &json!({"status":"queued","delay":null,"attempts":0})); }
        std::thread::scope(|scope| {
            for worker in 0..names.len().min(6) {
                let (next, results, names, client) = (&next, &results, &names, &client);
                scope.spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(worker as u64 * 20));
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(name) = names.get(i) else { break; };
                        progress(name, &json!({"status":"testing","delay":null,"attempts":0}));
                        let value = serde_json::to_value(display_probe(client, name, DISPLAY_URL, 1500)).unwrap();
                        results.lock().unwrap().insert(name.clone(), value.clone());
                        progress(name, &value);
                    }
                });
            }
        });
        Ok(Value::Object(results.into_inner().unwrap()))
    }
    #[test]
    #[ignore = "Opt-in real network benchmark; ATLAS_BATCH_DATABASE is opened read-only"]
    fn real_batch_profile() {
        let database = std::env::var("ATLAS_BATCH_DATABASE").expect("Set ATLAS_BATCH_DATABASE");
        let db = rusqlite::Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let encrypted: Vec<u8> = db.query_row("SELECT payload FROM state WHERE id=1", [], |r|r.get(0)).unwrap();
        let settings: Settings = serde_json::from_slice(&crate::storage::crypt(&encrypted, false).unwrap()).unwrap();
        let nodes = settings.servers();
        let names: Vec<_> = nodes.iter().filter_map(|n|n["name"].as_str())
            .map(str::to_owned).collect();
        assert!(!names.is_empty());
        let start = std::time::Instant::now();
        let first = std::sync::Mutex::new(None);
        let progress = std::sync::Mutex::new(std::collections::HashSet::new());
        let binary = std::env::var_os("ATLAS_BATCH_CORE").map(PathBuf::from)
            .unwrap_or_else(||PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/Atlas.Core.exe"));
        let policy = std::env::var("ATLAS_BATCH_POLICY").unwrap_or_else(|_|"after".into());
        assert!(matches!(policy.as_str(), "before" | "after"));
        let progress_callback = |name: &str, value: &Value| {
            if value["status"] != "queued" && value["status"] != "testing" {
                first.lock().unwrap().get_or_insert(start.elapsed().as_millis());
                progress.lock().unwrap().insert(name.to_owned());
            }
        };
        let results = with_offline_core(settings, binary, std::env::temp_dir(), |client| {
            if policy == "before" { before_batch(client, &names, &progress_callback) }
            else { batch_stream(client, &names, &progress_callback) }
        }).unwrap();
        let elapsed = start.elapsed().as_millis();
        assert_eq!(results.as_object().unwrap().len(), names.len());
        assert_eq!(progress.into_inner().unwrap().len(), names.len());
        let mut counts = std::collections::BTreeMap::<String,usize>::new();
        let mut outcomes = serde_json::Map::new();
        for (name, result) in results.as_object().unwrap() {
            *counts.entry(result["status"].as_str().unwrap().to_owned()).or_default() += 1;
            outcomes.insert(node_identity(&json!(name)), json!({"status":result["status"],"delay":result["delay"],"attempts":result["attempts"]}));
        }
        println!("REAL_BATCH {}", json!({"policy":policy,"nodes":nodes.len(),"tested":names.len(),"dataset":node_identity(&json!(nodes)),
            "fullMs":elapsed,"firstMs":first.into_inner().unwrap(),"counts":counts,"outcomes":outcomes}));
    }
    #[test]
    #[ignore = "Explicit scheduler load profile; simulated network waits, no user configuration"]
    fn scheduler_load_profile() {
        use std::sync::{Mutex,atomic::{AtomicUsize,Ordering}};
        fn percentile(values: &mut [u128], p: usize) -> u128 {values.sort_unstable();values[((values.len()-1)*p).div_ceil(100)]}
        for size in [10,50,100,500,1000] {
            for (policy,workers) in [("baseline_six_workers",6),("full_deadline_parallel",DISPLAY_WORKERS)] {
                let mut totals=Vec::new();let mut firsts=Vec::new();let mut latencies=Vec::new();let mut peak=0;
                for _ in 0..10 {
                    let names:Vec<_>=(0..size).map(|i|i.to_string()).collect();
                    let active=AtomicUsize::new(0);let maximum=AtomicUsize::new(0);
                    let first=Mutex::new(None);let completions=Mutex::new(Vec::new());let start=std::time::Instant::now();
                    let result=run_display_batch_with_workers(&names,workers,&|name| {
                        let n=name.parse::<usize>().unwrap();
                        let count=active.fetch_add(1,Ordering::SeqCst)+1;maximum.fetch_max(count,Ordering::SeqCst);
                        // Deliberately scaled scheduler workload: 20% failures.
                        std::thread::sleep(std::time::Duration::from_millis(if n%5==0 {20} else {1}));
                        active.fetch_sub(1,Ordering::SeqCst);
                        json!({"status":if n%5==0 {"timeout"} else {"ok"},"delay":if n%5==0 {None}else{Some(80)},"attempts":1})
                    },&|_,v| {
                        let elapsed=start.elapsed().as_micros();completions.lock().unwrap().push(elapsed);
                        if v["status"]=="ok" { first.lock().unwrap().get_or_insert(elapsed); }
                    });
                    assert_eq!(result.as_object().unwrap().len(),size);assert!(maximum.load(Ordering::SeqCst)<=workers);
                    peak=peak.max(maximum.load(Ordering::SeqCst));totals.push(start.elapsed().as_micros());firsts.push(first.into_inner().unwrap().unwrap());latencies.extend(completions.into_inner().unwrap());
                }
                println!("LOAD {}",json!({"nodes":size,"policy":policy,"runs":10,"maxWorkers":peak,
                    "firstP50Us":percentile(&mut firsts,50),"firstP95Us":percentile(&mut firsts,95),
                    "fullP50Us":percentile(&mut totals,50),"fullP95Us":percentile(&mut totals,95),"fullP99Us":percentile(&mut totals,99),"fullMaxUs":totals.iter().max(),
                    "completionP95Us":percentile(&mut latencies,95),"completionP99Us":percentile(&mut latencies,99)}));
            }
        }
    }
    #[test]
    fn discovery_samples_subscription_tails_and_prioritizes_known_working_nodes() {
        let names: Vec<_> = (0..174).map(|i|format!("node-{i}")).collect();
        let cold = discovery_order(&names, &Value::Null, DISPLAY_URL);
        assert_eq!(&cold[..6], &["node-0", "node-3", "node-6", "node-9", "node-12", "node-15"]);
        let mut sorted = cold.clone(); sorted.sort();
        let mut expected = names.clone(); expected.sort(); assert_eq!(sorted, expected);
        let proxies = json!({"proxies":{
            "ATLAS":{"now":"AUTO"}, "AUTO":{"now":"node-173"},
            "node-172":{"extra":{DISPLAY_URL:{"alive":true}}}
        }});
        let warm = discovery_order(&names, &proxies, DISPLAY_URL);
        assert_eq!(&warm[..2], &["node-173", "node-172"]);
        assert!(discovery_order(&[], &Value::Null, DISPLAY_URL).is_empty());
    }
    #[test]
    fn every_node_gets_one_full_deadline_and_one_final_result() {
        use std::sync::{Mutex,atomic::{AtomicUsize,Ordering}};
        let names: Vec<_> = (0..18).map(|i|i.to_string()).collect();
        let checked = AtomicUsize::new(0);
        let published = Mutex::new(Vec::new());
        let result = discover_batch(&names, DISPLAY_TIMEOUT_MS, &|name, timeout| {
            checked.fetch_add(1,Ordering::SeqCst);
            assert_eq!(timeout, DISPLAY_TIMEOUT_MS);
            if name == "17" || (name == "16" && timeout == DISPLAY_TIMEOUT_MS) {
                TestResult { status:"ok".into(),delay:Some(80),attempts:1,error:None }
            } else if name == "15" {
                TestResult { status:"error".into(),delay:None,attempts:1,error:Some("local IPC".into()) }
            } else {
                TestResult { status:"timeout".into(),delay:None,attempts:1,error:Some("Mihomo API: HTTP 504".into()) }
            }
        }, &|name,value|published.lock().unwrap().push((name.to_owned(),value.clone())));
        let published = published.into_inner().unwrap();
        assert_eq!(checked.load(Ordering::SeqCst), names.len());
        assert_eq!(published.len(), names.len());
        assert_eq!(result["17"]["attempts"],1);
        assert_eq!(result["16"]["status"],"ok");
        assert_eq!(result["16"]["attempts"],1);
        assert_eq!(result["0"]["status"],"timeout");
        assert_eq!(result["0"]["attempts"],1);
        assert_eq!(result["15"]["status"],"error");
        assert_eq!(result["15"]["attempts"],1);
        assert!(published.iter().all(|(_,v)|v["attempts"]==1));
    }
    #[test]
    fn batch_keeps_a_real_controller_reply_that_takes_longer_than_short_discovery() {
        use std::{io::{Read,Write}, net::TcpListener, time::{Duration,Instant}};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let client = ApiClient::loopback_fixture(listener.local_addr().unwrap().port());
        let server = std::thread::spawn(move || {
            std::thread::scope(|scope| {
                for _ in 0..3 {
                    let deadline = Instant::now()+Duration::from_secs(5);
                    let (mut stream,_) = loop {
                        match listener.accept() {
                            Ok(connection) => break connection,
                            Err(e) if e.kind()==std::io::ErrorKind::WouldBlock && Instant::now()<deadline =>
                                std::thread::sleep(Duration::from_millis(5)),
                            Err(e) => panic!("controller fixture: {e}"),
                        }
                    };
                    scope.spawn(move || {
                        stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                        let mut bytes = [0;4096];
                        let len = stream.read(&mut bytes).unwrap();
                        let request = String::from_utf8_lossy(&bytes[..len]);
                        let body = if request.starts_with("GET /proxies HTTP/") { "{\"proxies\":{}}" }
                        else {
                            assert!(request.contains("timeout=10000"));
                            if request.contains("/slow/") { std::thread::sleep(Duration::from_millis(1700)); }
                            "{\"delay\":210}"
                        };
                        write!(stream,"HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
                    });
                }
            });
        });
        let completed = std::sync::Mutex::new(Vec::new());
        let results = batch_stream(client, &["slow".into(),"fast".into()], &|name,value| {
            if value["status"] == "ok" { completed.lock().unwrap().push(name.to_owned()); }
        }).unwrap();
        server.join().unwrap();
        assert_eq!(completed.into_inner().unwrap(), ["fast", "slow"]);
        assert_eq!(results["slow"]["delay"], 210);
        assert_eq!(results["slow"]["attempts"], 1);
        assert_eq!(results["fast"]["delay"], 210);
    }
    #[test]
    fn reachable_subscription_tail_is_published_before_blocked_prefix_finishes() {
        use std::{sync::{Arc,mpsc,atomic::{AtomicBool,Ordering}},time::Duration};
        let released=Arc::new(AtomicBool::new(false));
        let worker_release=released.clone();
        let (tx,rx)=mpsc::channel();
        let worker=std::thread::spawn(move || {
            let names:Vec<_>=(0..174).map(|i|i.to_string()).collect();
            let names=discovery_order(&names,&Value::Null,DISPLAY_URL);
            discover_batch(&names,DISPLAY_TIMEOUT_MS,&|name,_| {
                if name!="60" {
                    let deadline=std::time::Instant::now()+Duration::from_secs(5);
                    while !worker_release.load(Ordering::SeqCst) && std::time::Instant::now()<deadline {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
                TestResult {status:"ok".into(),delay:Some(56),attempts:1,error:None}
            }, &|name,_| {let _=tx.send(name.to_owned());})
        });
        let first=rx.recv_timeout(Duration::from_secs(2));
        let still_waiting=!worker.is_finished();
        released.store(true,Ordering::SeqCst);
        let results=worker.join().unwrap();
        assert_eq!(first.unwrap(),"60");
        assert!(still_waiting);
        assert_eq!(results.as_object().unwrap().len(),174);
    }
    #[test]
    fn failed_node_recovers_on_next_probe_without_subscription_refresh() {
        use std::{io::{Read,Write}, net::TcpListener, thread, time::Duration};
        let origin = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://127.0.0.1:{}/test",origin.local_addr().unwrap().port());
        origin.set_nonblocking(true).unwrap();
        let worker = thread::spawn(move || {
            for responds in [false, true] {
                let deadline = std::time::Instant::now() + Duration::from_secs(15);
                let mut connection = loop {
                    if let Ok((connection,_)) = origin.accept() { break connection; }
                    assert!(std::time::Instant::now() < deadline);
                    thread::sleep(Duration::from_millis(10));
                };
                connection.set_nonblocking(false).unwrap();
                connection.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                let _ = connection.read(&mut [0;2048]);
                if !responds { continue; }
                thread::sleep(Duration::from_millis(20));
                connection.write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            }
        });
        let mut settings = Settings::default();
        settings.subscriptions.push(Subscription { source: Default::default(), options: Default::default(),id:"fixture".into(),name:"fixture".into(),masked_url:String::new(),updated_at:0,error:None,
            servers:vec![json!({"name":"fixture","type":"direct"})]});
        let binary = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/Atlas.Core.exe");
        with_offline_core(settings,binary,std::env::temp_dir(),|client| {
            assert_eq!(display_probe(&client,"fixture",&endpoint,3000).status,"unreachable");
            client.api("PUT","/proxies/ATLAS",Some(json!({"name":"fixture"})))?;
            assert_eq!(display_probe(&client,"fixture",&endpoint,3000).status,"ok");
            Ok(())
        }).unwrap();
        worker.join().unwrap();
    }
    #[test]
    fn publishes_fast_result_while_another_node_is_still_blocked() {
        use std::{sync::{mpsc, Mutex}, time::Duration};
        let (progress_tx, progress_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let release_rx = Mutex::new(release_rx);
            run_display_batch(&["slow".into(), "fast".into()], &|name| {
                if name == "slow" { release_rx.lock().unwrap().recv_timeout(Duration::from_secs(5)).unwrap(); }
                json!({"status":"ok","delay":42})
            }, &|name, _| { progress_tx.send(name.to_owned()).unwrap(); })
        });
        assert_eq!(progress_rx.recv_timeout(Duration::from_secs(2)).unwrap(), "fast");
        assert!(!worker.is_finished());
        release_tx.send(()).unwrap();
        let result = worker.join().unwrap();
        assert_eq!(result["slow"]["delay"], 42);
        assert_eq!(result["fast"]["delay"], 42);
    }
    #[test]
    fn display_uses_one_cloudflare_request_and_keeps_local_errors_distinct() {
        let r=display_attempt(DISPLAY_URL, |_| Err("Mihomo API: HTTP 504".into()));
        assert_eq!(r.status,"timeout"); assert_eq!(r.attempts,1);
        let r=display_attempt(DISPLAY_URL, |_| Err("Очередь проверок серверов заполнена".into()));
        assert_eq!(r.status,"error"); assert_eq!(r.attempts,1);
    }
    #[test]
    fn server_name_is_a_path_segment_not_form_data() {
        assert_eq!(
            encode_name("DE Frankfurt + A/B"),
            "DE%20Frankfurt%20%2B%20A%2FB"
        );
        assert!(!encode_name("🇩🇪 Германия · 1").contains('+'));
    }
    #[test]
    fn zero_is_valid() {
        assert_eq!(display_attempt(DISPLAY_URL, |_| Ok(0)).delay, Some(0));
    }
    #[test]
    fn control_failure_does_not_mark_working_server_unreachable() {
        for error in [
            "Сетевая служба занята",
            "Канал сетевой службы закрыт",
            "Mihomo API недоступен",
            "Mihomo API: HTTP 401",
        ] {
            let r = display_attempt(DISPLAY_URL, |_| Err(error.into()));
            assert_eq!(r.status, "error");
            assert_eq!(r.attempts, 1);
            assert_eq!(r.error.as_deref(), Some(error));
        }
    }
    #[test]
    fn disconnected_probe_uses_an_isolated_core_without_tun() {
        use std::{io::{Read,Write}, net::TcpListener, thread, time::Duration};
        let origin=TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint=format!("http://127.0.0.1:{}/test",origin.local_addr().unwrap().port());
        let worker=thread::spawn(move || {
            let (mut connection,_)=origin.accept().unwrap();
            connection.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut bytes=[0;2048];let _=connection.read(&mut bytes);
            thread::sleep(Duration::from_millis(20));
            connection.write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        });
        let mut settings=Settings::default();
        settings.mode="tun".into();
        settings.selected="fixture".into();
        settings.subscriptions.push(Subscription { source: Default::default(), options: Default::default(),id:"fixture".into(),name:"fixture".into(),
            masked_url:String::new(),updated_at:0,error:None,
            servers:vec![json!({"name":"fixture","type":"direct"})]});
        let binary=PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/Atlas.Core.exe");
        let parent=std::env::temp_dir().join(format!("atlas-probe-recovery-{}",uuid::Uuid::new_v4()));
        std::fs::create_dir(&parent).unwrap();
        let marker=parent.join("tun-guard.active");
        std::fs::write(&marker,b"existing protected session").unwrap();
        let result=with_offline_core(settings,binary,parent.clone(),|client| {
            assert_eq!(client.api("GET","/configs",None)?["tun"]["enable"],false);
            Ok(display_probe(&client,"fixture",&endpoint,3000))
        }).unwrap();
        worker.join().unwrap();
        assert_eq!(result.status,"ok","{result:?}");
        assert_eq!(std::fs::read(&marker).unwrap(),b"existing protected session");
        assert_eq!(std::fs::read_dir(&parent).unwrap().count(),1);
        std::fs::remove_dir_all(parent).unwrap();
    }
}

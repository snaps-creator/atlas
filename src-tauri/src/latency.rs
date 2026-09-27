use crate::{core::{ApiClient, Core}, model::Settings};
use serde::Serialize;
use serde_json::{json, Value};
use std::path::PathBuf;

pub const DISPLAY_URL: &str = "http://cp.cloudflare.com/generate_204";
pub const DISPLAY_TIMEOUT_MS: u64 = 10000;
// Match Clash Verge's per-node measurement without saturating the controller.
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
    Ok(run_display_batch(names, &|name| serde_json::to_value(display_probe(&client, name, endpoint, timeout)).unwrap(), progress))
}

fn run_display_batch(names: &[String], probe: &(dyn Fn(&str) -> Value + Sync),
    progress: &(dyn Fn(&str, &Value) + Sync)) -> Value {
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results = std::sync::Mutex::new(serde_json::Map::new());
    std::thread::scope(|scope| {
        // Keep spare controller capacity for active-node health and failover.
        // The UI deadline is sized for six parallel network timeouts.
        for worker in 0..names.len().min(6) {
            let (next, results) = (&next, &results);
            scope.spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(worker as u64 * 20));
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(name) = names.get(i) else { break; };
                    let value = probe(name);
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
pub const SECONDARY_URL: &str = "https://www.cloudflare.com/cdn-cgi/trace";
pub const ENDPOINTS: [&str; 2] = [DISPLAY_URL, SECONDARY_URL];
pub fn expected_status(endpoint: &str) -> u16 {
    if endpoint == SECONDARY_URL { 200 } else { 204 }
}
const CONTROL_STATUS_ERROR: &str = "Контрольный URL не подтвердил ожидаемый HTTP 204 (per-URL health)";
pub(crate) fn verified_probe(client: &ApiClient, name: &str, endpoint: &str) -> Result<Value,String> {
    let path = format!("/proxies/{}/delay?timeout={DISPLAY_TIMEOUT_MS}&expected={}&url={}",encode_name(name),expected_status(endpoint),encode_name(endpoint));
    let result = client.api("GET",&path,None)?;
    let proxies = client.api("GET","/proxies",None)?;
    // Mihomo returns a delay even when expected HTTP status did not match.
    // Its per-URL alive flag includes that status check; the delay alone does not.
    let health = &proxies["proxies"][name]["extra"][endpoint];
    if health["alive"] != true {
        return Err(CONTROL_STATUS_ERROR.into());
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
        let path = format!("/proxies/{}/delay?timeout={timeout}&url={}", encode_name(name), encode_name(url));
        client.api("GET", &path, None).and_then(|v|v["delay"].as_u64().ok_or("Ядро не вернуло задержку".into()))
    })
}
fn display_attempt(endpoint: &str, mut probe: impl FnMut(&str)->Result<u64,String>) -> TestResult {
    match probe(endpoint) {
        Ok(delay) => TestResult {status:"ok".into(),delay:Some(delay),attempts:1,error:None},
        Err(error) => TestResult {status:if matches!(error.as_str(),"Mihomo API: HTTP 503"|"Mihomo API: HTTP 504") {"unreachable"} else {"error"}.into(),
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
        assert_eq!(r.status,"unreachable"); assert_eq!(r.attempts,1);
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
        settings.subscriptions.push(Subscription{id:"fixture".into(),name:"fixture".into(),
            masked_url:String::new(),updated_at:0,error:None,
            servers:vec![json!({"name":"fixture","type":"direct"})]});
        let binary=PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/Atlas.Core.exe");
        let result=with_offline_core(settings,binary,std::env::temp_dir(),|client| {
            assert_eq!(client.api("GET","/configs",None)?["tun"]["enable"],false);
            Ok(display_probe(&client,"fixture",&endpoint,3000))
        }).unwrap();
        worker.join().unwrap();
        assert_eq!(result.status,"ok","{result:?}");
    }
}

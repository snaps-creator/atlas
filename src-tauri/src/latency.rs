use crate::core::ApiClient;
use serde::Serialize;
use serde_json::{json, Value};

pub const DISPLAY_URL: &str = "http://cp.cloudflare.com/generate_204";
pub const DISPLAY_TIMEOUT_MS: u64 = 10000;
// Match Clash Verge's per-node measurement, with ten bounded workers.
pub fn batch(client: ApiClient, names: &[String]) -> Result<Value, String> {
    display_batch_at(client, names, DISPLAY_URL, DISPLAY_TIMEOUT_MS)
}
pub(crate) fn display_batch_at(client: ApiClient, names: &[String], endpoint: &str, timeout: u64) -> Result<Value, String> {
    client.api("GET", "/version", None)?;
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results = std::sync::Mutex::new(serde_json::Map::new());
    std::thread::scope(|scope| {
        for worker in 0..names.len().min(10) {
            let (client, next, results) = (&client, &next, &results);
            scope.spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(worker as u64 * 20));
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(name) = names.get(i) else { break; };
                    let result = display_probe(client, name, endpoint, timeout);
                    results.lock().unwrap().insert(name.clone(), serde_json::to_value(result).unwrap());
                }
            });
        }
    });
    Ok(Value::Object(results.into_inner().unwrap()))
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
pub const ENDPOINTS: [&str; 2] = [
    "https://www.gstatic.com/generate_204",
    "https://cp.cloudflare.com/generate_204",
];
const CONTROL_STATUS_ERROR: &str = "Контрольный URL не подтвердил ожидаемый HTTP 204 (per-URL health)";
pub(crate) fn verified_probe(client: &ApiClient, name: &str, endpoint: &str) -> Result<Value,String> {
    let path = format!("/proxies/{}/delay?timeout=5000&expected=204&url={}",encode_name(name),encode_name(endpoint));
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
#[derive(Serialize, Debug)]
pub struct TestResult {
    pub status: String,
    pub delay: Option<u64>,
    pub attempts: u8,
    pub error: Option<String>,
}
#[cfg(test)]
fn attempts(mut test: impl FnMut(&str) -> Result<u64, String>) -> TestResult {
    for attempt in 0..4 {
        match test(ENDPOINTS[attempt % 2]) {
            Ok(delay) => {
                return TestResult {
                    status: "ok".into(),
                    delay: Some(delay),
                    attempts: attempt as u8 + 1,
                    error: None,
                }
            }
            // Only the delay endpoint's explicit failed probe is evidence about
            // the remote path. IPC, authentication and malformed replies are local errors.
            Err(error)
                if !matches!(
                    error.as_str(),
                    "Mihomo API: HTTP 503" | "Mihomo API: HTTP 504" | CONTROL_STATUS_ERROR
                ) =>
            {
                return TestResult {
                    status: "error".into(),
                    delay: None,
                    attempts: attempt as u8 + 1,
                    error: Some(error),
                }
            }
            Err(_) => {}
        }
    }
    TestResult {
        status: "unreachable".into(),
        delay: None,
        attempts: 4,
        error: Some("Два проверочных адреса не ответили через этот сервер. Это не доказывает недоступность всех сайтов через него.".into()),
    }
}
pub fn test(client: ApiClient, name: &str) -> TestResult {
    if let Err(error) = client.api("GET", "/version", None) {
        return TestResult {
            status: "error".into(),
            delay: None,
            attempts: 0,
            error: Some(format!("Не удалось проверить ядро: {error}")),
        };
    }
    let mut result = display_probe(&client, name, DISPLAY_URL, DISPLAY_TIMEOUT_MS);
    if result.status == "unreachable" && client.api("GET", "/version", None).is_err() {
        result.status = "error".into();
        result.error = Some("Связь с ядром прервалась во время проверки".into());
    }
    result
}
pub(crate) fn display_probe(client: &ApiClient, name: &str, endpoint: &str, timeout: u64) -> TestResult {
    // Like Clash: one URL test, no expected-status filter or alternate URL.
    // This measures latency, not application availability or download throughput.
    let path = format!("/proxies/{}/delay?timeout={timeout}&url={}", encode_name(name), encode_name(endpoint));
    match client.api("GET", &path, None).and_then(|v|v["delay"].as_u64().ok_or("Ядро не вернуло задержку".into())) {
        Ok(delay) => TestResult { status:"ok".into(), delay:Some(delay), attempts:1, error:None },
        Err(error) => TestResult { status:if matches!(error.as_str(), "Mihomo API: HTTP 503"|"Mihomo API: HTTP 504") { "unreachable" } else { "error" }.into(), delay:None, attempts:1, error:Some(error) },
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
    #[test]
    fn server_name_is_a_path_segment_not_form_data() {
        assert_eq!(
            encode_name("DE Frankfurt + A/B"),
            "DE%20Frankfurt%20%2B%20A%2FB"
        );
        assert!(!encode_name("🇩🇪 Германия · 1").contains('+'));
    }
    #[test]
    fn alternate_endpoint_prevents_false_unavailable() {
        let mut seen = vec![];
        let r = attempts(|url| {
            seen.push(url.to_owned());
            if url == ENDPOINTS[0] {
                Err("Mihomo API: HTTP 504".into())
            } else {
                Ok(31)
            }
        });
        assert_eq!(r.delay, Some(31));
        assert_eq!(r.attempts, 2);
        assert_ne!(seen[0], seen[1]);
    }
    #[test]
    fn requires_four_failures() {
        let mut count = 0;
        let r = attempts(|_| {
            count += 1;
            Err("Mihomo API: HTTP 504".into())
        });
        assert_eq!(count, 4);
        assert_eq!(r.status, "unreachable");
    }
    #[test]
    fn zero_is_valid() {
        assert_eq!(attempts(|_| Ok(0)).delay, Some(0));
    }
    #[test]
    fn control_failure_does_not_mark_working_server_unreachable() {
        for error in [
            "Сетевая служба занята",
            "Канал сетевой службы закрыт",
            "Mihomo API недоступен",
            "Mihomo API: HTTP 401",
        ] {
            let r = attempts(|_| Err(error.into()));
            assert_eq!(r.status, "error");
            assert_eq!(r.attempts, 1);
            assert_eq!(r.error.as_deref(), Some(error));
        }
    }
}

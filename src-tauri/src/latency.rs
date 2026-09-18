use crate::core::ApiClient;
use serde::Serialize;
pub const ENDPOINTS: [&str; 2] = [
    "https://www.gstatic.com/generate_204",
    "https://cp.cloudflare.com/generate_204",
];
#[derive(Serialize, Debug)]
pub struct TestResult {
    pub status: String,
    pub delay: Option<u64>,
    pub attempts: u8,
    pub error: Option<String>,
}
fn attempts(mut test: impl FnMut(&str) -> Result<u64, String>) -> TestResult {
    for attempt in 0..4 {
        if let Ok(delay) = test(ENDPOINTS[attempt % 2]) {
            return TestResult {
                status: "ok".into(),
                delay: Some(delay),
                attempts: attempt as u8 + 1,
                error: None,
            };
        }
    }
    TestResult {
        status: "unreachable".into(),
        delay: None,
        attempts: 4,
        error: Some("Не ответили два проверочных адреса после четырёх попыток".into()),
    }
}
pub fn test(client: ApiClient, name: &str) -> TestResult {
    if client.api("GET", "/version", None).is_err() {
        return TestResult {
            status: "not_connected".into(),
            delay: None,
            attempts: 0,
            error: Some("Ядро не запущено".into()),
        };
    }
    let encoded = encode_name(name);
    let mut api_error = None;
    let mut result = attempts(|endpoint| {
        let url: String = url::form_urlencoded::byte_serialize(endpoint.as_bytes()).collect();
        let response = client.api(
            "GET",
            &format!("/proxies/{encoded}/delay?timeout=8000&url={url}"),
            None,
        );
        if let Err(error) = &response {
            if ["HTTP 400", "HTTP 401", "HTTP 403", "HTTP 404"]
                .iter()
                .any(|code| error.contains(code))
            {
                api_error = Some(error.clone());
            }
        }
        response?["delay"]
            .as_u64()
            .ok_or("Ядро не вернуло задержку".into())
    });
    if result.status == "unreachable" && api_error.is_some() {
        result.status = "error".into();
        result.error = api_error;
    }
    if result.status == "unreachable" && client.api("GET", "/version", None).is_err() {
        result.status = "error".into();
        result.error = Some("Связь с ядром прервалась во время проверки".into());
    }
    result
}
fn encode_name(name: &str) -> String {
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
                Err("timeout".into())
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
            Err("failed".into())
        });
        assert_eq!(count, 4);
        assert_eq!(r.status, "unreachable");
    }
    #[test]
    fn zero_is_valid() {
        assert_eq!(attempts(|_| Ok(0)).delay, Some(0));
    }
}

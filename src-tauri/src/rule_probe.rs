use crate::{core::ApiClient, model::*, rules};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    io::Read,
    time::{Duration, Instant},
};
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Probe {
    pub status: String,
    pub delay: Option<u64>,
    pub detail: String,
    pub log_errors: usize,
    pub suggestions: Vec<Suggestion>,
}
#[derive(Serialize)]
pub struct Suggestion {
    pub domain: String,
    pub reason: String,
}
fn host(raw: &str) -> Option<String> {
    let value = if raw.contains("://") {
        raw.to_owned()
    } else {
        format!("https://{raw}")
    };
    let u = url::Url::parse(&value).ok()?;
    if !["http", "https"].contains(&u.scheme()) {
        return None;
    }
    let h = u.host_str()?.trim_end_matches('.').to_lowercase();
    if h.parse::<std::net::IpAddr>().is_ok() || !h.contains('.') || h.ends_with(".exe") {
        return None;
    }
    Some(h)
}
fn resources(html: &str, base: &url::Url) -> BTreeMap<String, String> {
    let tags =
        regex::Regex::new(r"(?is)<(?:script|img|iframe|source|link)\b[^>]{0,8192}>").unwrap();
    let attrs = regex::Regex::new(r#"(?i)\b(src|href)\s*=\s*["']([^"']+)["']"#).unwrap();
    let mut result = BTreeMap::new();
    for tag in tags.find_iter(html) {
        let tag = tag.as_str();
        if tag.to_lowercase().starts_with("<link")
            && !tag.to_lowercase().contains("stylesheet")
            && !tag.to_lowercase().contains("preload")
        {
            continue;
        }
        for attr in attrs.captures_iter(tag) {
            if let Ok(u) = base.join(&attr[2].replace("&amp;", "&")) {
                if let Some(h) = host(u.as_str()) {
                    result.insert(
                        h,
                        "Ресурс страницы: скрипт, стиль или встроенное содержимое".into(),
                    );
                }
            }
        }
    }
    result
}
fn covered(s: &Settings, domain: &str, route: &Route) -> bool {
    s.groups
        .iter()
        .filter(|g| g.enabled && g.route == *route)
        .any(|g| {
            g.rules.iter().any(|r| {
                rules::normalize(r).is_ok_and(|r| match r.kind.as_str() {
                    "DOMAIN" => r.value == domain,
                    "DOMAIN-SUFFIX" => {
                        r.value == domain || domain.ends_with(&format!(".{}", r.value))
                    }
                    _ => false,
                })
            })
        })
}
pub fn run(s: &Settings, client: ApiClient, rule: &Rule, route: &Route) -> Probe {
    let mut out = Probe {
        status: "waiting".into(),
        delay: None,
        detail: "Проверка после подключения VPN".into(),
        log_errors: 0,
        suggestions: vec![],
    };
    if client.api("GET", "/version", None).is_err() {
        return out;
    }
    let r = match rules::normalize(rule) {
        Ok(r) => r,
        Err(e) => {
            out.status = "error".into();
            out.detail = e;
            return out;
        }
    };
    let before = client.logs().unwrap_or_default();
    let mut candidates = BTreeMap::new();
    if ["DOMAIN", "DOMAIN-SUFFIX"].contains(&r.kind.as_str()) {
        let target = format!("https://{}/", r.value);
        let started = Instant::now();
        let response = reqwest::blocking::Client::builder()
            .no_proxy()
            .proxy(reqwest::Proxy::all("http://127.0.0.1:17890").unwrap())
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .and_then(|c| {
                c.get(&target)
                    .header("User-Agent", "Atlas-Rule-Check/1.0")
                    .send()
            });
        out.delay = Some(started.elapsed().as_millis() as u64);
        match response {
            Ok(response) => {
                let status = response.status().as_u16();
                let base = response.url().clone();
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| base.join(v).ok())
                    .and_then(|u| host(u.as_str()));
                out.status = if status < 400 { "ok" } else { "warning" }.into();
                out.detail = if status == 403 || status == 429 {
                    format!("HTTP {status}: сайт ограничил запрос. Причиной может быть IP сервера или защита от автоматических запросов.")
                } else {
                    format!("HTTP {status}")
                };
                if *route == Route::Block {
                    out.status = "warning".into();
                    out.detail=format!("Ожидалась блокировка, но получен HTTP {status}; проверьте приоритет правил");
                }
                if let Some(h) = location {
                    candidates.insert(h, "Перенаправление сайта".into());
                }
                let html = response
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|s| s.contains("text/html"));
                if html {
                    let mut body = String::new();
                    if response.take(512 * 1024).read_to_string(&mut body).is_ok() {
                        candidates.extend(resources(&body, &base));
                    }
                }
            }
            Err(e) => {
                out.status = if *route == Route::Block {
                    "blocked"
                } else {
                    "error"
                }
                .into();
                out.detail = if *route == Route::Block {
                    "Доступ не получен при правиле блокировки"
                } else if e.is_timeout() {
                    "Время ожидания сайта истекло"
                } else {
                    "Не удалось установить HTTPS-соединение"
                }
                .into();
            }
        }
    } else {
        let v = match client.api("GET", "/connections", None) {
            Ok(v) => v,
            Err(_) => {
                out.status = "error".into();
                out.detail = "Не удалось получить список соединений ядра".into();
                return out;
            }
        };
        let entries = v["connections"].as_array().cloned().unwrap_or_default();
        let matches: Vec<_> = entries
            .iter()
            .filter(|c| {
                let m = &c["metadata"];
                match r.kind.as_str() {
                    "PROCESS-NAME" => m["process"]
                        .as_str()
                        .is_some_and(|p| p.eq_ignore_ascii_case(&r.value)),
                    "PROCESS-DOMAIN" => {
                        let (p, d) = r.value.split_once('|').unwrap();
                        m["process"]
                            .as_str()
                            .is_some_and(|v| v.eq_ignore_ascii_case(p))
                            && m["host"].as_str() == Some(d)
                    }
                    "DOMAIN-KEYWORD" => m["host"].as_str().is_some_and(|h| h.contains(&r.value)),
                    "IP-CIDR" | "IP-CIDR6" => {
                        r.value.parse::<ipnet::IpNet>().ok().is_some_and(|net| {
                            m["destinationIP"]
                                .as_str()
                                .and_then(|ip| ip.parse::<std::net::IpAddr>().ok())
                                .is_some_and(|ip| net.contains(&ip))
                        })
                    }
                    _ => false,
                }
            })
            .collect();
        out.detail = if matches.is_empty() {
            "Ожидание трафика: сейчас нет активных соединений. Это не означает недоступность приложения.".into()
        } else {
            format!("Активных соединений: {} · проверка маршрута", matches.len())
        };
        out.status = if matches.is_empty() {
            "observing"
        } else {
            "ok"
        }
        .into();
        for c in matches {
            let direct = c["chains"]
                .as_array()
                .is_some_and(|a| a.iter().any(|v| v == "DIRECT"));
            if *route == Route::Proxy && direct {
                out.status = "warning".into();
                out.detail = "Есть соединения напрямую; проверьте правило приложения".into();
                if let Some(h) = c["metadata"]["host"].as_str().and_then(host) {
                    candidates.insert(h, "Соединение приложения прошло напрямую".into());
                }
            }
        }
    }
    let logs = client.logs().unwrap_or_default();
    let domains =
        regex::Regex::new(r"(?i)\b([a-z0-9](?:[a-z0-9.-]*[a-z0-9])?\.[a-z]{2,63}):[0-9]{1,5}\b")
            .unwrap();
    for line in logs
        .iter()
        .filter(|line| !before.contains(line) || line.to_lowercase().contains(&r.value))
    {
        let lower = line.to_lowercase();
        if !lower.contains("level=error") && !lower.contains("level=warning") {
            continue;
        }
        let related = lower.contains(&r.value) || candidates.keys().any(|d| lower.contains(d));
        if !related {
            continue;
        }
        out.log_errors += 1;
        for capture in domains.captures_iter(line) {
            if let Some(h) = host(&capture[1]) {
                candidates.insert(h, "Ошибка соединения в связанном журнале ядра".into());
            }
        }
    }
    candidates.remove(&r.value);
    out.suggestions = candidates
        .into_iter()
        .filter(|(d, _)| !covered(s, d, route))
        .take(20)
        .map(|(domain, reason)| Suggestion { domain, reason })
        .collect();
    if out.log_errors > 0 && out.status == "ok" {
        out.status = "warning".into();
        out.detail.push_str(" · есть связанные ошибки ядра");
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_resource_domains_not_external_links() {
        let base = url::Url::parse("https://site.test/").unwrap();
        let found = resources(
            r#"<script src="https://cdn.test/app.js"></script><a href="https://unrelated.test">link</a><link rel="stylesheet" href="//style.test/a.css"><img src="/image.png">"#,
            &base,
        );
        assert!(found.contains_key("cdn.test"));
        assert!(found.contains_key("style.test"));
        assert!(!found.contains_key("unrelated.test"));
    }
    #[test]
    fn suffix_coverage_uses_domain_boundary() {
        let mut s = Settings::default();
        s.groups = crate::portable::parse(
            "version: 1\ndefault-route: direct\nrules: [{domain-suffix: site.test, route: proxy}]",
        )
        .unwrap()
        .groups;
        assert!(covered(&s, "cdn.site.test", &Route::Proxy));
        assert!(!covered(&s, "othersite.test", &Route::Proxy));
    }
}

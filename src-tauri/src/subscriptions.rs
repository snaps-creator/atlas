use base64::{
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE_NO_PAD},
    Engine,
};
use serde_json::{json, Value};
pub fn decode(s: &str) -> Result<String, String> {
    for engine in [&STANDARD, &STANDARD_NO_PAD, &URL_SAFE_NO_PAD] {
        if let Ok(bytes) = engine.decode(s.trim()) {
            return String::from_utf8(bytes)
                .map_err(|_| "Подписка содержит некорректный UTF-8".into());
        }
    }
    Err("Некорректная Base64-подписка".into())
}
pub fn mask(s: &str) -> String {
    url::Url::parse(s)
        .ok()
        .and_then(|u| u.host_str().map(|h| format!("https://{h}/********")))
        .unwrap_or_else(|| "********".into())
}
pub fn parse(text: &str) -> Result<Vec<Value>, String> {
    if text.len() > 8 * 1024 * 1024 {
        return Err("Подписка превышает 8 МБ".into());
    }
    if let Ok(doc) = serde_yaml::from_str::<Value>(text) {
        if let Some(proxies) = doc.get("proxies").and_then(Value::as_array) {
            return validate(proxies.clone());
        }
    }
    let decoded;
    if !text.contains("://") {
        decoded = decode(&text.split_whitespace().collect::<String>())?;
    } else {
        decoded = text.to_owned()
    }
    let mut nodes = vec![];
    for line in decoded.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if let Some(raw) = line.strip_prefix("vmess://") {
            let v: Value =
                serde_json::from_str(&decode(raw)?).map_err(|_| "Некорректный VMess URI")?;
            let port = v["port"]
                .as_u64()
                .or_else(|| v["port"].as_str().and_then(|s| s.parse().ok()))
                .ok_or("VMess: отсутствует порт")?;
            let net = v["net"].as_str().unwrap_or("tcp");
            if !["tcp", "ws"].contains(&net) {
                return Err(
                    "VMess URI: поддерживаются transport tcp и ws; используйте YAML для остальных"
                        .into(),
                );
            }
            let mut p = json!({"name":v["ps"].as_str().unwrap_or("VMess"),"type":"vmess","server":v["add"],"port":port,"uuid":v["id"],"alterId":v["aid"].as_u64().unwrap_or(0),"cipher":"auto","tls":v["tls"]=="tls","servername":v["sni"].as_str().unwrap_or(""),"network":net});
            if net == "ws" {
                p["ws-opts"] = json!({"path":v["path"].as_str().unwrap_or("/"),"headers":{"Host":v["host"].as_str().unwrap_or("")}})
            }
            nodes.push(p);
            continue;
        }
        let u = url::Url::parse(line)
            .map_err(|_| "Неизвестный формат подписки; нужен Clash/Mihomo YAML или proxy URI")?;
        let host = u.host_str().ok_or("В URI отсутствует сервер")?;
        let port = u.port().ok_or("В URI отсутствует порт")?;
        let name = u
            .fragment()
            .map(percent_decode)
            .unwrap_or_else(|| format!("{}:{port}", host));
        let q: std::collections::HashMap<String, String> = u.query_pairs().into_owned().collect();
        let mut p = json!({"name":name,"server":host,"port":port,"type":u.scheme()});
        match u.scheme() {
            "ss" => {
                if q.contains_key("plugin") {
                    return Err("Shadowsocks plugin URI: импортируйте YAML с plugin-opts".into());
                }
                let user = percent_decode(u.username());
                let auth = if let Some(password) = u.password() {
                    format!("{}:{}", user, percent_decode(password))
                } else {
                    decode(&user)?
                };
                let (cipher, password) = auth
                    .split_once(':')
                    .ok_or("Shadowsocks: некорректные учётные данные")?;
                p["cipher"] = json!(cipher);
                p["password"] = json!(password);
            }
            "trojan" | "vless" | "hysteria2" | "hy2" => {
                if u.scheme() == "vless" {
                    p["uuid"] = json!(percent_decode(u.username()));
                    p["flow"] = json!(q.get("flow").cloned().unwrap_or_default());
                } else {
                    p["password"] = json!(percent_decode(u.username()));
                }
                if u.scheme() == "hy2" {
                    p["type"] = json!("hysteria2")
                }
                p["tls"] =
                    json!(u.scheme() != "vless" || q.get("security").is_some_and(|s| s != "none"));
                if let Some(sni) = q.get("sni") {
                    p[if u.scheme() == "vless" {
                        "servername"
                    } else {
                        "sni"
                    }] = json!(sni)
                }
                if q.get("security").is_some_and(|s| s == "reality") {
                    p["reality-opts"] = json!({"public-key":q.get("pbk").ok_or("Reality: public key отсутствует")?,"short-id":q.get("sid").cloned().unwrap_or_default()});
                    p["client-fingerprint"] =
                        json!(q.get("fp").cloned().unwrap_or("chrome".into()));
                }
                if let Some(net) = q.get("type") {
                    match net.as_str() {
                        "tcp" => {}
                        "ws" => {
                            p["network"] = json!("ws");
                            p["ws-opts"] = json!({"path":q.get("path").cloned().unwrap_or("/".into()),"headers":{"Host":q.get("host").cloned().unwrap_or_default()}})
                        }
                        "grpc" => {
                            p["network"] = json!("grpc");
                            p["grpc-opts"] = json!({"grpc-service-name":q.get("serviceName").cloned().unwrap_or_default()})
                        }
                        _ => return Err("Неизвестный transport в URI".into()),
                    }
                }
            }
            _ => {
                return Err(format!(
                    "Протокол {} не поддерживается в URI; используйте Mihomo YAML",
                    u.scheme()
                ))
            }
        }
        nodes.push(p);
    }
    validate(nodes)
}
fn percent_decode(s: &str) -> String {
    let encoded = format!("v={}", s.replace('+', "%2B"));
    url::form_urlencoded::parse(encoded.as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}
fn validate(nodes: Vec<Value>) -> Result<Vec<Value>, String> {
    if nodes.is_empty() {
        return Err("Подписка не содержит серверов".into());
    }
    if nodes.len() > 10000 {
        return Err("Слишком много серверов в подписке".into());
    }
    for p in &nodes {
        if p["name"].as_str().is_none()
            || p["server"].as_str().is_none()
            || p["type"].as_str().is_none()
            || p["port"].as_u64().is_none_or(|n| n == 0 || n > 65535)
        {
            return Err("Сервер должен содержать name, server, type и корректный port".into());
        }
    }
    Ok(nodes)
}
pub fn download(raw: &str, connected: bool) -> Result<Vec<Value>, String> {
    let u = url::Url::parse(raw).map_err(|_| "Некорректный URL подписки")?;
    if u.scheme() != "https"
        || u.host_str().is_none()
        || !u.username().is_empty()
        || u.password().is_some()
    {
        return Err("Подписка должна использовать HTTPS без userinfo".into());
    }
    let mut builder = reqwest::blocking::Client::builder().no_proxy();
    if connected {
        builder = builder.proxy(
            reqwest::Proxy::all("http://127.0.0.1:17890")
                .map_err(|_| "Ошибка локального прокси")?,
        );
    }
    let client = builder
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Ошибка HTTPS клиента")?;
    let mut response = client
        .get(u)
        .header("User-Agent", "Atlas/1.0-beta.1 mihomo")
        .send()
        .map_err(|_| {
            "Не удалось загрузить подписку: проверьте сеть и TLS. Предыдущая версия сохранена."
        })?;
    if !response.status().is_success() {
        return Err(format!(
            "Сервер подписки вернул HTTP {}. Предыдущая версия сохранена.",
            response.status().as_u16()
        ));
    }
    use std::io::Read;
    let mut body = String::new();
    (&mut response)
        .take(8 * 1024 * 1024 + 1)
        .read_to_string(&mut body)
        .map_err(|_| "Не удалось прочитать подписку")?;
    parse(&body)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn url_secret_never_displayed() {
        assert_eq!(
            mask("https://u:p@example.com/path/secret?token=secret"),
            "https://example.com/********"
        )
    }
    #[test]
    fn yaml() {
        assert_eq!(parse("proxies:\n - {name: A, type: ss, server: localhost, port: 443, cipher: aes-128-gcm, password: x}").unwrap().len(),1)
    }
    #[test]
    fn base64_uri() {
        let x = STANDARD.encode("trojan://password@example.com:443#Test");
        assert_eq!(parse(&x).unwrap()[0]["password"], "password")
    }
    #[test]
    fn invalid() {
        assert!(parse("invalid").is_err())
    }
}

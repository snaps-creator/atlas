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
    parse_options(text,&Default::default())
}
fn parse_options(text: &str, options: &crate::subscription_options::Options) -> Result<Vec<Value>,String> {
    if text.len() > 8 * 1024 * 1024 {
        return Err("Подписка превышает 8 МБ".into());
    }
    if let Some(nodes)=crate::xray_config::import_json(text)? {return validate(nodes);}
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
    if let Some(nodes)=crate::xray_config::import_json(&decoded)? {return validate(nodes);}
    let mut nodes = vec![];
    for line in decoded.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if line.starts_with("vless://") || line.starts_with("trojan://") {
            let line=crate::xray_config::with_provider_fragment(line,options)?;
            if let Some(mut node)=crate::xray_config::uri(&line)? {
                crate::xray_config::apply_provider_noises(&mut node,options)?;
                nodes.push(node);continue;
            }
        }
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
                // URI transport parameters apply to ordinary TLS too, not just
                // Reality. Dropping them changes the ClientHello and ALPN offer.
                if p["tls"] == true {
                    if let Some(fp) = q.get("fp").filter(|v| !v.is_empty()) {
                        p["client-fingerprint"] = json!(fp);
                    }
                    if let Some(alpn) = q.get("alpn") {
                        let protocols: Vec<&str> = alpn.split(',').map(str::trim).filter(|v| !v.is_empty()).collect();
                        if !protocols.is_empty() { p["alpn"] = json!(protocols); }
                    }
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
pub struct Downloaded {
    pub nodes: Vec<Value>,
    pub options: crate::subscription_options::Options,
}
pub fn download(raw: &str, connected: bool, options: &crate::subscription_options::Options) -> Result<Downloaded, String> {
    download_with(raw, options, |url| download_one(url, connected, options))
}
fn download_with(raw: &str, options: &crate::subscription_options::Options,
    mut fetch: impl FnMut(&str) -> Result<Downloaded, String>) -> Result<Downloaded, String> {
    let primary=options.effective_url.as_deref().unwrap_or(raw);
    match fetch(primary) {
        Ok(result)=>Ok(result),
        Err(primary_error)=> {
            if let Some(fallback)=options.fallback_url.as_deref().filter(|u|*u!=primary) {
                fetch(fallback).map_err(|fallback_error|
                    format!("Основной адрес: {primary_error}; резервный адрес: {fallback_error}"))
            } else { Err(primary_error) }
        }
    }
}
fn download_one(raw: &str, connected: bool, options: &crate::subscription_options::Options) -> Result<Downloaded, String> {
    crate::subscription_options::validate_agent(options.effective_user_agent())?;
    let u = crate::subscription_options::https_url(raw)?;
    let mut builder = reqwest::blocking::Client::builder().no_proxy();
    if connected {
        builder = builder.proxy(
            reqwest::Proxy::all("http://127.0.0.1:17890")
                .map_err(|_| "Ошибка локального прокси")?,
        );
    }
    let client = builder
        .timeout(std::time::Duration::from_secs(if options.fallback_url.is_some() { 9 } else { 30 }))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Ошибка HTTPS клиента")?;
    let mut response = client
        .get(u)
        .header("User-Agent", options.effective_user_agent())
        .send()
        .map_err(|e| {
            let mut chain=e.to_string();
            let mut source=std::error::Error::source(&e);
            while let Some(error)=source {chain.push_str(&format!(" -> {error}"));source=error.source();}
            format!("Загрузка подписки: timeout={}, connect={}, {}; предыдущая версия сохранена",
                e.is_timeout(),e.is_connect(),crate::support_report::redact(&chain,&[raw.to_owned()]))
        })?;
    if !response.status().is_success() {
        return Err(format!(
            "Сервер подписки вернул HTTP {}. Предыдущая версия сохранена.",
            response.status().as_u16()
        ));
    }
    use std::io::Read;
    let headers=response.headers().iter().filter_map(|(k,v)|v.to_str().ok().map(|v|(k.as_str().to_owned(),v.to_owned()))).collect();
    let mut body = String::new();
    (&mut response)
        .take(8 * 1024 * 1024 + 1)
        .read_to_string(&mut body)
        .map_err(|_| "Не удалось прочитать подписку")?;
    if body.len()>8*1024*1024 { return Err("Подписка превышает 8 МБ".into()); }
    parse_response(&body, &headers, options, raw)
}
fn parse_response(body: &str, headers: &std::collections::BTreeMap<String,String>,
    options: &crate::subscription_options::Options, source: &str) -> Result<Downloaded,String> {
    // Remove outer directives before decoding: a fallback URL in a comment
    // must not cause a Base64 payload to be treated as plaintext proxy URIs.
    let (body,options)=crate::subscription_options::extract(body,headers,options,source)?;
    let (body,options)=if !body.contains("://") && crate::xray_config::import_json(&body)?.is_none()
        && serde_yaml::from_str::<Value>(&body).ok().and_then(|v|v.get("proxies").cloned()).is_none() {
        let decoded=decode(&body.split_whitespace().collect::<String>())?;
        crate::subscription_options::extract(&decoded,headers,&options,source)?
    } else { (body,options) };
    Ok(Downloaded {nodes:parse_options(&body,&options)?,options})
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn response_accepts_metadata_outside_and_inside_base64() {
        let uri="trojan://test@example.com:443#Test";
        let headers=std::collections::BTreeMap::from([("change-user-agent".into(),"Header/1".into())]);
        for body in [
            format!("#providerid: vendor\n#fallback-url: https://backup.example/key\n{}",STANDARD.encode(uri)),
            STANDARD.encode(format!("#providerid: vendor\n#fallback-url: https://backup.example/key\n#change-user-agent: Body/1\n{uri}")),
        ] {
            let result=parse_response(&body,&headers,&Default::default(),"https://a.example/key").unwrap();
            assert_eq!(result.nodes.len(),1);
            assert_eq!(result.options.fallback_url.as_deref(),Some("https://backup.example/key"));
            assert_eq!(result.options.effective_user_agent(),"Header/1");
        }
    }
    #[test]
    fn migrated_primary_uses_fallback_only_after_failure() {
        let options = crate::subscription_options::Options {
            effective_url: Some("https://new.example/key".into()),
            fallback_url: Some("https://backup.example/key".into()),
            ..Default::default()
        };
        let mut requested = Vec::new();
        let result = download_with("https://old.example/key", &options, |url| {
            requested.push(url.to_owned());
            if requested.len() == 1 { return Err("HTTP 503".into()); }
            Ok(Downloaded { nodes: vec![json!({"name":"backup"})], options: options.clone() })
        }).unwrap();
        assert_eq!(requested, ["https://new.example/key", "https://backup.example/key"]);
        assert_eq!(result.nodes[0]["name"], "backup");
        requested.clear();
        download_with("https://old.example/key", &options, |url| {
            requested.push(url.to_owned());
            Ok(Downloaded { nodes: vec![], options: options.clone() })
        }).unwrap();
        assert_eq!(requested, ["https://new.example/key"]);
    }
    #[test]
    fn fallback_does_not_loop_back_to_primary() {
        let options = crate::subscription_options::Options {
            fallback_url: Some("https://a.example/key".into()), ..Default::default()
        };
        let mut calls = 0;
        let result = download_with("https://a.example/key", &options, |_| {
            calls += 1; Err("timeout".into())
        });
        assert!(result.is_err());
        assert_eq!(calls, 1);
    }
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
    #[test]
    fn tls_uri_preserves_fingerprint_and_alpn_outside_reality() {
        let nodes = parse("vless://00000000-0000-0000-0000-000000000001@example.com:443?security=tls&type=ws&fp=firefox&alpn=h2%2Chttp%2F1.1&sni=tls.example&host=ws.example&path=%2Fsocket#TLS").unwrap();
        let stream = &nodes[0]["xray"]["outbounds"][0]["streamSettings"];
        assert_eq!(stream["tlsSettings"]["fingerprint"], "firefox");
        assert_eq!(stream["tlsSettings"]["alpn"], json!(["h2","http/1.1"]));
        assert_eq!(stream["tlsSettings"]["serverName"], "tls.example");
        assert_eq!(stream["wsSettings"]["path"], "/socket");
        assert_eq!(stream["wsSettings"]["headers"]["Host"], "ws.example");
        assert!(stream["tlsSettings"].get("allowInsecure").is_none());
        let plain = parse("vless://id@example.com:443?security=none&fp=firefox&alpn=h2#plain").unwrap();
        assert!(plain[0]["xray"]["outbounds"][0]["streamSettings"].get("tlsSettings").is_none());
    }
}

/// A delayed refresh must not overwrite a newer manual refresh or restore a deleted subscription.
pub(crate) fn refresh_is_current(before: Option<&crate::model::Subscription>, current: Option<&crate::model::Subscription>) -> bool {
    matches!((before,current),(Some(a),Some(b)) if a.id==b.id && a.updated_at==b.updated_at && a.servers==b.servers && a.options==b.options)
}
#[cfg(test)]
mod refresh_tests {
    use super::*;
    #[test]
    fn stale_or_deleted_subscription_is_never_overwritten() {
        let a=crate::model::Subscription { options: Default::default(),id:"a".into(),name:"A".into(),masked_url:String::new(),updated_at:10,error:None,servers:vec![json!({"server":"old"})]};
        assert!(refresh_is_current(Some(&a),Some(&a)));
        assert!(!refresh_is_current(Some(&a),None));
        let mut b=a.clone(); b.updated_at=11;
        assert!(!refresh_is_current(Some(&a),Some(&b)));
        b.updated_at=10; b.options.user_agent=Some("changed".into());
        assert!(!refresh_is_current(Some(&a),Some(&b)));
        b.options=a.options.clone(); b.servers=vec![json!({"server":"new"})];
        assert!(!refresh_is_current(Some(&a),Some(&b)));
    }
}

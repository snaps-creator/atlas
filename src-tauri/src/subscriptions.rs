use base64::{
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD},
    Engine,
};
use serde_json::{json, Value};
pub fn decode(s: &str) -> Result<String, String> {
    for engine in [&STANDARD, &STANDARD_NO_PAD, &URL_SAFE, &URL_SAFE_NO_PAD] {
        if let Ok(bytes) = engine.decode(s.trim()) {
            return String::from_utf8(bytes)
                .map_err(|_| "Подписка содержит некорректный UTF-8".into());
        }
    }
    Err("Некорректная Base64-подписка".into())
}
fn clean_line(line: &str) -> &str { line.trim().trim_start_matches('\u{feff}').trim() }
fn has_uri(text: &str) -> bool {
    text.lines().any(|line| ["vless://","vmess://","trojan://","ss://","hysteria2://","hy2://"]
        .iter().any(|scheme|clean_line(line).starts_with(scheme)))
}
fn structured(text: &str) -> bool {
    serde_yaml::from_str::<Value>(text).ok().is_some_and(|v|
        v.get("proxies").is_some_and(Value::is_array) || v.get("outbounds").is_some_and(Value::is_array))
}
fn subscription_body(text: &str) -> Result<String,String> {
    let text=clean_line(text);
    if text.is_empty() {return Err("Пустой ответ подписки".into());}
    if has_uri(text) || structured(text) {return Ok(text.to_owned());}
    let decoded=decode(&text.split_whitespace().collect::<String>())?;
    if !has_uri(&decoded) && !structured(&decoded) {
        return Err("Подписка не содержит поддерживаемых серверов".into());
    }
    Ok(clean_line(&decoded).to_owned())
}
pub fn mask(s: &str) -> String {
    if s.trim().starts_with("vless://") { return "VLESS · ручной импорт".into(); }
    url::Url::parse(s)
        .ok()
        .and_then(|u| u.host_str().map(|h| format!("https://{h}/********")))
        .unwrap_or_else(|| "********".into())
}
#[cfg(test)]
pub fn parse(text: &str) -> Result<Vec<Value>, String> {
    parse_report(text,&Default::default(),&mut Vec::new())
}
fn parse_report(text: &str, options: &crate::subscription_options::Options, diagnostics: &mut Vec<String>) -> Result<Vec<Value>,String> {
    if text.len() > 8 * 1024 * 1024 {
        return Err("Подписка превышает 8 МБ".into());
    }
    if let Some(nodes)=crate::xray_config::import_json(text)? {return validate(nodes);}
    if let Ok(doc) = serde_yaml::from_str::<Value>(text) {
        if let Some(proxies) = doc.get("proxies").and_then(Value::as_array) {
            return validate(proxies.clone());
        }
    }
    let decoded=subscription_body(text)?;
    if let Some(nodes)=crate::xray_config::import_json(&decoded)? {return validate(nodes);}
    if let Ok(doc)=serde_yaml::from_str::<Value>(&decoded) {
        if let Some(proxies)=doc.get("proxies").and_then(Value::as_array) {return validate(proxies.clone());}
    }
    let mut nodes = vec![];
    let mut failures = Vec::new();
    for (index,line) in decoded.lines().map(clean_line).enumerate().filter(|(_,l)| !l.is_empty() && !l.starts_with('#')) {
        let result = (|| -> Result<Value,String> {
        if line.starts_with("vless://") || line.starts_with("trojan://") {
            let line=crate::xray_config::with_provider_fragment(line,options)?;
            if let Some(mut node)=crate::xray_config::uri(&line)? {
                crate::xray_config::apply_provider_noises(&mut node,options)?;
                return Ok(node);
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
            return Ok(p);
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
        let mut p = json!({"name":name,"server":host,"port":port,"type":u.scheme(),"extraParams":q});
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
                return Err("Протокол не поддерживается в URI; используйте Mihomo YAML".into())
            }
        }
        Ok(p)
        })();
        match result.and_then(|node| validate(vec![node]).map(|mut nodes| nodes.remove(0))) {
            Ok(node) => { if !nodes.contains(&node) { nodes.push(node); } },
            Err(error) => failures.push(format!("Строка {}: {}",index+1,error)),
        }
    }
    if !failures.is_empty() {
        crate::incident_history::record("subscription_nodes_skipped",json!({"count":failures.len(),"diagnostics":failures.iter().take(100).collect::<Vec<_>>()}),&[]);
        if nodes.is_empty() { return Err(format!("Нет пригодных серверов. {}",failures.into_iter().take(5).collect::<Vec<_>>().join("; "))); }
        diagnostics.extend(failures);
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
    pub diagnostics: Vec<String>,
}
/// Keep existing names (including legacy positional names) when a provider reorders
/// nodes. Presentation names and provider metadata do not change runtime identity.
pub(crate) fn reconcile_nodes(id: &str, nodes: Vec<Value>, previous: &[Value]) -> Vec<Value> {
    use sha2::{Digest,Sha256};
    let identity=|node: &Value| { let mut value=node.clone(); if let Some(map)=value.as_object_mut() {map.remove("name");map.remove("extraParams");map.remove("atlas");} value.to_string() };
    let old: std::collections::HashMap<_,_>=previous.iter().map(|node|(identity(node),node["name"].clone())).collect();
    let mut seen=std::collections::HashSet::new();
    nodes.into_iter().filter_map(|mut node| {
        let key=identity(&node);
        if !seen.insert(key.clone()) {return None;}
        node["name"]=old.get(&key).cloned().unwrap_or_else(|| {
            let digest=format!("{:x}",Sha256::digest(key.as_bytes()));
            json!(format!("{} · {}-{}",node["name"].as_str().unwrap_or("Сервер"),id.chars().take(8).collect::<String>(),&digest[..16]))
        });
        Some(node)
    }).collect()
}
pub fn download(raw: &str, connected: bool, options: &crate::subscription_options::Options) -> Result<Downloaded, String> {
    if raw.trim().starts_with("vless://") {
        if raw.trim().lines().count()!=1 { return Err("Вставьте один VLESS-ключ".into()); }
        let mut diagnostics=Vec::new();
        return Ok(Downloaded {nodes:parse_report(raw.trim(),options,&mut diagnostics)?,options:options.clone(),diagnostics});
    }
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
    download_one_with_client(raw,connected,options,reqwest::blocking::Client::builder())
}
#[cfg(test)]
pub(crate) fn download_test_https(raw: &str, certificate: &[u8]) -> Result<Downloaded,String> {
    let certificate=reqwest::Certificate::from_der(certificate).map_err(|_|"Test certificate invalid")?;
    download_one_with_client(raw,false,&Default::default(),reqwest::blocking::Client::builder().add_root_certificate(certificate))
}
fn download_one_with_client(raw: &str, connected: bool, options: &crate::subscription_options::Options, builder: reqwest::blocking::ClientBuilder) -> Result<Downloaded, String> {
    crate::subscription_options::validate_agent(options.effective_user_agent())?;
    let u = crate::subscription_options::https_url(raw)?;
    let mut builder = builder.no_proxy();
    if connected {
        builder = builder.proxy(
            reqwest::Proxy::all("http://127.0.0.1:17890")
                .map_err(|_| "Ошибка локального прокси")?,
        );
    }
    let client = builder
        .timeout(std::time::Duration::from_secs(if options.fallback_url.is_some() { 9 } else { 30 }))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 {
                attempt.error("Слишком много перенаправлений подписки")
            } else if crate::subscription_options::https_url(attempt.url().as_str()).is_err() {
                attempt.error("Перенаправление подписки должно использовать безопасный HTTPS URL")
            } else if attempt.previous().first().is_some_and(|original| original.origin()!=attempt.url().origin()) {
                attempt.error("Перенаправление подписки на другой источник запрещено для защиты токена")
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|_| "Ошибка HTTPS клиента")?;
    let mut response = client
        .get(u)
        .header("Accept-Encoding", "gzip")
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
    let encoding=response.headers().get("content-encoding").and_then(|v|v.to_str().ok()).unwrap_or("identity").trim().to_ascii_lowercase();
    let mut body = Vec::new();
    (&mut response)
        .take(8 * 1024 * 1024 + 1)
        .read_to_end(&mut body)
        .map_err(|_| "Не удалось прочитать подписку")?;
    if body.len()>8*1024*1024 { return Err("Подписка превышает 8 МБ".into()); }
    let body=decode_http_body(body,&encoding)?;
    let body = String::from_utf8(body).map_err(|_| "Ответ подписки не является UTF-8")?;
    parse_response(&body, &headers, options, raw)
}
fn decode_http_body(body: Vec<u8>, encoding: &str) -> Result<Vec<u8>,String> {
    use std::io::Read;
    match encoding {
        "" | "identity" => Ok(body),
        "gzip" | "x-gzip" => {
            let mut decoded=Vec::new();
            flate2::read::MultiGzDecoder::new(body.as_slice()).take(8*1024*1024+1).read_to_end(&mut decoded)
                .map_err(|_| "Повреждённый gzip-ответ подписки")?;
            if decoded.len()>8*1024*1024 {return Err("Распакованная подписка превышает 8 МБ".into());}
            Ok(decoded)
        },
        _=>Err("Неподдерживаемое HTTP-сжатие подписки".into()),
    }
}
pub(crate) fn parse_response(body: &str, headers: &std::collections::BTreeMap<String,String>,
    options: &crate::subscription_options::Options, source: &str) -> Result<Downloaded,String> {
    // Remove outer directives before decoding: a fallback URL in a comment
    // must not cause a Base64 payload to be treated as plaintext proxy URIs.
    if body.len()>8*1024*1024 {return Err("Подписка превышает 8 МБ".into());}
    let (body,options)=crate::subscription_options::extract(clean_line(body),headers,options,source)?;
    let decoded=subscription_body(&body)?;
    let (body,options)=crate::subscription_options::extract(&decoded,headers,&options,source)?;
    let mut diagnostics=Vec::new();
    Ok(Downloaded {nodes:parse_report(&body,&options,&mut diagnostics)?,options,diagnostics})
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provider_rotation_preserves_source_scoped_selection_and_favorite() {
        use crate::model::{Settings,Subscription,SubscriptionSource};
        let original=json!({"name":"London","type":"vless","server":"192.0.2.1","port":443,"sni":"old.example"});
        for legacy in [false,true] {
            let mut old=reconcile_nodes("provider",vec![original.clone()],&[]);
            if legacy {old[0]["name"]=json!("London · provider-1");}
            let chosen=old[0]["name"].as_str().unwrap().to_owned();
            let mut rotated=original.clone();rotated["sni"]=json!("new.example");
            let nodes=reconcile_nodes("provider",vec![rotated],&old);
            // This is the 2.4.2 regression trigger: exact-name lookup loses the pin.
            assert!(!nodes.iter().any(|n|n["name"]==chosen));
            let expected=nodes[0]["name"].as_str().unwrap().to_owned();
            let mut settings=Settings::default();
            settings.subscriptions.push(Subscription {source:SubscriptionSource::Url,options:Default::default(),id:"provider".into(),name:"fixture".into(),masked_url:String::new(),updated_at:0,error:None,servers:old});
            crate::model::repository::normalize(&mut settings);
            settings.select_node(&chosen).unwrap();settings.favorites=vec![settings.selected_node_id.clone()];
            let mut next=settings.clone();next.subscriptions[0].servers=nodes;
            crate::model::repository::reconcile_references(&settings,&mut next);
            assert_eq!(next.selected,expected);
            assert_eq!(next.favorites,vec![next.selected_node_id.clone()]);
        }
    }
    fn provider_fixture() -> String {
        (0..34).map(|i|format!("vless://00000000-0000-0000-0000-{:012}@node{i}.example:8443?encryption=none&{}&x-durev-block=whitelist&x-durev-prio={}&provider-secret=sanitized#Germany%20{}%20🇩🇪%20→%20[📃%20Белые%20списки]",
            i+1,if i%2==0 {"type=xhttp&security=reality&sni=tls.example&pbk=BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc&sid=aabb&fp=chrome&mode=stream-one&path=%2Fxhttp&concurrency=4"}
                else {"type=ws&security=tls&sni=tls.example&host=cdn.example&path=%2Fws&fp=chrome&alpn=h2%2Chttp%2F1.1"},i%4,i)).collect::<Vec<_>>().join("\r\n")
    }
    #[test]
    fn provider_plain_and_all_base64_encodings_keep_unicode_metadata_and_runtime() {
        let fixture=provider_fixture();
        let variants=vec![fixture.clone(),STANDARD.encode(&fixture),STANDARD_NO_PAD.encode(&fixture),URL_SAFE.encode(&fixture),URL_SAFE_NO_PAD.encode(&fixture)];
        for body in variants {
            let result=parse_response(&format!("\u{feff}\r\n{body}\r\n"),&Default::default(),&Default::default(),"https://provider.example/sub/sanitized").unwrap();
            assert_eq!(result.nodes.len(),34);assert!(result.diagnostics.is_empty());
            for (i,node) in result.nodes.iter().enumerate() {
                assert_eq!(node["name"],format!("Germany {i} 🇩🇪 → [📃 Белые списки]"));
                assert_eq!(node["extraParams"]["x-durev-block"],"whitelist");
                assert_eq!(node["extraParams"]["provider-secret"],"sanitized");
                let stream=&node["xray"]["outbounds"][0]["streamSettings"];
                if i%2==0 {assert_eq!(stream["security"],"reality");assert_eq!(stream["xhttpSettings"]["xmux"]["maxConcurrency"],4);assert_eq!(stream["xhttpSettings"]["mode"],"stream-one");}
                else {assert_eq!(stream["wsSettings"]["headers"]["Host"],"cdn.example");assert_eq!(stream["tlsSettings"]["alpn"],json!(["h2","http/1.1"]));}
            }
        }
    }
    #[test]
    fn bundled_xray_accepts_provider_subscription_profiles() {
        use std::os::windows::process::CommandExt;
        let binary=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/Atlas.Xray.exe");
        for node in parse(&STANDARD.encode(provider_fixture())).unwrap().into_iter().take(2) {
            let profile=crate::xray_config::controlled_profile(&node["xray"],json!({"tag":"atlas","listen":"127.0.0.1","port":19001,"protocol":"socks","settings":{}})).unwrap();
            let path=std::env::temp_dir().join(format!("atlas-provider-test-{}.json",uuid::Uuid::new_v4()));
            std::fs::write(&path,serde_json::to_vec(&profile).unwrap()).unwrap();
            let result=std::process::Command::new(&binary).creation_flags(0x08000000).args(["run","-test","-config"]).arg(&path).output().unwrap();
            std::fs::remove_file(path).unwrap();
            assert!(result.status.success(),"{} {}",String::from_utf8_lossy(&result.stdout),String::from_utf8_lossy(&result.stderr));
        }
    }
    #[test]
    fn detection_rejects_empty_invalid_utf8_and_decoded_non_subscriptions() {
        assert!(parse(" \u{feff} ").unwrap_err().contains("Пустой"));
        assert!(parse("not base64!").unwrap_err().contains("Base64"));
        assert!(parse(&STANDARD.encode([255u8,254])).unwrap_err().contains("UTF-8"));
        assert!(parse(&STANDARD.encode("ordinary text")).unwrap_err().contains("поддерживаемых"));
        assert!(parse("vless://@bad:443").unwrap_err().contains("Нет пригодных"));
    }
    #[test]
    fn gzip_body_is_bounded_and_rejects_corruption() {
        use std::io::Write;
        let compress=|body:&[u8]| {
            let mut encoder=flate2::write::GzEncoder::new(Vec::new(),flate2::Compression::fast());
            encoder.write_all(body).unwrap();encoder.finish().unwrap()
        };
        let body=STANDARD.encode(provider_fixture());
        assert_eq!(decode_http_body(compress(body.as_bytes()),"gzip").unwrap(),body.as_bytes());
        assert!(decode_http_body(vec![1,2,3],"gzip").unwrap_err().contains("Повреждённый"));
        assert!(decode_http_body(compress(&vec![b'x';8*1024*1024+1]),"gzip").unwrap_err().contains("8 МБ"));
        assert!(decode_http_body(vec![],"unsupported").is_err());
    }
    #[test]
    fn provider_refresh_preserves_names_metadata_and_replaces_removed_or_modified_nodes() {
        let original=parse(&provider_fixture()).unwrap();
        let old=reconcile_nodes("provider",original.clone(),&[]);
        let mut changed=original[0].clone();changed["extraParams"]["x-durev-prio"]=json!("9");
        let mut modified=original[1].clone();modified["port"]=json!(9443);
        modified["xray"]["outbounds"][0]["settings"]["vnext"][0]["port"]=json!(9443);
        let next=reconcile_nodes("provider",vec![changed.clone(),modified,changed],&old);
        assert_eq!(next.len(),2);assert_eq!(next[0]["name"],old[0]["name"]);
        assert_eq!(next[0]["extraParams"]["x-durev-prio"],"9");assert_ne!(next[1]["name"],old[1]["name"]);
        assert_eq!(reconcile_nodes("provider",next.clone(),&next),next);
    }
    #[test]
    fn broken_provider_node_does_not_reject_valid_neighbors() {
        let fixture=format!("{}\r\n\u{feff}vless://@invalid.example:443\r\n",provider_fixture());
        let result=parse_response(&STANDARD.encode(fixture),&Default::default(),&Default::default(),"https://provider.example/sub/sanitized").unwrap();
        assert_eq!(result.nodes.len(),34);assert_eq!(result.diagnostics.len(),1);
        assert!(!result.diagnostics.join(" ").contains("vless://"));
    }
    #[test]
    fn mixed_vless_refresh_keeps_identity_and_skips_bad_nodes() {
        let a="vless://00000000-0000-0000-0000-000000000001@example.com:443?security=tls&type=ws#A";
        let b="trojan://password@other.example:443#B";
        let body=format!("{a}\nvless://@bad.example:443\nvless://id@bad.example:443?type=unknown\n{b}\n{a}");
        let result=parse_response(&STANDARD.encode(body),&Default::default(),&Default::default(),"https://subscription.example/key").unwrap();
        assert_eq!(result.nodes.len(),2); assert_eq!(result.diagnostics.len(),2);
        let first=reconcile_nodes("subscription",result.nodes,&[]);
        let next=reconcile_nodes("subscription",parse(&format!("{b}\n{a}\n{a}")).unwrap(),&first);
        assert_eq!(next.len(),2); assert_eq!(first[0],next[1]); assert_eq!(first[1],next[0]);
        assert_eq!(reconcile_nodes("subscription",parse(a).unwrap(),&next),vec![first[0].clone()]);
        assert_eq!(download(a,false,&Default::default()).unwrap().nodes,parse(a).unwrap());
    }
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
            Ok(Downloaded { nodes: vec![json!({"name":"backup"})], options: options.clone(), diagnostics:vec![] })
        }).unwrap();
        assert_eq!(requested, ["https://new.example/key", "https://backup.example/key"]);
        assert_eq!(result.nodes[0]["name"], "backup");
        requested.clear();
        download_with("https://old.example/key", &options, |url| {
            requested.push(url.to_owned());
            Ok(Downloaded { nodes: vec![], options: options.clone(), diagnostics:vec![] })
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
    matches!((before,current),(Some(a),Some(b)) if a.id==b.id && a.source==b.source && a.updated_at==b.updated_at && a.servers==b.servers && a.options==b.options)
}
#[cfg(test)]
mod refresh_tests {
    use super::*;
    #[test]
    fn stale_or_deleted_subscription_is_never_overwritten() {
        let a=crate::model::Subscription { source: Default::default(), options: Default::default(),id:"a".into(),name:"A".into(),masked_url:String::new(),updated_at:10,error:None,servers:vec![json!({"server":"old"})]};
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

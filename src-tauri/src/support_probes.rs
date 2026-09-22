//! Explicit export-time probes. Never switch selectors or modify networking.
use crate::core::ApiClient;
use serde_json::{json, Value};
use std::{error::Error, net::UdpSocket, time::{Duration, Instant}};

fn error_chain(error: &(dyn Error + 'static)) -> String {
    let mut parts = vec![error.to_string()];
    let mut source = error.source();
    while let Some(e) = source { parts.push(e.to_string()); source = e.source(); }
    parts.join(" -> ")
}
fn http(endpoint: &str, proxy: bool) -> Value {
    http_with(endpoint,proxy.then_some("http://127.0.0.1:17890"),Duration::from_secs(6))
}
fn http_with(endpoint: &str, proxy: Option<&str>, timeout: Duration) -> Value {
    let started = Instant::now();
    let at = crate::model::now();
    let host = url::Url::parse(endpoint).ok().and_then(|u|u.host_str().map(str::to_owned));
    let mut builder = reqwest::blocking::Client::builder().no_proxy()
        .connect_timeout(timeout.min(Duration::from_secs(3))).timeout(timeout)
        .redirect(reqwest::redirect::Policy::none());
    if let Some(proxy) = proxy { builder = builder.proxy(reqwest::Proxy::http(proxy).unwrap()); }
    let response = builder.build().and_then(|client| client.head(endpoint).send());
    let path = if proxy.is_some() { "local_mixed_proxy" } else { "windows_default_path" };
    match response {
        Ok(r) => json!({"path":path,"endpoint":endpoint,"controlHost":host,"startedAt":at,"elapsedMs":started.elapsed().as_millis(),
            "httpResponded":true,"controlSucceeded":r.status().is_success(),"status":r.status().as_u16(),"remoteAddress":r.remote_addr().map(|a|a.to_string())}),
        Err(e) => json!({"path":path,"endpoint":endpoint,"controlHost":host,"startedAt":at,"elapsedMs":started.elapsed().as_millis(),
            "httpResponded":false,"controlSucceeded":false,"timeout":e.is_timeout(),"connectError":e.is_connect(),"errorChain":error_chain(&e)}),
    }
}
fn eligible(node: &Value) -> bool {
    node.get("all").is_none() && node["type"].as_str().is_some_and(|kind| {
        !["direct","reject","rejectdrop","pass","passrule","compatible","dns"].contains(&kind.to_ascii_lowercase().replace('-', "").as_str())
    })
}
fn sample_nodes(proxies: &Value) -> Vec<String> {
    let all = proxies["proxies"].as_object();
    let mut names: Vec<String> = Vec::new();
    let mut selected = "ATLAS".to_owned();
    for _ in 0..8 {
        match proxies["proxies"][&selected]["now"].as_str() {
            Some(next) => selected = next.to_owned(), None => break,
        }
    }
    if all.is_some_and(|p|p.get(&selected).is_some_and(eligible)) { names.push(selected); }
    for alive in [false,true] {
        if let Some(all) = all {
            for (name,node) in all {
                if names.len() >= if alive {6} else {4} { break; }
                if eligible(node) && node["alive"] == alive && !names.contains(name) { names.push(name.clone()); }
            }
        }
    }
    names
}
fn dns() -> Value {
    let start = Instant::now();
    let result = (|| -> Result<Value, String> {
        let socket = UdpSocket::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        socket.set_read_timeout(Some(Duration::from_secs(3))).map_err(|e| e.to_string())?;
        socket.connect("127.0.0.1:11053").map_err(|e| e.to_string())?;
        let id = (crate::model::now() as u16).to_be_bytes();
        let mut query = vec![id[0],id[1],1,0,0,1,0,0,0,0,0,0];
        for label in "www.gstatic.com".split('.') { query.push(label.len() as u8); query.extend(label.as_bytes()); }
        query.extend([0,0,1,0,1]);
        socket.send(&query).map_err(|e| format!("DNS send: {e}"))?;
        let mut data = [0;4096];
        let count = socket.recv(&mut data).map_err(|e| format!("DNS receive: {e}"))?;
        if count < 12 || data[..2] != id || data[2] & 0x80 == 0 { return Err("Malformed DNS reply".into()); }
        Ok(json!({"response":true,"rcode":data[3]&15,"answerCount":u16::from_be_bytes([data[6],data[7]]),
            "truncated":data[2]&2 != 0,"bytes":count}))
    })();
    json!({"endpoint":"127.0.0.1:11053","question":"www.gstatic.com A",
        "elapsedMs":start.elapsed().as_millis(),"result":result.unwrap_or_else(|e| json!({"response":false,"error":e})),
        "scope":"Local DNS reply; fake-IP response does not prove upstream DNS or VPN success"})
}
pub(crate) fn valid_dns_name(name: &str) -> bool {
    name.len() <= 253 && name.contains('.') && name.split('.').all(|part|
        !part.is_empty() && part.len() <= 63 && !part.starts_with('-') && !part.ends_with('-') && part.bytes().all(|c|c.is_ascii_alphanumeric() || c==b'-'))
}
fn failed_domains(lines: &[String]) -> Vec<String> {
    let mut domains = vec!["www.gstatic.com".to_owned(),"cp.cloudflare.com".to_owned()];
    for line in lines.iter().rev().filter(|l| !l.starts_with("ATLAS_EVENT") && (l.contains("resolve failed") || l.contains("can't resolve ip"))) {
        if let Some(name) = line.split("--> ").nth(1).and_then(|s|s.split(':').next()) {
            if valid_dns_name(name) && name.parse::<std::net::IpAddr>().is_err() && !domains.iter().any(|s|s == name) { domains.push(name.into()); }
        }
        if domains.len() >= 6 { break; }
    }
    domains
}
fn dns_verdict(result: &Value) -> &'static str {
    if let Some(error)=result["error"].as_str() {
        if error.contains("SERVFAIL") {return "SERVFAIL";}
        if error.contains("REFUSED") {return "REFUSED";}
        if error.contains("NXDOMAIN") {return "NXDOMAIN";}
    }
    match result["Status"].as_u64() {
        Some(3) => "NXDOMAIN", Some(2) => "SERVFAIL", Some(5) => "REFUSED",
        Some(0) if result["Answer"].as_array().is_some_and(|a|!a.is_empty()) => "ANSWER",
        Some(0) => "NO_ANSWER", Some(_) => "DNS_ERROR", None => "QUERY_FAILED",
    }
}
pub(crate) fn upstream_dns(client: &ApiClient) -> Value {
    let domains = failed_domains(&client.logs().unwrap_or_default());
    let results = std::thread::scope(|scope| {
        let jobs: Vec<_> = domains.iter().map(|name|scope.spawn(move || {
            let path = format!("/dns/query?name={}&type=A",crate::latency::encode_name(name));
            let at = crate::model::now();
            let result = client.api("GET",&path,None).unwrap_or_else(|e|json!({"error":e}));
            json!({"name":name,"at":at,"classification":dns_verdict(&result),"result":result})
        })).collect();
        jobs.into_iter().map(|j|j.join().unwrap_or(Value::Null)).collect::<Vec<_>>()
    });
    json!({"scope":"Mihomo default resolver Exchange, bypasses fake-IP replies; not a direct-nameserver measurement. NXDOMAIN is a DNS response, not proof that every resolver agrees.","results":results})
}
pub fn run(client: Option<ApiClient>, deadline: Instant) -> Value {
    let started_at = crate::model::now();
    let mut http_results = Vec::new();
    let local_dns = std::thread::scope(|scope| {
        let dns_job = scope.spawn(dns);
        let mut jobs = Vec::new();
        for endpoint in crate::latency::ENDPOINTS {
            for proxy in [false,true] { jobs.push(scope.spawn(move || http(endpoint,proxy))); }
        }
        for job in jobs { http_results.push(job.join().unwrap_or_else(|_|json!({"error":"Probe worker panicked"}))); }
        dns_job.join().unwrap_or_else(|_|json!({"error":"DNS worker panicked"}))
    });
    // Query a bounded set of nodes individually. Unlike /group/delay this does
    // not ForceSet AUTO's selection. Include last failed and working histories.
    let resolver = client.as_ref().filter(|_|Instant::now()<deadline).map(upstream_dns);
    let node_tests = client.map(|c| {
        if Instant::now() >= deadline { return json!({"error":"Export deadline reached before node tests","tested":0}); }
        match c.api("GET","/proxies",None) {
        Ok(proxies) => {
            let names = sample_nodes(&proxies);
            let results: Vec<Value> = std::thread::scope(|scope| {
                let jobs: Vec<_> = names.iter().map(|name| {
                    let client = c.clone();
                    scope.spawn(move || {
                        let mut checks = Vec::new();
                        for (index, endpoint) in crate::latency::ENDPOINTS.iter().enumerate() {
                            if Instant::now() >= deadline {
                                checks.push(json!({"control":index,"skipped":"Export deadline reached"}));
                                break;
                            }
                            let start = Instant::now();
                            let at = crate::model::now();
                            let result = crate::latency::verified_probe(&client,name,endpoint);
                            checks.push(json!({"control":index,"startedAt":at,"elapsedMs":start.elapsed().as_millis(),
                                "result":result.unwrap_or_else(|e|json!({"error":e}))}));
                        }
                        json!({"name":name,"checks":checks})
                    })
                }).collect();
                jobs.into_iter().map(|j|j.join().unwrap_or_else(|_|json!({"error":"Node probe worker panicked"}))).collect()
            });
            json!({"tested":names.len(),"limit":6,"selection":"selected node, then failed and healthy samples; not the entire pool","results":results})
        }
        Err(e) => json!({"error":e,"tested":0}),
    }}).unwrap_or_else(||json!({"error":"No captured core client; node probes skipped","tested":0}));
    json!({"startedAt":started_at,"completedAt":crate::model::now(),"http":http_results,"localDns":local_dns,"upstreamDns":resolver,"nodeTests":node_tests,
        "scope":"Windows path obeys active TUN/WFP/rules; it is NOT an independent physical bypass. Mixed proxy obeys Atlas rules. HTTP success need not imply every VPN node works."})
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read,Write};
    #[test]
    fn dns_failures_are_classified_without_claiming_every_timeout_is_nxdomain() {
        assert_eq!(dns_verdict(&json!({"Status":3})),"NXDOMAIN");
        assert_eq!(dns_verdict(&json!({"Status":2})),"SERVFAIL");
        assert_eq!(dns_verdict(&json!({"Status":0})),"NO_ANSWER");
        assert_eq!(dns_verdict(&json!({"error":"timeout"})),"QUERY_FAILED");
        assert!(valid_dns_name("crl.anydesk.com"));
        assert!(!valid_dns_name("a.com&name=other.com"));
        assert_eq!(failed_domains(&["[TCP] dial DIRECT --> crl.anydesk.com:80 error: dns resolve failed".into()]).last().unwrap(),"crl.anydesk.com");
    }
    #[test]
    fn direct_and_group_nodes_never_count_as_vpn_evidence() {
        let mut p = json!({"proxies":{"ATLAS":{"now":"DIRECT","all":["DIRECT"]},
            "DIRECT":{"type":"Direct","alive":true},"named-direct":{"type":"Direct","alive":true},
            "PASS-RULE":{"type":"PassRule","alive":true},"reject-drop":{"type":"RejectDrop","alive":false},
            "failed":{"type":"Vless","alive":false},"healthy":{"type":"Vless","alive":true}}});
        assert_eq!(sample_nodes(&p),vec!["failed","healthy"]);
        p["proxies"]["ATLAS"]["now"] = json!("healthy");
        assert_eq!(sample_nodes(&p),vec!["healthy","failed"]);
        p["proxies"]["ATLAS"]["now"] = json!("ATLAS");
        assert_eq!(sample_nodes(&p),vec!["failed","healthy"]);
    }
    #[test]
    fn http_probe_records_status_and_bounds_a_stalled_peer() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = std::thread::spawn(move || {
            let (mut stream,_) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut bytes = [0;1024];
            let mut headers = Vec::new();
            while !headers.windows(4).any(|v|v==b"\r\n\r\n") {
                let size=stream.read(&mut bytes).unwrap(); if size==0 {return;}
                headers.extend_from_slice(&bytes[..size]);
            }
            stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        });
        let result = http_with(&format!("http://{address}/"),None,Duration::from_secs(2));
        worker.join().unwrap();
        assert_eq!(result["httpResponded"],true);
        assert_eq!(result["controlSucceeded"],false);
        assert_eq!(result["status"],503);
        let stalled = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let start=Instant::now();
        let result=http_with(&format!("http://{}/",stalled.local_addr().unwrap()),None,Duration::from_millis(150));
        assert_eq!(result["timeout"],true);
        assert!(result["errorChain"].as_str().unwrap().len()>10);
        assert!(start.elapsed()<Duration::from_secs(2));
    }
}

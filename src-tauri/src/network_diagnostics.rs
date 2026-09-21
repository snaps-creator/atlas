use crate::{
    core::ApiClient,
    model::{Route, Settings},
};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    net::{IpAddr, SocketAddr, ToSocketAddrs, UdpSocket},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    time::Duration,
};

const DNS_HIJACK_TEST_ADDRESS: &str = "203.0.113.53:53";
const IPV6_TEST_ADDRESS: &str = "[2606:4700:4700::1111]:443";

fn check(name: &str, ok: Option<bool>, detail: impl Into<String>) -> Value {
    json!({"name":name,"ok":ok,"detail":detail.into()})
}

fn protection_policy_error(settings: &Settings) -> Option<&'static str> {
    if settings.mode != "tun" {
        return Some("TUN Atlas выключен.");
    }
    if settings.routing_mode == crate::model::RoutingMode::Direct {
        return Some("Прямой режим: трафик идёт без VPN.");
    }
    let has_vpn_route = settings.routing_mode == crate::model::RoutingMode::Global || matches!(settings.default_route, Route::Proxy)
        || settings.groups.iter().any(|group| {
            group.enabled && !group.rules.is_empty() && matches!(group.route, Route::Proxy)
        });
    if !has_vpn_route {
        return Some("Нет активных правил или маршрута через VPN.");
    }
    let encrypted_dns = !settings.dns.servers.is_empty()
        && settings.dns.servers.iter().all(|server| {
            let endpoint = server.split('#').next().unwrap_or(server);
            url::Url::parse(endpoint)
                .ok()
                .is_some_and(|url| matches!(url.scheme(), "https" | "tls"))
        });
    if !encrypted_dns {
        return Some("DNS не зашифрован: используйте DNS over HTTPS или DNS over TLS.");
    }
    None
}

fn summarize_protection(settings: &Settings, checks: &[Value]) -> Value {
    if let Some(detail) = protection_policy_error(settings) {
        return json!({"secure":false,"detail":detail,"checkedAt":crate::model::now()});
    }
    let required = [
        "Ядро",
        "TUN / TCP / IPv4",
        "TUN / IPv6",
        "UDP через TUN",
        "DNS / утечки",
    ];
    if required
        .iter()
        .any(|name| !checks.iter().any(|item| item["name"] == **name))
    {
        return json!({"secure":false,"detail":"Проверка защиты не завершена: отсутствуют обязательные результаты.","checkedAt":crate::model::now()});
    }
    let failed = required.iter().find_map(|name| {
        checks
            .iter()
            .find(|item| item["name"] == **name)
            .filter(|item| item["ok"] != true)
    });
    match failed {
        Some(item) => json!({
            "secure":false,
            "detail":item["detail"].as_str().unwrap_or("Защищённый путь трафика не подтверждён."),
            "checkedAt":crate::model::now()
        }),
        None => json!({
            "secure":true,
            "detail":if matches!(settings.default_route, Route::Proxy) {
                "Трафик и DNS защищены туннелем Atlas; правила DIRECT применяются как задано."
            } else {
                "Правила Atlas активны: выбранный трафик идёт через VPN, DNS защищён."
            },
            "checkedAt":crate::model::now()
        }),
    }
}

pub fn protection_status(settings: &Settings, client: ApiClient) -> Value {
    if let Some(detail) = protection_policy_error(settings) {
        return json!({"secure":false,"detail":detail,"checkedAt":crate::model::now()});
    }
    summarize_protection(settings, &run(settings, client))
}

fn tun(c: &Value) -> bool {
    c["metadata"]["type"]
        .as_str()
        .is_some_and(|kind| kind.eq_ignore_ascii_case("tun"))
}

fn chain_has(c: &Value, name: &str) -> bool {
    c["chains"].as_array().is_some_and(|chains| {
        chains
            .iter()
            .filter_map(Value::as_str)
            .any(|chain| chain.eq_ignore_ascii_case(name))
    })
}

fn routed(c: &Value) -> bool {
    tun(c) && chain_has(c, "ATLAS") && !chain_has(c, "DIRECT") && !chain_has(c, "REJECT")
}

fn follows_route(c: &Value, route: &Route) -> bool {
    match route {
        Route::Proxy => routed(c),
        Route::Direct => tun(c) && chain_has(c, "DIRECT") && !chain_has(c, "REJECT"),
        Route::Block => rejected(c),
    }
}

// The probe is routed by the same rules as other traffic. MATCH is only the
// fallback; e.g. google.com can select ATLAS while the default is DIRECT.
fn configured_route(c: &Value, settings: &Settings) -> Option<Route> {
    match settings.routing_mode {
        crate::model::RoutingMode::Global => return Some(Route::Proxy),
        crate::model::RoutingMode::Direct => return Some(Route::Direct),
        _ => {}
    }
    let kind = c["rule"].as_str()?.replace('-', "").to_uppercase();
    if kind == "MATCH" {
        return Some(settings.default_route.clone());
    }
    let payload = c["rulePayload"].as_str()?;
    settings
        .groups
        .iter()
        .filter(|group| group.enabled)
        .find_map(|group| {
            group
                .rules
                .iter()
                .filter_map(|rule| crate::rules::normalize(rule).ok())
                .find_map(|rule| {
                    let matches = if rule.kind == "PROCESS-DOMAIN" && kind == "AND" {
                        rule.value.split_once('|').is_some_and(|(process, domain)| {
                            payload == format!("((PROCESS-NAME,{process}),(DOMAIN,{domain}))")
                        })
                    } else {
                        let same_kind = rule.kind.replace('-', "") == kind
                            || (rule.kind == "IP-CIDR6" && kind == "IPCIDR");
                        same_kind && rule.value.eq_ignore_ascii_case(payload)
                    };
                    matches.then(|| group.route.clone())
                })
        })
}

fn follows_configured_route(c: &Value, settings: &Settings) -> bool {
    configured_route(c, settings).is_some_and(|route| follows_route(c, &route))
}

fn rejected(c: &Value) -> bool {
    tun(c) && chain_has(c, "REJECT") && !chain_has(c, "DIRECT")
}

fn metadata_port(c: &Value, key: &str) -> Option<u16> {
    c["metadata"][key]
        .as_u64()
        .and_then(|port| u16::try_from(port).ok())
        .or_else(|| {
            c["metadata"][key]
                .as_str()
                .and_then(|port| port.parse().ok())
        })
}

fn destination_matches(
    c: &Value,
    network: &str,
    host: &str,
    addresses: &HashSet<IpAddr>,
    port: u16,
) -> bool {
    let network_matches = c["metadata"]["network"]
        .as_str()
        .is_some_and(|value| value.eq_ignore_ascii_case(network));
    let port_matches = metadata_port(c, "destinationPort") == Some(port);
    let host_matches = !host.is_empty()
        && c["metadata"]["host"].as_str().is_some_and(|value| {
            value
                .trim_end_matches('.')
                .eq_ignore_ascii_case(host.trim_end_matches('.'))
        });
    let ip_matches = c["metadata"]["destinationIP"]
        .as_str()
        .and_then(|value| value.parse::<IpAddr>().ok())
        .is_some_and(|address| addresses.contains(&address));
    network_matches && port_matches && (host_matches || ip_matches)
}

fn resolve(host: &str, port: u16, ipv6: bool) -> HashSet<IpAddr> {
    (host, port)
        .to_socket_addrs()
        .map(|addresses| {
            addresses
                .map(|address| address.ip())
                .filter(|address| address.is_ipv6() == ipv6)
                .collect()
        })
        .unwrap_or_default()
}

// Capture while the probe runs; short-lived flows may disappear after it finishes.
fn observe<T>(client: &ApiClient, operation: impl FnOnce() -> T) -> (T, Vec<Value>) {
    let baseline: HashSet<String> = client
        .api("GET", "/connections", None)
        .ok()
        .and_then(|value| value["connections"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|connection| connection["id"].as_str().map(str::to_owned))
        .collect();
    let done = Arc::new(AtomicBool::new(false));
    let entries = Arc::new(Mutex::new(HashMap::new()));
    let stop = done.clone();
    let out = entries.clone();
    let api = client.clone();
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        let _ = ready_tx.send(());
        while !stop.load(Ordering::Relaxed) {
            if let Ok(value) = api.api("GET", "/connections", None) {
                if let Some(list) = value["connections"].as_array() {
                    let mut stored = out.lock().unwrap();
                    for connection in list {
                        if let Some(id) = connection["id"].as_str() {
                            if baseline.contains(id) {
                                continue;
                            }
                            stored.insert(id.to_owned(), connection.clone());
                        }
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    });
    let _ = ready_rx.recv_timeout(Duration::from_secs(1));
    let value = operation();
    std::thread::sleep(Duration::from_millis(100));
    done.store(true, Ordering::Relaxed);
    let _ = worker.join();
    let captured = entries.lock().unwrap().values().cloned().collect();
    (value, captured)
}

fn https_probe(host: &str) -> Result<String, ()> {
    let http = reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|_| ())?;
    let response = http
        .get(format!("https://{host}/?atlas={}", uuid::Uuid::new_v4()))
        .send()
        .and_then(|response| response.error_for_status())
        .map_err(|_| ())?;
    // Keep the socket observable until Mihomo's connection sampler sees it.
    std::thread::sleep(Duration::from_millis(500));
    response.text().map_err(|_| ())
}

// A TUN stack can accept TCP locally before REJECT is applied upstream.
// Only an authenticated remote TLS/HTTP response proves external connectivity.
fn https_reachable(target: SocketAddr) -> bool {
    reqwest::blocking::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(8))
        .build()
        .is_ok_and(|http| http.get(format!("https://{target}/")).send().is_ok())
}

fn dns_query() -> Result<(u16, usize), ()> {
    let socket = UdpSocket::bind("0.0.0.0:0").map_err(|_| ())?;
    let source_port = socket.local_addr().map_err(|_| ())?.port();
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|_| ())?;
    // TEST-NET-3 is not a public DNS resolver. A valid response from this
    // destination proves that Atlas intercepted the packet before the Internet.
    socket.connect(DNS_HIJACK_TEST_ADDRESS).map_err(|_| ())?;
    let id = uuid::Uuid::new_v4();
    let mut query = vec![
        id.as_bytes()[0],
        id.as_bytes()[1],
        1,
        0,
        0,
        1,
        0,
        0,
        0,
        0,
        0,
        0,
    ];
    for label in format!("{}.example.com", id.simple()).split('.') {
        query.push(label.len() as u8);
        query.extend_from_slice(label.as_bytes());
    }
    query.extend_from_slice(&[0, 0, 1, 0, 1]);
    socket.send(&query).map_err(|_| ())?;
    let mut response = [0u8; 4096];
    let size = socket.recv(&mut response).map_err(|_| ())?;
    std::thread::sleep(Duration::from_millis(300));
    if size < 12 || response[..2] != query[..2] || response[2] & 0x80 == 0 {
        return Err(());
    }
    Ok((source_port, size))
}

pub fn run(settings: &Settings, client: ApiClient) -> Vec<Value> {
    let alive = client.api("GET", "/version", None).is_ok();
    let mut results = vec![check(
        "Ядро",
        Some(alive),
        if alive {
            "Доступно"
        } else {
            "Не запущено"
        },
    )];
    if !alive {
        return results;
    }

    let mut tcp_targets = vec![("TUN / TCP / IPv4", "api4.ipify.org", false)];
    if settings.dns.ipv6 {
        tcp_targets.push(("TUN / IPv6", "api6.ipify.org", true));
    }
    for (name, host, ipv6) in tcp_targets {
        let addresses = resolve(host, 443, ipv6);
        let (response, entries) = observe(&client, || https_probe(host));
        let flows: Vec<_> = entries
            .iter()
            .filter(|connection| destination_matches(connection, "tcp", host, &addresses, 443))
            .collect();
        let valid = response
            .as_ref()
            .ok()
            .and_then(|ip| ip.trim().parse::<IpAddr>().ok())
            .is_some_and(|ip| ip.is_ipv6() == ipv6);
        let confirmed = !flows.is_empty()
            && flows
                .iter()
                .all(|connection| follows_configured_route(connection, settings));
        let ok = Some(valid && confirmed && settings.mode == "tun");
        let detail = if valid && confirmed {
            format!(
                "Ответ {} через TUN по настроенным правилам. Запрос выполнен без системного прокси.",
                response.unwrap().trim()
            )
        } else if valid {
            "Ответ получен, но соединение не найдено среди TUN-потоков Atlas.".into()
        } else {
            "HTTPS-запрос не завершился через проверяемый сетевой путь.".into()
        };
        results.push(check(name, ok, detail));
    }

    if !settings.dns.ipv6 {
        let target: SocketAddr = IPV6_TEST_ADDRESS.parse().unwrap();
        let addresses = HashSet::from([target.ip()]);
        let (connected, entries) = observe(&client, || {
            let connected = https_reachable(target);
            std::thread::sleep(Duration::from_millis(300));
            connected
        });
        let flows: Vec<_> = entries
            .iter()
            .filter(|connection| destination_matches(connection, "tcp", "", &addresses, 443))
            .collect();
        let tunneled = !flows.is_empty() && flows.iter().all(|connection| routed(connection));
        let safely_rejected = !connected
            && (flows.is_empty()
                || flows
                    .iter()
                    .all(|connection| rejected(connection) || routed(connection)));
        let safe = settings.mode == "tun" && (tunneled || safely_rejected);
        let detail: String = if connected && tunneled {
            "IPv6-соединение фактически прошло через TUN → ATLAS.".into()
        } else if safely_rejected {
            "Проверочный HTTPS-запрос по IPv6 заблокирован; неподтверждённых маршрутов не обнаружено."
                .into()
        } else if connected {
            "IPv6-соединение установлено вне подтверждённого пути TUN → ATLAS.".into()
        } else {
            "IPv6-соединение не установлено, но обнаружена неподтверждённая цепочка маршрута."
                .into()
        };
        results.push(check("TUN / IPv6", Some(safe), detail));
    }

    let mut udp_port = 0;
    let (udp, entries) = observe(&client, || -> Result<(), ()> {
        let address = ("stun.l.google.com", 19302)
            .to_socket_addrs()
            .map_err(|_| ())?
            .find(|address| address.is_ipv4())
            .ok_or(())?;
        let socket = UdpSocket::bind("0.0.0.0:0").map_err(|_| ())?;
        udp_port = socket.local_addr().map_err(|_| ())?.port();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .map_err(|_| ())?;
        socket.connect(address).map_err(|_| ())?;
        let mut query = vec![0, 1, 0, 0, 0x21, 0x12, 0xa4, 0x42];
        query.extend_from_slice(&uuid::Uuid::new_v4().as_bytes()[..12]);
        socket.send(&query).map_err(|_| ())?;
        let mut response = [0u8; 1024];
        let size = socket.recv(&mut response).map_err(|_| ())?;
        std::thread::sleep(Duration::from_millis(300));
        if size < 20 || response[..2] != [1, 1] || response[4..20] != query[4..20] {
            return Err(());
        }
        Ok(())
    });
    let flow = entries.iter().find(|connection| {
        connection["metadata"]["network"] == "udp"
            && metadata_port(connection, "sourcePort") == Some(udp_port)
    });
    results.push(check(
        "UDP через TUN",
        flow.map(|connection| udp.is_ok() && follows_configured_route(connection, settings)),
        if udp.is_ok()
            && flow.is_some_and(|connection| follows_configured_route(connection, settings))
        {
            "Ответ STUN и совпадающий исходный порт подтверждают UDP через TUN по настроенному маршруту."
        } else if udp.is_err() {
            "STUN-сервер не ответил: передача UDP не подтверждена. Это не доказывает утечку."
        } else if flow.is_none() {
            "Ответ STUN получен, но тестовый UDP-поток не найден в Atlas."
        } else {
            "Ответ STUN получен, но путь UDP не соответствует сработавшему правилу Atlas."
        },
    ));

    let (dns, dns_entries) = observe(&client, dns_query);
    let dns_port = dns.as_ref().ok().map(|(port, _)| *port);
    let dns_flows: Vec<_> = dns_entries
        .iter()
        .filter(|connection| {
            connection["metadata"]["network"] == "udp"
                && metadata_port(connection, "sourcePort") == dns_port
        })
        .collect();
    let unexpected_route = dns_flows
        .iter()
        .any(|connection| chain_has(connection, "DIRECT") || chain_has(connection, "REJECT"));
    let dns_ok = dns.is_ok() && settings.mode == "tun" && !unexpected_route;
    results.push(check(
        "DNS / утечки",
        Some(dns_ok),
        if dns_ok {
            "Запрос к зарезервированному TEST-NET DNS-адресу получил корректный ответ: DNS фактически перехвачен Atlas до выхода в Интернет."
        } else if dns.is_err() {
            "Atlas не перехватил тестовый DNS-запрос к зарезервированному адресу."
        } else {
            "DNS-ответ получен, но обнаружен DIRECT/REJECT вместо защищённой обработки Atlas."
        },
    ));

    let observed = client
        .api("GET", "/connections", None)
        .ok()
        .and_then(|value| value["connections"].as_array().cloned())
        .unwrap_or_default();
    let telegram: Vec<_> = observed
        .iter()
        .filter(|connection| {
            connection["metadata"]["process"]
                .as_str()
                .is_some_and(|process| process.eq_ignore_ascii_case("telegram.exe"))
        })
        .collect();
    let telegram_ok = !telegram.is_empty()
        && telegram.iter().all(|connection| {
            routed(connection) && connection["download"].as_u64().unwrap_or(0) > 0
        });
    results.push(check(
        "Telegram Desktop",
        if telegram.is_empty() { None } else { Some(telegram_ok) },
        if telegram_ok {
            "Есть полученные данные у наблюдаемых соединений Telegram через TUN → ATLAS."
        } else {
            "Трафик Telegram через TUN не подтверждён. Отключите встроенный прокси и создайте трафик перед проверкой."
        },
    ));
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_tcp_accept_is_not_remote_https_connectivity() {
        use std::io::Read;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let server = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            while std::time::Instant::now() < deadline {
                if let Ok((mut socket, _)) = listener.accept() {
                    socket
                        .set_read_timeout(Some(Duration::from_secs(1)))
                        .unwrap();
                    let mut hello = [0; 1024];
                    let _ = socket.read(&mut hello);
                    return true; // Accept TCP, then reject without a TLS response.
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            false
        });
        assert!(!https_reachable(address));
        assert!(server.join().unwrap());
    }

    fn protected_checks() -> Vec<Value> {
        [
            "Ядро",
            "TUN / TCP / IPv4",
            "TUN / IPv6",
            "UDP через TUN",
            "DNS / утечки",
        ]
        .into_iter()
        .map(|name| check(name, Some(true), "ok"))
        .collect()
    }

    #[test]
    fn probes_follow_rule_overrides_instead_of_default_route() {
        let mut settings = Settings::default();
        settings.default_route = Route::Direct;
        settings.groups = crate::rules::import("rules:\n - DOMAIN-SUFFIX,google.com,ATLAS\n - IP-CIDR,192.0.2.0/24,DIRECT,no-resolve\n - DOMAIN,blocked.example,REJECT").unwrap().groups;
        let mut flow = json!({"metadata":{"type":"Tun","network":"udp"},"rule":"DomainSuffix","rulePayload":"google.com","chains":["node","ATLAS"]});
        assert!(follows_configured_route(&flow, &settings));
        flow["chains"] = json!(["DIRECT"]);
        assert!(!follows_configured_route(&flow, &settings));
        settings.default_route = Route::Proxy;
        flow["rule"] = json!("IPCIDR");
        flow["rulePayload"] = json!("192.0.2.0/24");
        assert!(follows_configured_route(&flow, &settings));
        flow["rule"] = json!("Match");
        assert!(!follows_configured_route(&flow, &settings));
        flow["chains"] = json!(["node", "ATLAS"]);
        assert!(follows_configured_route(&flow, &settings));
        flow["rule"] = json!("Domain");
        flow["rulePayload"] = json!("blocked.example");
        assert!(!follows_configured_route(&flow, &settings));
        flow["chains"] = json!(["REJECT"]);
        assert!(follows_configured_route(&flow, &settings));
        flow["rulePayload"] = json!("unknown.example");
        assert!(!follows_configured_route(&flow, &settings));
        flow["rule"] = json!("DomainSuffix");
        flow["rulePayload"] = json!("google.com");
        flow["chains"] = json!(["node", "ATLAS"]);
        settings.groups[0].enabled = false;
        assert!(!follows_configured_route(&flow, &settings));
    }

    #[test]
    fn local_proxy_and_direct_are_not_tun_evidence() {
        assert!(!routed(
            &json!({"metadata":{"type":"HTTP"},"chains":["ATLAS"]})
        ));
        assert!(!routed(
            &json!({"metadata":{"type":"Tun"},"chains":["DIRECT"]})
        ));
        assert!(routed(
            &json!({"metadata":{"type":"Tun"},"chains":["node","ATLAS"]})
        ));
        assert!(follows_route(
            &json!({"metadata":{"type":"Tun"},"chains":["DIRECT"]}),
            &Route::Direct
        ));
    }

    #[test]
    fn destination_matches_host_or_resolved_ip_and_numeric_ports() {
        let addresses = HashSet::from(["198.18.0.42".parse().unwrap()]);
        assert!(destination_matches(
            &json!({"metadata":{"network":"tcp","host":"api4.ipify.org","destinationIP":"","destinationPort":"443"}}),
            "tcp",
            "api4.ipify.org",
            &addresses,
            443,
        ));
        assert!(destination_matches(
            &json!({"metadata":{"network":"TCP","host":"","destinationIP":"198.18.0.42","destinationPort":443}}),
            "tcp",
            "api4.ipify.org",
            &addresses,
            443,
        ));
    }

    #[test]
    fn protection_accepts_selective_rules_with_encrypted_dns_and_real_checks() {
        let mut settings = Settings::default();
        settings.default_route = Route::Proxy;
        assert_eq!(
            summarize_protection(&settings, &protected_checks())["secure"],
            true
        );

        settings.default_route = Route::Direct;
        assert_eq!(
            summarize_protection(&settings, &protected_checks())["secure"],
            false
        );
        settings.groups.push(crate::model::RuleGroup {
            id: "proxy".into(),
            name: "proxy".into(),
            description: String::new(),
            enabled: true,
            route: Route::Proxy,
            rules: vec![crate::model::Rule {
                kind: "DOMAIN".into(),
                value: "example.com".into(),
                no_resolve: false,
            }],
        });
        assert_eq!(
            summarize_protection(&settings, &protected_checks())["secure"],
            true
        );
        settings.default_route = Route::Proxy;
        settings.dns.servers = vec!["1.1.1.1".into()];
        assert_eq!(
            summarize_protection(&settings, &protected_checks())["secure"],
            false
        );

        settings.dns.servers = vec!["https://1.1.1.1/dns-query".into()];
        let mut checks = protected_checks();
        checks[3]["ok"] = json!(false);
        assert_eq!(summarize_protection(&settings, &checks)["secure"], false);
    }

    #[test]
    fn missing_or_unknown_checks_never_report_protected() {
        let mut settings = Settings::default();
        settings.default_route = Route::Proxy;
        assert_eq!(summarize_protection(&settings, &[])["secure"], false);
        for index in 0..protected_checks().len() {
            let mut checks = protected_checks();
            checks.remove(index);
            assert_eq!(summarize_protection(&settings, &checks)["secure"], false);
            let mut checks = protected_checks();
            checks[index]["ok"] = Value::Null;
            assert_eq!(summarize_protection(&settings, &checks)["secure"], false);
        }
    }
}

use crate::{model::*, rules};
use serde_json::{json, Value};
pub fn generate(s: &Settings, secret: &str) -> Result<String, String> {
    if !(MIN_AUTO_TEST_INTERVAL_SECONDS..=MAX_AUTO_TEST_INTERVAL_SECONDS)
        .contains(&s.auto_test_interval_seconds)
    {
        return Err("Интервал проверки серверов должен быть от 30 секунд до 60 минут".into());
    }
    let proxies = s.servers();
    if proxies.is_empty() {
        return Err("Сначала добавьте подписку с серверами".into());
    }
    let names: Vec<String> = proxies
        .iter()
        .map(|p| p["name"].as_str().unwrap_or_default().to_owned())
        .collect();
    let mut unique = std::collections::HashSet::new();
    for n in &names {
        if n.is_empty()
            || ["ATLAS", "AUTO", "FAILOVER", "DIRECT", "REJECT", "GLOBAL"].contains(&n.as_str())
            || !unique.insert(n)
        {
            return Err(
                "Названия серверов должны быть уникальными и не совпадать с системными группами"
                    .into(),
            );
        }
    }
    if !["AUTO", "FAILOVER"].contains(&s.selected.as_str()) && !names.contains(&s.selected) {
        return Err("Выбранный сервер отсутствует в подписках".into());
    }
    if s.dns.servers.is_empty() {
        return Err("Укажите хотя бы один DNS-сервер".into());
    }
    for dns in &s.dns.servers {
        if dns.parse::<std::net::IpAddr>().is_err() {
            let u = url::Url::parse(dns).map_err(|_| "Некорректный DNS-сервер")?;
            if !["https", "tls"].contains(&u.scheme()) {
                return Err("DNS должен быть IP-адресом, HTTPS или TLS URL".into());
            }
        }
    }
    let mut selection = vec![s.selected.clone(), "AUTO".into(), "FAILOVER".into()];
    selection.extend(names.clone());
    let mut seen = std::collections::HashSet::new();
    selection.retain(|n| seen.insert(n.clone()));
    let mut doc: Value = json!({"mixed-port":17890,"allow-lan":false,"bind-address":"127.0.0.1","mode":"rule","log-level":"warning","ipv6":s.dns.ipv6,"find-process-mode":"always","external-controller":"127.0.0.1:19090","secret":secret,"profile":{"store-selected":false},"tun":{"enable":s.mode=="tun","stack":"mixed","auto-route":true,"strict-route":true,"auto-detect-interface":true,"dns-hijack":["any:53"]},"dns":{"enable":true,"listen":"127.0.0.1:11053","ipv6":s.dns.ipv6,"enhanced-mode":if s.dns.fake_ip{"fake-ip"}else{"redir-host"},"fake-ip-range":"198.18.0.1/16","nameserver":s.dns.servers,"default-nameserver":["1.1.1.1","8.8.8.8"]},"proxies":proxies,"proxy-groups":[{"name":"ATLAS","type":"select","proxies":selection},{"name":"AUTO","type":"url-test","proxies":names,"url":"https://www.gstatic.com/generate_204","interval":s.auto_test_interval_seconds,"tolerance":50,"lazy":false},{"name":"FAILOVER","type":"fallback","proxies":names,"url":"https://www.gstatic.com/generate_204","interval":s.auto_test_interval_seconds,"lazy":false}],"rules":rules::compile(s)?});
    doc["mode"] = json!(s.routing_mode);
    // Pin global traffic to Atlas instead of Mihomo's generated DIRECT default.
    doc["proxy-groups"].as_array_mut().unwrap().push(json!({"name":"GLOBAL","type":"select","proxies":["ATLAS"]}));
    // Match Clash Verge's warm-connection URL latency measurement.
    doc["unified-delay"] = json!(true);
    if s.mode == "tun" {
        for node in doc["proxies"].as_array_mut().unwrap() {
            if ["ss", "vmess", "vless", "trojan", "socks5"]
                .contains(&node["type"].as_str().unwrap_or(""))
                && node.get("udp").is_none()
            {
                node["udp"] = json!(true);
            }
        }
        doc["ipv6"] = json!(true);
        doc["tun"]["device"] = json!("Atlas-TUN");
        // The top-level Mihomo TUN IPv4 address derives from fake-ip-range.
        // Do not reuse another client's default 198.18.0.1 address.
        doc["dns"]["fake-ip-range"] = json!("198.19.0.1/16");
        doc["tun"]["inet6-address"] = json!(["fd72:6174:6c61::1/126"]);
        doc["tun"]["route-address"] = json!(["0.0.0.0/1", "128.0.0.0/1", "::/1", "8000::/1"]);
        doc["tun"]["dns-hijack"] = json!(["any:53", "tcp://any:53"]);
        doc["tun"]["udp-timeout"] = json!(300);
        doc["tun"]["mtu"] = json!(1500);
        doc["dns"]["nameserver"] = json!(s
            .dns
            .servers
            .iter()
            .map(|server| format!("{}#{}", server.split('#').next().unwrap(), if s.routing_mode == RoutingMode::Direct { "DIRECT" } else { "ATLAS" }))
            .collect::<Vec<_>>());
        doc["dns"]["proxy-server-nameserver"] =
            json!(["https://1.1.1.1/dns-query", "https://8.8.8.8/dns-query"]);
        doc["dns"]["default-nameserver"] =
            json!(["https://1.1.1.1/dns-query", "https://8.8.8.8/dns-query"]);
        // Capture all destinations; DIRECT/PROXY/REJECT are decided inside the core.
        // Preserve the user's default route rather than replacing MATCH unconditionally.
        doc["tun"]["disable-icmp-forwarding"] = json!(true);
        doc["dns"]["direct-nameserver"] = doc["dns"]["nameserver"].clone();
        let rules = doc["rules"].as_array_mut().unwrap();
        if !s.dns.ipv6 {
            rules.insert(0, json!("IP-CIDR6,::/0,REJECT,no-resolve"));
        }
    }
    serde_yaml::to_string(&doc).map_err(|_| "Ошибка генерации YAML".into())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_fail_closed_without_nodes() {
        assert!(generate(&Settings::default(), "secret").is_err())
    }
    #[test]
    fn generated_dns_and_route() {
        let mut s = Settings::default();
        s.mode = "system".into();
        s.subscriptions.push(Subscription{id:"a".into(),name:"test".into(),masked_url:"hidden".into(),updated_at:0,error:None,servers:vec![json!({"name":"test","type":"ss","server":"127.0.0.1","port":443,"cipher":"aes-128-gcm","password":"test"})]});
        let y: Value = serde_yaml::from_str(&generate(&s, "secret").unwrap()).unwrap();
        assert_eq!(y["rules"][0], "MATCH,DIRECT");
        assert_eq!(y["allow-lan"], false);
        assert_eq!(y["unified-delay"], true);
        assert_eq!(y["dns"]["enhanced-mode"], "fake-ip");
        assert!(!y["proxy-groups"][1]["proxies"]
            .as_array()
            .unwrap()
            .contains(&json!("DIRECT")));
        s.mode = "tun".into();
        s.default_route = Route::Proxy;
        let tun: Value = serde_yaml::from_str(&generate(&s, "secret").unwrap()).unwrap();
        assert_eq!(tun["tun"]["enable"], true);
        assert_eq!(tun["tun"]["strict-route"], true);
        assert_eq!(tun["tun"]["device"], "Atlas-TUN");
        assert_eq!(tun["dns"]["fake-ip-range"], "198.19.0.1/16");
        assert_eq!(tun["tun"]["inet6-address"][0], "fd72:6174:6c61::1/126");
        assert_eq!(tun["ipv6"], true); // Keep IPv6 captured even when resolution is disabled.
        assert_eq!(tun["rules"][0], "IP-CIDR6,::/0,REJECT,no-resolve");
        assert_eq!(
            tun["rules"].as_array().unwrap().last().unwrap(),
            "MATCH,ATLAS"
        );
        assert_eq!(tun["tun"]["dns-hijack"][1], "tcp://any:53");
        assert!(tun["dns"]["nameserver"][0]
            .as_str()
            .unwrap()
            .ends_with("#ATLAS"));
        assert_eq!(tun["proxies"][0]["udp"], true);
        assert_eq!(tun["proxy-groups"][1]["interval"], 300);
        assert_eq!(tun["proxy-groups"][2]["interval"], 300);
        s.auto_test_interval_seconds = 60;
        let faster: Value = serde_yaml::from_str(&generate(&s, "secret").unwrap()).unwrap();
        assert_eq!(faster["proxy-groups"][1]["interval"], 60);
        assert_eq!(faster["proxy-groups"][2]["interval"], 60);
        s.default_route = Route::Direct;
        let direct: Value = serde_yaml::from_str(&generate(&s, "secret").unwrap()).unwrap();
        assert_eq!(
            direct["rules"].as_array().unwrap().last().unwrap(),
            "MATCH,DIRECT"
        );
        s.dns.ipv6 = true;
        let v6: Value = serde_yaml::from_str(&generate(&s, "secret").unwrap()).unwrap();
        assert_ne!(v6["rules"][0], "IP-CIDR6,::/0,REJECT,no-resolve");
        s.routing_mode = RoutingMode::Global;
        let global: Value = serde_yaml::from_str(&generate(&s, "secret").unwrap()).unwrap();
        assert_eq!(global["mode"], "global");
        assert_eq!(global["proxy-groups"][3]["proxies"], json!(["ATLAS"]));
        s.routing_mode = RoutingMode::Direct;
        let direct: Value = serde_yaml::from_str(&generate(&s, "secret").unwrap()).unwrap();
        assert_eq!(direct["mode"], "direct");
        assert!(direct["dns"]["nameserver"][0].as_str().unwrap().ends_with("#DIRECT"));
        let mut legacy = serde_json::to_value(&s).unwrap();
        legacy.as_object_mut().unwrap().remove("routingMode");
        assert_eq!(serde_json::from_value::<Settings>(legacy).unwrap().routing_mode, RoutingMode::Rule);
    }
}

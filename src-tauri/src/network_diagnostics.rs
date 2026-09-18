use crate::{core::ApiClient, model::Settings};
use serde_json::{json, Value};
use std::{
    net::{ToSocketAddrs, UdpSocket},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
fn check(name: &str, ok: Option<bool>, detail: impl Into<String>) -> Value {
    json!({"name":name,"ok":ok,"detail":detail.into()})
}
fn routed(c: &Value) -> bool {
    c["metadata"]["type"]
        .as_str()
        .is_some_and(|t| t.eq_ignore_ascii_case("tun"))
        && c["chains"].as_array().is_some_and(|a| {
            a.iter().any(|v| v == "ATLAS") && !a.iter().any(|v| v == "DIRECT" || v == "REJECT")
        })
}
// Capture while the probe runs; short-lived flows may disappear after it finishes.
fn observe<T>(client: &ApiClient, operation: impl FnOnce() -> T) -> (T, Vec<Value>) {
    let baseline: std::collections::HashSet<String> = client
        .api("GET", "/connections", None)
        .ok()
        .and_then(|v| v["connections"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|c| c["id"].as_str().map(str::to_owned))
        .collect();
    let done = Arc::new(AtomicBool::new(false));
    let entries = Arc::new(Mutex::new(std::collections::HashMap::new()));
    let stop = done.clone();
    let out = entries.clone();
    let api = client.clone();
    let worker = std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            if let Ok(v) = api.api("GET", "/connections", None) {
                if let Some(list) = v["connections"].as_array() {
                    let mut stored = out.lock().unwrap();
                    for c in list {
                        if let Some(id) = c["id"].as_str() {
                            if baseline.contains(id) {
                                continue;
                            }
                            stored.insert(id.to_owned(), c.clone());
                        }
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    });
    let value = operation();
    done.store(true, Ordering::Relaxed);
    let _ = worker.join();
    let captured = entries.lock().unwrap().values().cloned().collect();
    (value, captured)
}
pub fn run(s: &Settings, client: ApiClient) -> Vec<Value> {
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
    for (name, host, ipv6) in [
        ("TUN / TCP / IPv4", "api4.ipify.org", false),
        ("TUN / IPv6", "api6.ipify.org", true),
    ] {
        let (response, entries) = observe(&client, || -> Result<String, ()> {
            let http = reqwest::blocking::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(8))
                .build()
                .map_err(|_| ())?;
            let response = http
                .get(format!("https://{host}/?atlas={}", uuid::Uuid::new_v4()))
                .send()
                .and_then(|r| r.error_for_status())
                .and_then(|r| r.text())
                .map_err(|_| ());
            std::thread::sleep(Duration::from_millis(300));
            response
        });
        let flows: Vec<_> = entries
            .iter()
            .filter(|c| c["metadata"]["host"] == host)
            .collect();
        let valid = response
            .as_ref()
            .ok()
            .and_then(|ip| ip.trim().parse::<std::net::IpAddr>().ok())
            .is_some_and(|ip| ip.is_ipv6() == ipv6);
        let confirmed = !flows.is_empty() && flows.iter().all(|c| routed(c));
        let ok = if valid {
            Some(confirmed && s.mode == "tun" && (!ipv6 || s.dns.ipv6))
        } else {
            None
        };
        let detail = if valid && confirmed {
            format!(
                "Ответ {} через наблюдаемый TUN → ATLAS. Запрос без системного прокси.",
                response.unwrap().trim()
            )
        } else if valid {
            "Ответ получен; путь TUN → ATLAS не подтверждён.".into()
        } else if ipv6 && !s.dns.ipv6 {
            "Запрос IPv6 не прошёл. Для подтверждения отсутствия утечки нужен захват на физическом интерфейсе.".into()
        } else {
            "Запрос не завершился; туннель не подтверждён.".into()
        };
        results.push(check(name, ok, detail));
    }
    let mut port = 0;
    let (udp, entries) = observe(&client, || -> Result<(), ()> {
        let addr = ("stun.l.google.com", 19302)
            .to_socket_addrs()
            .map_err(|_| ())?
            .find(|a| a.is_ipv4())
            .ok_or(())?;
        let socket = UdpSocket::bind("0.0.0.0:0").map_err(|_| ())?;
        port = socket.local_addr().map_err(|_| ())?.port();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .map_err(|_| ())?;
        socket.connect(addr).map_err(|_| ())?;
        let mut q = vec![0, 1, 0, 0, 0x21, 0x12, 0xa4, 0x42];
        q.extend_from_slice(&uuid::Uuid::new_v4().as_bytes()[..12]);
        socket.send(&q).map_err(|_| ())?;
        let mut r = [0u8; 1024];
        let n = socket.recv(&mut r).map_err(|_| ())?;
        std::thread::sleep(Duration::from_millis(300));
        if n < 20 || r[..2] != [1, 1] || r[4..20] != q[4..20] {
            return Err(());
        }
        Ok(())
    });
    let flow = entries.iter().find(|c| {
        c["metadata"]["network"] == "udp"
            && c["metadata"]["sourcePort"]
                .as_str()
                .and_then(|p| p.parse::<u16>().ok())
                == Some(port)
    });
    results.push(check(
        "UDP через TUN",
        flow.map(|c| udp.is_ok() && routed(c)),
        if udp.is_ok() && flow.is_some_and(routed) {
            "Ответ STUN и совпадающий исходный порт подтверждают UDP через TUN → ATLAS."
        } else {
            "Нет ответа STUN или подтверждения маршрута его сокета через TUN → ATLAS."
        },
    ));
    let dns = (|| -> Result<(), ()> {
        let socket = UdpSocket::bind("0.0.0.0:0").map_err(|_| ())?;
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .map_err(|_| ())?;
        socket.connect("8.8.8.8:53").map_err(|_| ())?;
        let id = uuid::Uuid::new_v4();
        let mut q = vec![
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
            q.push(label.len() as u8);
            q.extend_from_slice(label.as_bytes());
        }
        q.extend_from_slice(&[0, 0, 1, 0, 1]);
        socket.send(&q).map_err(|_| ())?;
        let mut r = [0u8; 4096];
        let n = socket.recv(&mut r).map_err(|_| ())?;
        if n < 12 || r[..2] != q[..2] || r[2] & 0x80 == 0 {
            return Err(());
        }
        Ok(())
    })();
    results.push(check("DNS / утечки", if dns.is_err() { Some(false) } else { None }, if dns.is_ok() {
        "Новый DNS-запрос получил ответ. Отсутствие DNS на физическом интерфейсе требует отдельного захвата пакетов."
    } else { "Новый DNS-запрос не получил ответа." }));
    let observed = client
        .api("GET", "/connections", None)
        .ok()
        .and_then(|v| v["connections"].as_array().cloned())
        .unwrap_or_default();
    let telegram: Vec<_> = observed
        .iter()
        .filter(|c| {
            c["metadata"]["process"]
                .as_str()
                .is_some_and(|p| p.eq_ignore_ascii_case("telegram.exe"))
        })
        .collect();
    let ok = !telegram.is_empty()
        && telegram
            .iter()
            .all(|c| routed(c) && c["download"].as_u64().unwrap_or(0) > 0);
    results.push(check("Telegram Desktop", if telegram.is_empty() { None } else { Some(ok) }, if ok {
        "Есть полученные данные у наблюдаемых соединений Telegram через TUN → ATLAS."
    } else { "Трафик Telegram через TUN не подтверждён. Отключите встроенный прокси и создайте трафик перед проверкой." }));
    results
}
#[cfg(test)]
mod tests {
    use super::*;
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
    }
}

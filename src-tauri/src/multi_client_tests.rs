//! Real bundled cores, one shared VLESS server, loopback only. No TUN, WFP,
//! service installation, subscriptions, or OS proxy changes.
use super::*;
use crate::model::{Route, Subscription};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

fn isolated_core() -> Core {
    let dir = std::env::temp_dir().join(format!("atlas-multi-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut core = Core::new(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/mihomo.exe"),
        dir,
    );
    let ports: Vec<_> = (0..3)
        .map(|_| TcpListener::bind("127.0.0.1:0").unwrap())
        .collect();
    core.ports = std::array::from_fn(|i| ports[i].local_addr().unwrap().port());
    core
}

#[test]
fn direct_dns_survives_a_dead_vpn_with_real_core() {
    // Exercise the generated TUN DNS policy with TUN disabled in this fixture.
    // Both DNS and the HTTP origin are loopback-only; the VPN port is closed.
    let dns = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    dns.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
    let dns_address = dns.local_addr().unwrap();
    let dns_done = Arc::new(AtomicBool::new(false));
    let worker_done = dns_done.clone();
    let responder = thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        let deadline = Instant::now() + Duration::from_secs(15);
        while !worker_done.load(Ordering::SeqCst) && Instant::now() < deadline {
            let (size, peer) = match dns.recv_from(&mut buffer) {
                Ok(packet) => packet,
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
                Err(e) => panic!("fixture DNS receive: {e}"),
            };
            let mut end = 12;
            while buffer[end] != 0 { end += 1 + buffer[end] as usize; }
            end += 5;
            assert!(end <= size);
            assert_eq!(&buffer[end - 4..end - 2], &[0, 1]); // A question
            let mut answer = buffer[..end].to_vec();
            answer[2..4].copy_from_slice(&[0x81, 0x80]);
            answer[6..8].copy_from_slice(&[0, 1]);
            answer[8..12].fill(0);
            answer.extend_from_slice(&[0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 30, 0, 4, 127, 0, 0, 1]);
            dns.send_to(&answer, peer).unwrap();
            }
    });
    let origin = TcpListener::bind("127.0.0.1:0").unwrap();
    origin.set_nonblocking(true).unwrap();
    let origin_port = origin.local_addr().unwrap().port();
    let origin_worker = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok((mut stream, _)) = origin.accept() {
                stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut data = [0; 4096];
                let _ = stream.read(&mut data);
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").unwrap();
                return;
            }
            assert!(Instant::now() < deadline, "DIRECT never reached origin");
            thread::sleep(Duration::from_millis(10));
        }
    });
    let mut core = isolated_core();
    let mut settings = Settings::default();
    settings.selected = "dead-vpn".into();
    settings.default_route = Route::Direct;
    settings.dns.servers = vec!["1.1.1.1".into()];
    settings.subscriptions.push(Subscription {
        id: "fixture".into(), name: "fixture".into(), masked_url: "hidden".into(),
        updated_at: 0, error: None,
        servers: vec![json!({"name":"dead-vpn","type":"ss","server":"127.0.0.1",
            "port":1,"cipher":"aes-128-gcm","password":"fixture"})],
    });
    let mut doc: Value = serde_yaml::from_str(&config::generate(&settings, &core.secret).unwrap()).unwrap();
    doc["tun"]["enable"] = json!(false);
    doc["mixed-port"] = json!(core.ports[0]);
    doc["external-controller"] = json!(format!("127.0.0.1:{}", core.ports[1]));
    doc["dns"]["listen"] = json!(format!("127.0.0.1:{}", core.ports[2]));
    // Substitute only the resolver address, retaining the generated proxy suffix.
    for field in ["nameserver", "direct-nameserver"] {
        for value in doc["dns"][field].as_array_mut().unwrap() {
            *value = json!(value.as_str().unwrap().replace("1.1.1.1", &format!("udp://{dns_address}")));
        }
    }
    doc["dns"]["default-nameserver"] = json!(["127.0.0.1"]);
    doc["dns"]["proxy-server-nameserver"] = json!(["127.0.0.1"]);
    doc["proxy-groups"].as_array_mut().unwrap().retain(|group| group["name"] == "ATLAS");
    doc["proxy-groups"][0]["proxies"] = json!(["dead-vpn"]);
    let path = core.directory.join("dns-fixture.yaml");
    std::fs::write(&path, serde_yaml::to_string(&doc).unwrap()).unwrap();
    let child = core.command().arg("-d").arg(&core.directory).arg("-f").arg(path).spawn().unwrap();
    core.job = Some(crate::job::Job::attach(&child).unwrap());
    core.child = Some(child);
    let deadline = Instant::now() + Duration::from_secs(5);
    while core.api("GET", "/version", None).is_err() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(25));
    }
    let http = reqwest::blocking::Client::builder().no_proxy()
        .proxy(reqwest::Proxy::http(format!("http://127.0.0.1:{}", core.ports[0])).unwrap())
        .timeout(Duration::from_secs(5)).build().unwrap();
    let response = http.get(format!("http://direct-fixture.invalid:{origin_port}/")).send().unwrap();
    assert_eq!(response.status(), 200, "DIRECT must not use the failed VPN for DNS");
    assert_eq!(response.text().unwrap(), "ok");
    dns_done.store(true, Ordering::SeqCst);
    responder.join().unwrap();
    origin_worker.join().unwrap();
    core.stop().unwrap();
    let directory = core.directory.clone();
    drop(core);
    std::fs::remove_dir_all(directory).unwrap();
}

fn start_server(core: &mut Core, port: u16, id: &str) {
    let path = core.directory.join("server.yaml");
    let config = json!({
        "external-controller":format!("127.0.0.1:{}",core.ports[1]), "secret":core.secret,
        "log-level":"debug", "allow-lan":false, "mode":"rule", "rules":["MATCH,DIRECT"],
        "listeners":[{"name":"shared-vless", "type":"vless", "listen":"127.0.0.1", "port":port,
            "allow-insecure":true,
            "users":[{"username":"fixture", "uuid":id}]}]
    });
    std::fs::write(&path, serde_yaml::to_string(&config).unwrap()).unwrap();
    let log = std::fs::File::create(core.directory.join("server.log")).unwrap();
    let child = core
        .command()
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .arg("-d")
        .arg(&core.directory)
        .arg("-f")
        .arg(path)
        .spawn()
        .unwrap();
    core.job = Some(crate::job::Job::attach(&child).unwrap());
    core.child = Some(child);
    let deadline = Instant::now() + Duration::from_secs(5);
    while core.api("GET", "/version", None).is_err()
        || std::net::TcpStream::connect(("127.0.0.1", port)).is_err()
    {
        assert!(Instant::now() < deadline, "fixture server readiness");
        thread::sleep(Duration::from_millis(50));
    }
}

fn request(proxy_port: u16, origin_port: u16) -> std::io::Result<String> {
    let mut socket = std::net::TcpStream::connect(("127.0.0.1", proxy_port))?;
    socket.set_read_timeout(Some(Duration::from_secs(3)))?;
    socket.set_write_timeout(Some(Duration::from_secs(3)))?;
    socket.write_all(&[5, 1, 0])?;
    let mut greeting = [0; 2];
    socket.read_exact(&mut greeting)?;
    assert_eq!(greeting, [5, 0]);
    let mut connect = vec![5, 1, 0, 1, 127, 0, 0, 1];
    connect.extend(origin_port.to_be_bytes());
    socket.write_all(&connect)?;
    let mut response = [0; 10];
    socket.read_exact(&mut response)?;
    if response[1] != 0 {
        return Err(std::io::Error::other("SOCKS connection rejected"));
    }
    assert_eq!(response[3], 1);
    socket.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")?;
    let mut body = String::new();
    socket.read_to_string(&mut body)?;
    if !body.starts_with("HTTP/1.1 200") {
        return Err(std::io::Error::other(format!("origin did not answer: {body:?}")));
    }
    Ok(body.split_once("\r\n\r\n").unwrap().1.to_owned())
}

fn http_proxy_request(proxy_port: u16, origin_port: u16) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .proxy(
            reqwest::Proxy::http(format!("http://127.0.0.1:{proxy_port}"))
                .map_err(|e| e.to_string())?,
        )
        .timeout(Duration::from_secs(3))
        .build()
        .map_err(|e| e.to_string())?;
    client
        .get(format!("http://127.0.0.1:{origin_port}/"))
        .send()
        .and_then(|response| response.error_for_status())
        .and_then(|response| response.text())
        .map_err(|e| e.to_string())
}

#[test]
fn independent_clients_share_vless_server_and_recover_after_its_restart() {
    let origin = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin_port = origin.local_addr().unwrap().port();
    origin.set_nonblocking(true).unwrap();
    let done = Arc::new(AtomicBool::new(false));
    let worker_done = done.clone();
    let worker = thread::spawn(move || {
        let mut connections = Vec::new();
        while !worker_done.load(Ordering::SeqCst) {
            if let Ok((mut stream, _)) = origin.accept() {
                // One slow peer's half-close must not hold up the other clients.
                // This origin receives concurrent traffic through eight proxies.
                connections.push(thread::spawn(move || {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(1)))
                        .unwrap();
                    // VLESS may split HTTP headers across TCP reads. Closing with
                    // unread request bytes resets the connection and fabricates 502s.
                    let mut request = Vec::new();
                    let mut buffer = [0; 1024];
                    while request.len() < 8192 && !request.windows(4).any(|v| v == b"\r\n\r\n") {
                        match stream.read(&mut buffer) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => request.extend_from_slice(&buffer[..n]),
                        }
                    }
                    let _ = stream.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    );
                    let _ = stream.shutdown(std::net::Shutdown::Write);
                    // Drain the peer's close instead of aborting a socket with
                    // pending input; Windows otherwise can turn fixture teardown into RST.
                    while stream.read(&mut buffer).is_ok_and(|n| n > 0) {}
                }));
            }
            thread::sleep(Duration::from_millis(2));
        }
        for connection in connections {
            connection.join().unwrap();
        }
    });
    let reservation = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = reservation.local_addr().unwrap().port();
    let id = uuid::Uuid::new_v4().to_string();
    let mut server = isolated_core();
    drop(reservation);
    start_server(&mut server, port, &id);
    let mut clients = Vec::new();
    let mut settings = Settings::default();
    settings.mode = "system".into();
    settings.default_route = Route::Proxy;
    settings.selected = "shared".into();
    settings.subscriptions.push(Subscription {
        id:"fixture".into(), name:"fixture".into(), masked_url:"hidden".into(), updated_at:0, error:None,
        servers:vec![json!({"name":"shared", "type":"vless", "server":"127.0.0.1", "port":port, "uuid":id, "tls":false})],
    });
    for _ in 0..8 {
        let mut c = isolated_core();
        c.start(&settings).unwrap();
        clients.push(c);
    }
    let ports: Vec<_> = clients.iter().map(|c| c.ports[0]).collect();
    let check_all = |phase: &str| {
        thread::scope(|scope| {
            let workers: Vec<_> = ports
                .iter()
                .enumerate()
                .map(|(index, port)| {
                    let core = &clients[index];
                    scope.spawn(move || {
                        let response = request(*port, origin_port);
                        match response {
                            Ok(text) => {
                                assert_eq!(text, "ok", "{phase}: {:?}", core.client().logs());
                            }
                            Err(e) => panic!("{phase}: {e:?}; {:?}", core.client().logs()),
                        }
                        assert_eq!(
                            http_proxy_request(*port, origin_port).unwrap_or_else(|e| panic!(
                                "{phase} HTTP proxy: {e}; {:?}",
                                core.client().logs()
                            )),
                            "ok"
                        );
                    })
                })
                .collect();
            for worker in workers {
                worker.join().unwrap();
            }
        })
    };
    for _ in 0..5 {
        check_all("before restart");
    }
    server.stop().unwrap();
    thread::scope(|scope| {
        for port in &ports {
            scope.spawn(move || {
                assert!(request(*port, origin_port).is_err());
                assert!(http_proxy_request(*port, origin_port).is_err());
            });
        }
    });
    // Keep all client processes and their selectors; only the shared server restarts.
    start_server(&mut server, port, &id);
    for _ in 0..5 {
        check_all("after restart");
    }
    clients[0].stop().unwrap();
    for (index, port) in ports.iter().enumerate().skip(1) {
        assert_eq!(
            request(*port, origin_port).unwrap(),
            "ok",
            "remaining client {index}: {:?}",
            clients[index].client().logs()
        );
    }
    done.store(true, Ordering::SeqCst);
    worker.join().unwrap();
    clients.push(server);
    for mut core in clients {
        core.stop().unwrap();
        let dir = core.directory.clone();
        drop(core);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

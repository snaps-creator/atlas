use crate::{config, model::Settings};
use serde_json::{json, Value};
use std::os::windows::process::CommandExt;
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
#[cfg(test)]
#[path = "multi_client_tests.rs"]
mod multi_client_tests;
pub struct Core {
    pub binary: PathBuf,
    pub directory: PathBuf,
    pub child: Option<Child>,
    job: Option<crate::job::Job>,
    broker: Option<std::sync::Arc<crate::broker::Broker>>,
    elevated: bool,
    secret: String,
    ports: [u16; 3],
    pub started: Option<Instant>,
    logs: std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<String>>>,
}
impl Core {
    pub fn new(binary: PathBuf, directory: PathBuf) -> Self {
        Self {
            binary,
            directory,
            child: None,
            job: None,
            broker: None,
            elevated: false,
            secret: format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            ),
            started: None,
            ports: [17890, 19090, 11053],
            logs: Default::default(),
        }
    }
    pub(crate) fn privileged(binary: PathBuf, directory: PathBuf) -> Self {
        let mut c = Self::new(binary, directory);
        c.elevated = true;
        c
    }
    fn command(&self) -> Command {
        let mut c = Command::new(&self.binary);
        c.creation_flags(0x08000000)
            .current_dir(&self.directory)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        c
    }
    pub fn validate(&self, s: &Settings) -> Result<PathBuf, String> {
        let yaml = config::generate(s, &self.secret)?;
        #[cfg(test)]
        let yaml = {
            let mut doc: Value = serde_yaml::from_str(&yaml).map_err(|e| e.to_string())?;
            doc["mixed-port"] = json!(self.ports[0]);
            doc["external-controller"] = json!(format!("127.0.0.1:{}", self.ports[1]));
            doc["dns"]["listen"] = json!(format!("127.0.0.1:{}", self.ports[2]));
            serde_yaml::to_string(&doc).map_err(|e| e.to_string())?
        };
        let path = self.directory.join("candidate.yaml");
        std::fs::write(&path, yaml).map_err(|e| e.to_string())?;
        let mut c = self
            .command()
            .arg("-t")
            .arg("-d")
            .arg(&self.directory)
            .arg("-f")
            .arg(&path)
            .spawn()
            .map_err(|_| "Не удалось запустить Mihomo; проверьте наличие ядра")?;
        let start = Instant::now();
        loop {
            if let Some(status) = c.try_wait().map_err(|e| e.to_string())? {
                if status.success() {
                    return Ok(path);
                }
                return Err("Mihomo отклонил конфигурацию. Проверьте параметры серверов и DNS; предыдущая версия сохранена.".into());
            }
            if start.elapsed() > Duration::from_secs(20) {
                let _ = c.kill();
                let _ = c.wait();
                return Err("Проверка конфигурации Mihomo превысила 20 секунд".into());
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
    pub fn client(&self) -> ApiClient {
        ApiClient {
            secret: self.secret.clone(),
            controller_port: self.ports[1],
            broker: self.broker.clone(),
            logs: self.logs.clone(),
        }
    }
    pub fn api(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
        self.client().api(method, path, body)
    }
    pub fn guard_active(&self) -> bool {
        self.started.is_some() && self.broker.as_ref().is_some_and(|broker| broker.alive())
    }
    pub fn running(&mut self) -> bool {
        if let Some(broker) = &self.broker {
            if !broker.alive() {
                let _ = std::fs::remove_file(self.directory.join("tun-guard.active"));
                return false;
            }
            // The service closes this session when its core dies. Status polling
            // must not queue behind URL latency tests on the command pipe.
            return true;
        }
        if let Some(child) = self.child.as_mut() {
            matches!(child.try_wait(), Ok(None))
        } else {
            false
        }
    }
    pub fn start(&mut self, s: &Settings) -> Result<(), String> {
        if self.running() {
            return Ok(());
        }
        if s.mode == "tun" && !self.elevated {
            crate::broker::validate_settings(s)?;
            self.validate(s)?;
            let broker = std::sync::Arc::new(crate::broker::Broker::launch()?);
            std::fs::write(
                self.directory.join("tun-guard.active"),
                b"Atlas persistent network guard",
            )
            .map_err(|e| e.to_string())?;
            self.broker = Some(broker.clone());
            if let Err(error) =
                broker.call("start", serde_json::to_value(s).map_err(|e| e.to_string())?)
            {
                let _ = self.stop();
                return Err(error);
            }
            self.started = Some(Instant::now());
            return Ok(());
        }
        for port in self.ports {
            std::net::TcpListener::bind(("127.0.0.1", port))
                .map_err(|_| format!("Порт {port} занят другим приложением"))?;
        }
        let path = self.validate(s)?;
        self.child = Some(
            self.command()
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .arg("-d")
                .arg(&self.directory)
                .arg("-f")
                .arg(&path)
                .spawn()
                .map_err(|_| "Не удалось запустить Mihomo")?,
        );
        self.logs.lock().map_err(|_| "Журнал недоступен")?.clear();
        let child = self.child.as_mut().unwrap();
        let mut outputs: Vec<Box<dyn std::io::Read + Send>> = Vec::new();
        if let Some(output) = child.stdout.take() {
            outputs.push(Box::new(output));
        }
        if let Some(output) = child.stderr.take() {
            outputs.push(Box::new(output));
        }
        for output in outputs {
            let logs = self.logs.clone();
            std::thread::spawn(move || {
                use std::io::BufRead;
                for line in std::io::BufReader::new(output)
                    .lines()
                    .map_while(Result::ok)
                {
                    if let Ok(mut buffer) = logs.lock() {
                        buffer.push_back(line.chars().take(4096).collect());
                        while buffer.len() > 256 {
                            buffer.pop_front();
                        }
                    }
                }
            });
        }
        match crate::job::Job::attach(self.child.as_ref().unwrap()) {
            Ok(job) => self.job = Some(job),
            Err(e) => {
                self.stop()?;
                return Err(e);
            }
        }
        // Mihomo exposes its controller before creating the TUN adapter.
        // A false enable flag during that interval is pending, not a failure.
        let deadline = Instant::now() + Duration::from_secs(if s.mode == "tun" { 60 } else { 5 });
        while Instant::now() < deadline {
            if s.mode == "tun"
                && self.logs.lock().is_ok_and(|logs| {
                    logs.iter()
                        .any(|line| line.to_lowercase().contains("start tun listening error"))
                })
            {
                break;
            }
            if !self.running() {
                break;
            }
            if self.api("GET", "/version", None).is_ok() {
                // The controller can answer before inbound listeners finish
                // starting. Do not publish Connected during that interval.
                if std::net::TcpStream::connect_timeout(
                    &std::net::SocketAddr::from(([127, 0, 0, 1], self.ports[0])),
                    Duration::from_millis(100),
                )
                .is_err()
                {
                    thread::sleep(Duration::from_millis(50));
                    continue;
                }
                if s.mode == "tun" {
                    let ready = self
                        .api("GET", "/configs", None)
                        .is_ok_and(|config| config["tun"]["enable"] == true);
                    if !ready {
                        thread::sleep(Duration::from_millis(100));
                        continue;
                    }
                }
                self.api("PUT", "/proxies/ATLAS", Some(json!({"name":s.selected})))?;
                self.commit(&path)?;
                self.started = Some(Instant::now());
                return Ok(());
            }
            thread::sleep(Duration::from_millis(100));
        }
        let error = if s.mode == "tun" {
            self.tun_error()
        } else {
            "Mihomo не прошёл проверку запуска".into()
        };
        self.stop()?;
        Err(error)
    }
    fn commit(&self, path: &std::path::Path) -> Result<(), String> {
        let current = self.directory.join("current.yaml");
        let last = self.directory.join("last-working.yaml");
        if current.exists() {
            std::fs::copy(&current, self.directory.join("previous.yaml"))
                .map_err(|e| e.to_string())?;
        }
        std::fs::copy(path, &current).map_err(|e| e.to_string())?;
        std::fs::copy(path, &last).map_err(|e| e.to_string())?;
        Ok(())
    }
    fn tun_error(&self) -> String {
        let detail = self.logs.lock().ok().and_then(|logs| {
            logs.iter()
                .rev()
                .find(|line| {
                    let lower = line.to_lowercase();
                    lower.contains("tun") && (lower.contains("error") || lower.contains("fatal"))
                })
                .cloned()
        });
        detail
            .map(|line| format!("Не удалось создать TUN: {line}"))
            .unwrap_or_else(|| {
                "Не дождались готовности TUN: ядро завершилось или истекло время ожидания. Подключение отменено.".into()
            })
    }
    pub fn apply(&mut self, s: &Settings) -> Result<(), String> {
        if let Some(broker) = &self.broker {
            crate::broker::validate_settings(s)?;
            broker.call("apply", serde_json::to_value(s).map_err(|e| e.to_string())?)?;
            return Ok(());
        }
        let path = self.validate(s)?;
        if !self.running() {
            return Ok(());
        }
        let old = self.directory.join("last-working.yaml");
        let result = self
            .api("PUT", "/configs?force=true", Some(json!({"path":path})))
            .and_then(|_| self.api("PUT", "/proxies/ATLAS", Some(json!({"name":s.selected}))))
            .and_then(|_| self.api("GET", "/configs", None))
            .and_then(|config| {
                if s.mode == "tun" && config["tun"]["enable"] != true {
                    Err(self.tun_error())
                } else {
                    Ok(config)
                }
            });
        if let Err(e) = result {
            if old.exists() {
                self.api("PUT", "/configs?force=true", Some(json!({"path":old})))?;
            }
            return Err(e);
        }
        self.commit(&path)
    }
    pub fn select(&self, name: &str) -> Result<(), String> {
        if let Some(broker) = &self.broker {
            broker.call("select", json!({"name":name}))?;
        } else {
            self.api("PUT", "/proxies/ATLAS", Some(json!({"name":name})))?;
        }
        Ok(())
    }
    pub fn stop(&mut self) -> Result<(), String> {
        if self.broker.as_ref().is_some_and(|b| !b.alive()) {
            self.broker = None;
        }
        let mut result = Ok(());
        if let Some(broker) = self.broker.take() {
            result = broker.call("stop", Value::Null).map(|_| ());
            let _ = std::fs::remove_file(self.directory.join("tun-guard.active"));
        }
        if self.elevated && self.child.is_some() {
            // Run sing-tun's real Close path (adapter, routes, DNS cache) before
            // TerminateProcess. The local API has a 3-second deadline; a hung
            // core still falls through to owned-process termination.
            let _ = self.api("PATCH", "/configs", Some(json!({"tun":{"enable":false}})));
        }
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        self.started = None;
        self.job = None;
        let _ = std::fs::remove_file(self.directory.join("tun-guard.active"));
        result
    }
}
impl Drop for Core {
    fn drop(&mut self) {
        // Closing the IPC session releases the broker's dynamic WFP filters.
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.broker = None;
        self.job = None;
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::model::{Route, Rule, RuleGroup, Subscription};
    #[test]
    fn privileged_stop_closes_tun_before_terminating_owned_process() {
        use std::io::{Read, Write};
        use std::os::windows::io::AsRawHandle;
        let api = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut core = Core::privileged(
            PathBuf::from("unused.exe"),
            std::env::temp_dir().join(format!("atlas-stop-order-{}", uuid::Uuid::new_v4())),
        );
        core.ports[1] = api.local_addr().unwrap().port();
        let child = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Start-Sleep -Seconds 60",
            ])
            .creation_flags(0x08000000)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        core.job = Some(crate::job::Job::attach(&child).unwrap());
        let handle = child.as_raw_handle() as usize;
        core.child = Some(child);
        let worker = thread::spawn(move || {
            let (mut socket, _) = api.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            let mut part = [0; 1024];
            while !request.ends_with(b"}}") {
                let n = socket.read(&mut part).unwrap();
                assert!(n > 0);
                request.extend_from_slice(&part[..n]);
                assert!(request.len() < 8192);
            }
            assert!(request.starts_with(b"PATCH /configs "));
            assert!(String::from_utf8_lossy(&request).contains("\"tun\":{\"enable\":false}"));
            assert_eq!(
                unsafe {
                    windows_sys::Win32::System::Threading::WaitForSingleObject(handle as _, 0)
                },
                windows_sys::Win32::Foundation::WAIT_TIMEOUT
            );
            socket
                .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
                .unwrap();
        });
        core.stop().unwrap();
        worker.join().unwrap();
        assert!(core.child.is_none());
        assert!(!core.running());
    }
    #[test]
    fn disconnected_stop_does_not_launch_a_broker_for_a_stale_marker() {
        let directory =
            std::env::temp_dir().join(format!("atlas-stop-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let marker = directory.join("tun-guard.active");
        std::fs::write(&marker, b"stale").unwrap();
        // A deliberately nonexistent executable proves stop cannot launch a core/helper.
        let mut core = Core::new(directory.join("missing.exe"), directory.clone());
        assert!(!core.guard_active()); // An on-disk marker does not prove live WFP filters.
        core.stop().unwrap();
        core.stop().unwrap();
        assert!(!marker.exists());
        assert!(core.broker.is_none());
        assert!(!core.running());
        drop(core);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn real_core_accepts_empty_full_tunnel_and_route_exceptions() {
        let directory =
            std::env::temp_dir().join(format!("atlas-config-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let core = Core::new(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/mihomo.exe"),
            directory.clone(),
        );
        let mut s = Settings::default();
        s.default_route = Route::Proxy;
        s.subscriptions.push(Subscription {id:"fixture".into(),name:"fixture".into(),masked_url:"hidden".into(),updated_at:0,error:None,servers:vec![json!({"name":"fixture","type":"ss","server":"127.0.0.1","port":1,"cipher":"aes-128-gcm","password":"fixture-only"})]});
        for text in ["version: 1\ndefault-route: proxy\nrules: []", "version: 1\ndefault-route: proxy\nrules:\n - {domain-suffix: example.com, route: direct}\n - {ip-cidr: 192.0.2.0/24, route: block, no-resolve: true}"] {
            let import = crate::portable::parse(text).unwrap(); s.groups = import.groups;
            for ipv6 in [false, true] { s.dns.ipv6 = ipv6; core.validate(&s).expect("Mihomo TUN configuration must be valid"); }
        }
        drop(core);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn real_mihomo_validation_reload_selector_and_rollback() {
        let directory =
            std::env::temp_dir().join(format!("atlas-core-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let binary = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/mihomo.exe");
        let mut core = Core::new(binary, directory.clone());
        let reservations: Vec<_> = (0..3)
            .map(|_| std::net::TcpListener::bind("127.0.0.1:0").unwrap())
            .collect();
        core.ports = std::array::from_fn(|i| reservations[i].local_addr().unwrap().port());
        let mut settings = Settings::default();
        settings.mode = "system".into();
        // Isolated fixture. No real subscription, credentials or OS proxy changes.
        settings.subscriptions.push(Subscription {id:"fixture".into(),name:"fixture".into(),masked_url:"hidden".into(),updated_at:0,error:None,servers:vec![json!({"name":"fixture","type":"ss","server":"127.0.0.1","port":1,"cipher":"aes-128-gcm","password":"fixture-only"})]});
        let mut tun_candidate = settings.clone();
        tun_candidate.mode = "tun".into();
        core.validate(&tun_candidate)
            .expect("Mihomo must accept dual-stack TUN configuration");
        drop(reservations);
        core.start(&settings).unwrap();
        assert!(core.running());
        assert!(core.api("GET", "/version", None).unwrap()["version"].is_string());
        let process = core.child.as_ref().unwrap().id();
        let original_config = std::fs::read(directory.join("last-working.yaml")).unwrap();
        for name in ["fixture", "AUTO", "FAILOVER", "fixture", "AUTO"] {
            core.select(name).unwrap();
            assert_eq!(
                core.api("GET", "/proxies/ATLAS", None).unwrap()["now"],
                name
            );
            assert_eq!(core.child.as_ref().unwrap().id(), process);
            assert_eq!(
                std::fs::read(directory.join("last-working.yaml")).unwrap(),
                original_config
            );
        }
        assert!(core.select("missing-fixture").is_err());
        assert_eq!(
            core.api("GET", "/proxies/ATLAS", None).unwrap()["now"],
            "AUTO"
        );
        settings.selected = "FAILOVER".into();
        core.apply(&settings).unwrap();
        assert_eq!(
            core.api("GET", "/proxies/ATLAS", None).unwrap()["now"],
            "FAILOVER"
        );
        let working = std::fs::read(directory.join("last-working.yaml")).unwrap();
        let mut invalid = settings.clone();
        invalid.subscriptions[0].servers[0]["type"] = json!("invalid-proxy-protocol");
        assert!(core.apply(&invalid).is_err());
        assert_eq!(
            std::fs::read(directory.join("last-working.yaml")).unwrap(),
            working
        );
        assert!(core.api("GET", "/version", None).is_ok());
        use std::io::{Read, Write};
        use std::sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Arc,
        };
        let origin = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let origin_url = format!("http://{}", origin.local_addr().unwrap());
        origin.set_nonblocking(true).unwrap();
        let alive = Arc::new(AtomicBool::new(true));
        let hits = Arc::new(AtomicUsize::new(0));
        let worker_alive = alive.clone();
        let worker_hits = hits.clone();
        let worker = thread::spawn(move || {
            while worker_alive.load(Ordering::SeqCst) {
                if let Ok((mut stream, _)) = origin.accept() {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(1)))
                        .unwrap();
                    let mut buffer = [0; 4096];
                    let _ = stream.read(&mut buffer);
                    worker_hits.fetch_add(1, Ordering::SeqCst);
                    let _ = stream.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    );
                }
                thread::sleep(Duration::from_millis(10));
            }
        });
        let client = reqwest::blocking::Client::builder()
            .proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{}", core.ports[0])).unwrap())
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap();
        assert!(client
            .get(&origin_url)
            .send()
            .unwrap()
            .status()
            .is_success());
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        // UDP regression through the real Mihomo SOCKS5 relay, without OS routing changes.
        let echo = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        echo.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let echo_port = echo.local_addr().unwrap().port();
        let echo_worker = thread::spawn(move || {
            let mut b = [0; 512];
            let (n, peer) = echo.recv_from(&mut b).unwrap();
            echo.send_to(&b[..n], peer).unwrap();
        });
        let mut control = std::net::TcpStream::connect(("127.0.0.1", core.ports[0])).unwrap();
        control
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        control.write_all(&[5, 1, 0]).unwrap();
        let mut greeting = [0; 2];
        control.read_exact(&mut greeting).unwrap();
        assert_eq!(greeting, [5, 0]);
        control.write_all(&[5, 3, 0, 1, 0, 0, 0, 0, 0, 0]).unwrap();
        let mut reply = [0; 10];
        control.read_exact(&mut reply).unwrap();
        assert_eq!(reply[1], 0);
        assert_eq!(reply[3], 1);
        let relay = std::net::SocketAddrV4::new(
            std::net::Ipv4Addr::LOCALHOST,
            u16::from_be_bytes([reply[8], reply[9]]),
        );
        let udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        udp.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let mut packet = vec![0, 0, 0, 1, 127, 0, 0, 1];
        packet.extend(echo_port.to_be_bytes());
        packet.extend(b"atlas-udp-regression");
        udp.send_to(&packet, relay).unwrap();
        let mut answer = [0; 512];
        let (n, _) = udp.recv_from(&mut answer).unwrap();
        assert_eq!(&answer[10..n], b"atlas-udp-regression");
        echo_worker.join().unwrap();
        settings.groups.push(RuleGroup {
            id: "block".into(),
            name: "Block test".into(),
            description: String::new(),
            enabled: true,
            route: Route::Block,
            rules: vec![Rule {
                kind: "IP-CIDR".into(),
                value: "127.0.0.1/32".into(),
                no_resolve: true,
            }],
        });
        core.apply(&settings).unwrap();
        let response = client.get(&origin_url).send();
        // The origin remains healthy; a blocked request must never reach it.
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        alive.store(false, Ordering::SeqCst);
        worker.join().unwrap();
        assert!(response.is_err() || !response.unwrap().status().is_success());
        // Kill only this isolated, non-TUN test child to exercise restart readiness.
        core.child.as_mut().unwrap().kill().unwrap();
        core.child.as_mut().unwrap().wait().unwrap();
        assert!(!core.running());
        core.start(&settings).unwrap();
        assert!(core.running());
        assert!(core.api("GET", "/version", None).is_ok());
        core.stop().unwrap();
        assert!(!core.running());
        drop(core);
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[derive(Clone)]
pub struct ApiClient {
    controller_port: u16,
    secret: String,
    broker: Option<std::sync::Arc<crate::broker::Broker>>,
    logs: std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<String>>>,
}
impl ApiClient {
    pub fn logs(&self) -> Result<Vec<String>, String> {
        if let Some(b) = &self.broker {
            return serde_json::from_value(b.call("logs", Value::Null)?)
                .map_err(|_| "Журнал недоступен".into());
        }
        Ok(self
            .logs
            .lock()
            .map_err(|_| "Журнал недоступен")?
            .iter()
            .cloned()
            .collect())
    }
    pub fn api(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
        if let Some(broker) = &self.broker {
            if method == "GET" && path.contains("/delay?") {
                let job = broker.call("delay", json!({"path":path}))?;
                let deadline = Instant::now() + Duration::from_secs(15);
                loop {
                    if Instant::now() >= deadline {
                        return Err("Проверка сервера превысила время ожидания".into());
                    }
                    let result = broker.call("delay_result", json!({"id":job["id"]}))?;
                    if result["done"] == true {
                        return Ok(result["value"].clone());
                    }
                    thread::sleep(Duration::from_millis(150));
                }
            }
            return broker.call("api", json!({"method":method,"path":path}));
        }
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(1))
            .timeout(Duration::from_secs(if path.contains("/delay?") {
                12
            } else {
                3
            }))
            .build()
            .map_err(|e| e.to_string())?;
        let method =
            reqwest::Method::from_bytes(method.as_bytes()).map_err(|_| "Некорректный метод")?;
        let mut req = client
            .request(
                method,
                format!("http://127.0.0.1:{}{path}", self.controller_port),
            )
            .bearer_auth(&self.secret);
        if let Some(b) = body {
            req = req.json(&b)
        }
        let response = req.send().map_err(|_| "Mihomo API недоступен")?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("Mihomo API: HTTP {}", status.as_u16()));
        }
        let bytes = response.bytes().map_err(|_| "Ошибка чтения API")?;
        if bytes.is_empty() {
            Ok(json!({}))
        } else {
            serde_json::from_slice(&bytes).map_err(|_| "Некорректный ответ Mihomo API".into())
        }
    }
}

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
#[path = "selector_state.rs"]
mod selector_state;
pub struct Core {
    pub continue_running: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    pub binary: PathBuf,
    pub directory: PathBuf,
    pub child: Option<Child>,
    job: Option<crate::job::Job>,
    xray: Option<crate::xray_runtime::Runtime>,
    draining_xray: Vec<crate::xray_runtime::Runtime>,
    last_drain_check: Option<Instant>,
    broker: Option<std::sync::Arc<crate::broker::Broker>>,
    elevated: bool,
    secret: String,
    ports: [u16; 3],
    pub started: Option<Instant>,
    logs: std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<String>>>,
}
impl Core {
    fn restore_nested_selectors(&self, before: &Value) -> Result<Value, String> {
        let after = self.api("GET", "/proxies", None)?;
        for (group, selected) in selector_state::restore_plan(before, &after) {
            let path = format!("/proxies/{group}");
            self.api("PUT", &path, Some(json!({"name":selected})))?;
            if self.api("GET", &path, None)?["now"] != selected {
                return Err(format!("Ядро не восстановило выбранный узел группы {group}"));
            }
        }
        Ok(Value::Null)
    }
    pub(crate) fn owned_processes_released(&self) -> bool {
        self.child.is_none() && self.xray.is_none() && self.draining_xray.is_empty() && self.broker.is_none()
    }
    pub(crate) fn has_broker(&self) -> bool {
        self.broker.is_some()
    }
    pub fn new(binary: PathBuf, directory: PathBuf) -> Self {
        Self {
            continue_running: None,
            binary,
            directory,
            child: None,
            job: None,
            xray: None,
            draining_xray: Vec::new(),
            last_drain_check: None,
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
    /// A probe-only core has private loopback listeners and never owns TUN,
    /// system proxy settings, or the service's fixed controller ports.
    pub(crate) fn use_ephemeral_ports(&mut self) -> Result<(), String> {
        // Windows can reserve different port ranges for TCP and UDP. Mixed
        // and DNS listeners need both, so TCP availability alone is insufficient.
        let mut reservations = Vec::new();
        let mut ports = [0; 3];
        for port in &mut ports {
            let mut last_error = String::new();
            for _ in 0..128 {
                let udp = std::net::UdpSocket::bind(("127.0.0.1", 0))
                    .map_err(|e| format!("Не удалось выделить UDP-порт проверки: {e}"))?;
                let candidate = udp.local_addr().map_err(|e| e.to_string())?.port();
                match std::net::TcpListener::bind(("127.0.0.1", candidate)) {
                    Ok(tcp) => {
                        *port = candidate;
                        // Keep all pairs reserved until the entire set is allocated.
                        reservations.push((tcp, udp));
                        break;
                    }
                    Err(error) => last_error = error.to_string(),
                }
            }
            if *port == 0 {
                return Err(format!("Не удалось выделить общий TCP/UDP-порт проверки: {last_error}"));
            }
        }
        self.ports = ports;
        Ok(())
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
        let prepared=crate::xray_runtime::Prepared::new(s,&self.binary,&self.directory)?.cancellable(self.continue_running.clone());
        prepared.validate()?;
        self.validate_mapped(&prepared.settings)
    }
    fn validate_mapped(&self, s: &Settings) -> Result<PathBuf, String> {
        let yaml = config::generate(s, &self.secret)?;
        let yaml=if s.servers().iter().any(|n|n["atlas-xray-bridge"]==true) {
            let mut doc: Value=serde_yaml::from_str(&yaml).map_err(|e|e.to_string())?;
            let path=self.binary.with_file_name("Atlas.Xray.exe");
            let path=path.to_str().ok_or("Некорректный путь Xray")?;
            if path.contains([',','\n','\r']) {return Err("Путь установки содержит неподдерживаемые символы для правила Xray".into());}
            doc["rules"].as_array_mut().ok_or("Отсутствуют правила маршрутизации")?
                .insert(0,json!(format!("PROCESS-PATH,{path},DIRECT")));
            serde_yaml::to_string(&doc).map_err(|e|e.to_string())?
        } else {yaml};
        let yaml = if self.ports != [17890, 19090, 11053] {
            let mut doc: Value = serde_yaml::from_str(&yaml).map_err(|e| e.to_string())?;
            doc["mixed-port"] = json!(self.ports[0]);
            doc["external-controller"] = json!(format!("127.0.0.1:{}", self.ports[1]));
            doc["dns"]["listen"] = json!(format!("127.0.0.1:{}", self.ports[2]));
            serde_yaml::to_string(&doc).map_err(|e| e.to_string())?
        } else { yaml };
        #[cfg(test)]
        let yaml = {
            let mut doc: Value = serde_yaml::from_str(&yaml).map_err(|e| e.to_string())?;
            doc["mixed-port"] = json!(self.ports[0]);
            doc["external-controller"] = json!(format!("127.0.0.1:{}", self.ports[1]));
            doc["dns"]["listen"] = json!(format!("127.0.0.1:{}", self.ports[2]));
            // Isolated core tests must never perform the generated public URL tests.
            for group in doc["proxy-groups"].as_array_mut().unwrap() {
                group["url"] = json!("http://127.0.0.1:1/fixture-health");
            }
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
        let job = match crate::job::Job::attach(&c) {
            Ok(job) => job,
            Err(error) => {
                let _ = crate::process_stop::stop(&mut c, || {}, Duration::from_secs(1));
                return Err(error);
            }
        };
        let start = Instant::now();
        loop {
            if self.cancelled() {
                crate::process_stop::stop(&mut c, || drop(job), Duration::from_secs(1))?;
                return Err("Подключение отменено".into());
            }
            if let Some(status) = c.try_wait().map_err(|e| e.to_string())? {
                if status.success() {
                    return Ok(path);
                }
                return Err("Mihomo отклонил конфигурацию. Проверьте параметры серверов и DNS; предыдущая версия сохранена.".into());
            }
            if start.elapsed() > Duration::from_secs(20) {
                crate::process_stop::stop(&mut c, || drop(job), Duration::from_secs(1))?;
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
    pub fn service_reachable(&self) -> bool {
        self.broker.as_ref().is_some_and(|broker| broker.alive())
    }
    pub fn service_status(&self) -> Option<Value> {
        self.broker.as_ref()?.call("status", Value::Null).ok()
    }
    pub fn running(&mut self) -> bool {
        self.reap_draining_xray();
        if self.xray.as_mut().is_some_and(|runtime|!runtime.healthy()) {return false;}
        if let Some(broker) = &self.broker {
            if !broker.alive() {
                if broker.reconnect().is_err() { return false; }
            }
            // The service survives the UI and may hold WFP in protected pause.
            // A live pipe alone is not evidence of a working VPN path.
            return broker.call("status", Value::Null).is_ok_and(|state|
                state["running"] == true && state["guard"] == true);
        }
        if let Some(child) = self.child.as_mut() {
            matches!(child.try_wait(), Ok(None))
        } else {
            false
        }
    }
    pub fn start(&mut self, s: &Settings) -> Result<(), String> {
        if self.cancelled() { return Err("Подключение отменено".into()); }
        if self.running() {
            return Ok(());
        }
        if s.mode == "tun" && !self.elevated {
            crate::broker::validate_settings(s)?;
            self.validate(s)?;
            let broker = std::sync::Arc::new(crate::broker::Broker::launch_cancellable(self.continue_running.as_deref())?);
            std::fs::write(
                self.directory.join("tun-guard.active"),
                b"Atlas active network session",
            )
            .map_err(|e| e.to_string())?;
            self.broker = Some(broker.clone());
            // Do not issue Stop on a failed Start reply: the service may already
            // own a healthy session. The caller checks service state and keeps
            // any confirmed connection alive.
            let response = broker.call_cancellable("start", serde_json::to_value(s).map_err(|e| e.to_string())?, self.continue_running.as_deref())?;
            if response["recovering"] == true {
                let deadline = Instant::now() + Duration::from_secs(240);
                loop {
                    if self.cancelled() { return Err("Подключение отменено".into()); }
                    if Instant::now() >= deadline {
                        return Err("Служба Atlas не завершила восстановление за 240 секунд".into());
                    }
                    let status = broker.call_cancellable("status", Value::Null, self.continue_running.as_deref())?;
                    if status["running"] == true && status["guard"] == true { break; }
                    if status["state"] == "Disconnected" {
                        return Err("Служба Atlas завершила восстановление без активной VPN-сессии".into());
                    }
                    thread::sleep(Duration::from_millis(250));
                }
            }
            self.started = Some(Instant::now());
            return Ok(());
        }
        for port in self.ports {
            std::net::TcpListener::bind(("127.0.0.1", port))
                .map_err(|_| format!("Порт {port} занят другим приложением"))?;
        }
        let prepared=crate::xray_runtime::Prepared::new(s,&self.binary,&self.directory)?.cancellable(self.continue_running.clone());
        let path=self.validate_mapped(&prepared.settings)?;
        let xray=prepared.start_logged(self.logs.clone())?;
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
        self.xray=Some(xray);
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
                        while buffer.len() > 4096 {
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
        // Cold Windows CI workers concurrently start several real-core fixtures.
        // Give test processes scheduling headroom without changing app deadlines.
        #[cfg(test)]
        let deadline = deadline + Duration::from_secs(25);
        while Instant::now() < deadline {
            if self.cancelled() {
                self.stop()?;
                return Err("Подключение отменено".into());
            }
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
                // The listener can be ready while selecting or persisting the
                // candidate still fails. Never leave that core (and its TUN)
                // running after reporting a failed start.
                if let Err(error) = self
                    .api("PUT", "/proxies/ATLAS", Some(json!({"name":s.selected})))
                    .and_then(|_| self.commit(&path))
                {
                    return match self.stop() {
                        Ok(()) => Err(error),
                        Err(cleanup) => Err(format!("{error}; очистка после ошибки запуска: {cleanup}")),
                    };
                }
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
        #[cfg(test)]
        let error = format!("{error}; fixture core log: {}", self.logs.lock()
            .map(|lines| lines.iter().rev().take(8).cloned().collect::<Vec<_>>().join(" | "))
            .unwrap_or_else(|_| "log unavailable".into()));
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
    pub(crate) fn cancelled(&self) -> bool {
        self.continue_running.as_ref().is_some_and(|flag| !flag.load(std::sync::atomic::Ordering::SeqCst))
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
    fn reap_draining_xray(&mut self) {
        if self.draining_xray.is_empty() || self.last_drain_check.is_some_and(|t|t.elapsed()<Duration::from_secs(2)) {return;}
        self.last_drain_check=Some(Instant::now());
        let before=self.draining_xray.len();
        self.draining_xray.retain(|runtime| runtime.has_live_connections().unwrap_or(true));
        if before!=self.draining_xray.len() {
            self.client().event(json!({"at":crate::model::now(),"kind":"xray_streams_drained","released":before-self.draining_xray.len(),"remaining":self.draining_xray.len()}));
        }
    }
    pub fn apply(&mut self, s: &Settings) -> Result<(), String> {
        if let Some(broker) = &self.broker {
            crate::broker::validate_settings(s)?;
            broker.call_cancellable("apply", serde_json::to_value(s).map_err(|e| e.to_string())?, self.continue_running.as_deref())?;
            return Ok(());
        }
        if !self.running() { return self.validate(s).map(|_| ()); }
        self.apply_verified(s, |_, _| Ok(()))
    }
    /// Validate a replacement in a separate loopback-only runtime. It cannot
    /// acquire TUN or change Windows proxy/routes/DNS, and never borrows the
    /// current core's configuration files or listeners.
    pub(crate) fn preflight_verified(&self, s: &Settings,
        verify: impl FnOnce(&mut Self) -> Result<(), String>) -> Result<(), String> {
        let directory = self.directory.join(format!("preflight-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).map_err(|e| e.to_string())?;
        let mut probe = Self::new(self.binary.clone(), directory.clone());
        probe.continue_running = self.continue_running.clone();
        let mut isolated = s.clone();
        isolated.mode = "system".into();
        let result = probe.use_ephemeral_ports().and_then(|_| probe.start(&isolated)).and_then(|_| verify(&mut probe));
        let stopped = probe.stop();
        let released = probe.owned_processes_released();
        drop(probe);
        let files = if released { std::fs::remove_dir_all(&directory).map_err(|e| e.to_string()) }
            else { Err("Проверочное ядро не подтвердило остановку".into()) };
        let errors: Vec<_> = [result, stopped, files].into_iter().filter_map(Result::err).collect();
        if errors.is_empty() { Ok(()) } else { Err(errors.join("; ")) }
    }
    /// Keep the previous YAML, live selector and Xray workers until both the
    /// candidate's path and disk commit have succeeded. Rollback uses the
    /// exact pre-transaction YAML, not a regenerated provider configuration.
    pub(crate) fn apply_verified(&mut self, s: &Settings,
        mut verify: impl FnMut(&mut Self, bool) -> Result<(), String>) -> Result<(), String> {
        let operation_id = uuid::Uuid::new_v4().to_string();
        self.client().event(json!({"kind":"configuration_transition","operationId":operation_id,
            "previousState":"Committed","nextState":"PreparingCandidate"}));
        self.reap_draining_xray();
        let mapped=self.xray.as_ref().and_then(|runtime|runtime.mapped_settings(s));
        // Bound worker accumulation without terminating anybody's live stream.
        if mapped.is_none() && self.draining_xray.len()>=8 {
            return Err("Обновление сетевой конфигурации отложено: предыдущие соединения ещё используются; текущий VPN продолжает работать".into());
        }
        let prepared=if mapped.is_none() {
            let prepared=crate::xray_runtime::Prepared::new(s,&self.binary,&self.directory)?.cancellable(self.continue_running.clone());
            prepared.validate()?; Some(prepared)
        } else {None};
        let path=self.validate_mapped(mapped.as_ref().unwrap_or_else(||&prepared.as_ref().unwrap().settings))?;
        if !self.running() { return Err("Ядро остановлено до применения новой конфигурации".into()); }
        let next_xray=prepared.map(|p|p.start_logged(self.logs.clone())).transpose()?;
        let old = self.directory.join("last-working.yaml");
        let previous = std::fs::read(&old).map_err(|e| format!("Не удалось сохранить конфигурацию для отката: {e}"))?;
        let selectors = self.api("GET", "/proxies", None)?;
        let selected = selectors["proxies"]["ATLAS"]["now"]
            .as_str().ok_or("Не удалось сохранить выбранный сервер для отката")?.to_owned();
        let result = self
            .api("PUT", "/configs?force=true", Some(json!({"path":path})))
            .and_then(|_| self.restore_nested_selectors(&selectors))
            .and_then(|_| self.api("PUT", "/proxies/ATLAS", Some(json!({"name":s.selected}))))
            .and_then(|_| self.api("GET", "/configs", None))
            .and_then(|config| {
                if s.mode == "tun" && config["tun"]["enable"] != true {
                    Err(self.tun_error())
                } else {
                    Ok(config)
                }
            }).and_then(|_| verify(self, false)).and_then(|_| self.commit(&path));
        if let Err(e) = result {
            self.client().event(json!({"kind":"configuration_transition","operationId":operation_id,
                "previousState":"Candidate","nextState":"RollingBack","error":e}));
            let rollback = (|| {
                // A failed commit may already have overwritten current/last-working.
                // Use the pre-transaction bytes and the live selector, not its old default.
                let rollback_path = self.directory.join("rollback.yaml");
                std::fs::write(&rollback_path, &previous).map_err(|e| e.to_string())?;
                self.api("PUT", "/configs?force=true", Some(json!({"path":rollback_path})))?;
                self.restore_nested_selectors(&selectors)?;
                self.api("PUT", "/proxies/ATLAS", Some(json!({"name":selected})))?;
                self.api("GET", "/version", None)?;
                let restored = self.api("GET", "/configs", None)?;
                let expected: Value = serde_yaml::from_slice(&previous).map_err(|e| e.to_string())?;
                if restored["tun"]["enable"] != expected["tun"]["enable"]
                    || self.api("GET", "/proxies/ATLAS", None)?["now"] != selected {
                    return Err("Ядро не подтвердило восстановление конфигурации и сервера".to_owned());
                }
                verify(self, true)?;
                std::fs::write(self.directory.join("current.yaml"), &previous).map_err(|e| e.to_string())?;
                std::fs::write(&old, &previous).map_err(|e| e.to_string())?;
                let _ = std::fs::remove_file(rollback_path);
                Ok::<(), String>(())
            })();
            if let Err(rollback_error) = rollback {
                self.client().event(json!({"kind":"configuration_transition","operationId":operation_id,
                    "previousState":"RollingBack","nextState":"RecoveryRequired","error":rollback_error}));
                let cleanup = self.stop();
                return Err(format!("{e}. Откат не подтверждён: {rollback_error}. {}",
                    match cleanup {
                        Ok(()) => "Сессия остановлена.".to_owned(),
                        Err(error) => format!("Остановка сессии не подтверждена: {error}"),
                    }));
            }
            self.client().event(json!({"kind":"configuration_transition","operationId":operation_id,
                "previousState":"RollingBack","nextState":"PreviousRestored"}));
            return Err(e);
        }
        if let Some(next_xray)=next_xray {
            if let Some(previous)=self.xray.replace(next_xray) {self.draining_xray.push(previous);}
            // Leave a grace interval for in-flight SOCKS accepts around reload.
            self.last_drain_check=Some(Instant::now());
            self.client().event(json!({"at":crate::model::now(),"kind":"xray_transport_replaced","existingStreamsPreserved":true,"drainingWorkers":self.draining_xray.len()}));
        }
        self.client().event(json!({"kind":"configuration_transition","operationId":operation_id,
            "previousState":"CandidateVerified","nextState":"Committed"}));
        Ok(())
    }
    pub fn select(&self, name: &str) -> Result<(), String> {
        if let Some(broker) = &self.broker {
            broker.call_cancellable("select", json!({"name":name}), self.continue_running.as_deref())?;
        } else {
            self.api("PUT", "/proxies/ATLAS", Some(json!({"name":name})))?;
            if self.api("GET", "/proxies/ATLAS", None)?["now"] != name {
                return Err("Ядро не подтвердило выбранный сервер".into());
            }
        }
        Ok(())
    }
    pub fn stop(&mut self) -> Result<(), String> {
        self.stop_with_tun_observer(crate::network_guard::tun_identity)
    }

    fn stop_with_tun_observer(&mut self, tun_identity: impl Fn() -> Option<u64>) -> Result<(), String> {
        // An Xray error must never skip termination of Mihomo or the service.
        for runtime in self.xray.iter_mut().chain(self.draining_xray.iter_mut()) {runtime.signal_stop();}
        let mut xray_errors=Vec::new();
        for runtime in self.xray.iter_mut().chain(self.draining_xray.iter_mut()) {
            if let Err(error)=runtime.stop() {xray_errors.push(error);}
        }
        let xray_result=if xray_errors.is_empty() {Ok(())} else {Err(xray_errors.join("; "))};
        self.xray=None;
        self.draining_xray.clear();
        let had_privileged_core = self.elevated && self.child.is_some();
        let had_broker = self.broker.is_some();
        if had_privileged_core || had_broker {
            // Capture the verified GUID/LUID before Windows withdraws driver
            // metadata during teardown. This does not classify an UP TUN as free.
            let _ = tun_identity();
        }
        if self.broker.as_ref().is_some_and(|b| !b.alive()) {
            self.broker = None;
        }
        let mut result = xray_result;
        if let Some(broker) = self.broker.take() {
            let broker_result = broker.call("stop", Value::Null).map(|evidence| {
                crate::incident_history::record("service_shutdown_completed", evidence, &[]);
            });
            result = result.and(broker_result);
        }
        if had_broker {
            // Disconnect and Exit both require the on-demand service to be
            // STOPPED. A live controller must not outlast the desktop.
            let stopped = crate::service::stop_and_wait_timeout(Duration::from_secs(1));
            result = match (result, stopped) {
                (Ok(()), Ok(())) => Ok(()),
                (Err(a), Ok(())) | (Ok(()), Err(a)) => Err(a),
                (Err(a), Err(b)) => Err(format!("{a}; {b}")),
            };
        }
        if self.elevated && self.child.is_some() {
            // Give the core a short chance to release TUN cleanly. A lost
            // desktop skips controller I/O; the owning job remains the hard stop.
            let deadline = Instant::now() + Duration::from_millis(500);
            let started = Instant::now();
            let initial_tun = tun_identity();
            let pid = self.child.as_ref().map(|c|c.id());
            self.client().event(json!({"at":crate::model::now(),"kind":"core_shutdown_started",
                "pid":pid,"tunLuid":initial_tun}));
            let closed = if self.cancelled() { Ok(Value::Null) }
                else { self.api("PATCH", "/configs", Some(json!({"tun":{"enable":false}}))) };
            let close_ms = started.elapsed().as_millis();
            while !self.cancelled() && tun_identity().is_some() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(25));
            }
            self.client().event(json!({"at":crate::model::now(),"kind":"core_shutdown_cleanup",
                "pid":pid,"initialTunLuid":initial_tun,"remainingTunLuid":tun_identity(),
                "closeMs":close_ms,"elapsedMs":started.elapsed().as_millis(),"ownerCancelled":self.cancelled(),
                "closeError":closed.err(),"tunRemainingBeforeTermination":tun_identity().is_some()}));
        }
        if let Some(mut c) = self.child.take() {
            let job = self.job.take();
            // Windows may finish pending socket I/O after accepting termination.
            // This is a maximum wait on the process handle, not a fixed delay;
            // a normally exited process returns immediately.
            if let Err(error) = crate::process_stop::stop(&mut c, || drop(job), Duration::from_secs(1)) {
                self.child = Some(c);
                if let Ok(mut logs) = self.logs.lock() { logs.push_back(error.clone()); }
                return Err(error);
            }
            self.client().event(json!({"at":crate::model::now(),"kind":"core_shutdown_process_exited","pid":c.id()}));
        }
        self.started = None;
        self.job = None;
        if had_privileged_core {
            let deadline = Instant::now() + Duration::from_secs(1);
            while tun_identity().is_some() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(50));
            }
            if tun_identity().is_some() {
                self.client().event(json!({"at":crate::model::now(),"kind":"core_shutdown_tun_remaining","tunLuid":tun_identity()}));
                return Err("Ядро остановлено, но освобождение Atlas-TUN не подтверждено".into());
            }
            self.client().event(json!({"at":crate::model::now(),"kind":"core_shutdown_tun_released"}));
        }
        // The marker is ownership evidence for recovery after an interrupted
        // shutdown. Removing it before the service and TUN are confirmed gone
        // makes the next Atlas instance treat its own adapter as foreign.
        if result.is_ok() {
            let _ = std::fs::remove_file(self.directory.join("tun-guard.active"));
        }
        result
    }
}
impl Drop for Core {
    fn drop(&mut self) {
        // The service observes IPC loss and releases its dynamic WFP session.
        if let Some(mut child) = self.child.take() {
            let job = self.job.take();
            if let Err(error) = crate::process_stop::stop(&mut child, || drop(job), Duration::from_secs(1)) {
                if let Ok(mut logs) = self.logs.lock() { logs.push_back(error.clone()); }
                eprintln!("{error}");
            }
        }
        self.broker = None;
        self.job = None;
    }
}

fn controller_timeout(method: &str, path: &str, body: Option<&Value>) -> Duration {
    Duration::from_secs(if method == "PATCH" && path == "/configs"
        && body.is_some_and(|value| value["tun"]["enable"] == false) {
        1
    } else if path.contains("/delay?") { 12 } else { 3 })
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::model::{Route, Rule, RuleGroup, Subscription};
    #[test]
    fn persistent_xray_stream_survives_configuration_replacement() {
        check_persistent_xray_stream(true);
    }
    #[test]
    fn stopping_core_terminates_current_and_draining_xray_streams() {
        check_persistent_xray_stream(false);
    }
    fn check_persistent_xray_stream(drain_before_stop: bool) {
        use std::io::{Read, Write};
        let directory = std::env::temp_dir().join(format!("atlas-stream-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let binary = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/Atlas.Core.exe");
        let mut core = Core::new(binary, directory.clone());
        core.use_ephemeral_ports().unwrap();
        let mut settings = Settings::default();
        settings.mode = "system".into();
        settings.selected = "fixture".into();
        settings.routing_mode = crate::model::RoutingMode::Global;
        settings.subscriptions.push(Subscription { source: Default::default(), options: Default::default(), id: "fixture".into(), name: "fixture".into(), masked_url: String::new(), updated_at: 0, error: None,
            servers: vec![json!({"name":"fixture","type":"xray","server":"127.0.0.1","port":443,"xray":{"outbounds":[{"protocol":"freedom"}]}})] });
        let mut alternative=settings.subscriptions[0].servers[0].clone();
        alternative["name"]=json!("alternative");
        settings.subscriptions[0].servers.push(alternative);
        core.start(&settings).unwrap();
        let target = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        target.set_nonblocking(true).unwrap();
        let target_port = target.local_addr().unwrap().port();
        let worker = thread::spawn(move || {
            let deadline=Instant::now()+Duration::from_secs(10);
            let (mut stream, _) = loop {
                match target.accept() {
                    Ok(stream)=>break stream,
                    Err(error) if error.kind()==std::io::ErrorKind::WouldBlock && Instant::now()<deadline=>thread::sleep(Duration::from_millis(10)),
                    _=>return,
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
            let mut bytes = [0; 4];
            while stream.read_exact(&mut bytes).is_ok() {
                if stream.write_all(&bytes).is_err() { break; }
            }
        });
        let mut held_stream=None;
        let result = (|| -> Result<(), String> {
            let mut stream = std::net::TcpStream::connect(("127.0.0.1", core.ports[0])).map_err(|e| e.to_string())?;
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            write!(stream, "CONNECT 127.0.0.1:{target_port} HTTP/1.1\r\nHost: 127.0.0.1:{target_port}\r\n\r\n").map_err(|e| e.to_string())?;
            let mut header = Vec::new();
            while !header.ends_with(b"\r\n\r\n") {
                let mut byte = [0]; stream.read_exact(&mut byte).map_err(|e| e.to_string())?; header.push(byte[0]);
                if header.len() > 4096 { return Err("Oversized CONNECT reply".into()); }
            }
            if !String::from_utf8_lossy(&header).contains("200") { return Err("CONNECT rejected".into()); }
            for step in 0..3 {
                if step == 1 {
                    core.select("alternative")?; core.apply(&settings)?;
                    if !core.draining_xray.is_empty() {return Err("Unchanged Xray configuration restarted workers".into());}
                }
                if step == 2 {
                    settings.subscriptions[0].servers.push(json!({"name":"added","type":"xray","server":"127.0.0.1","port":443,"xray":{"outbounds":[{"protocol":"freedom"}]}}));
                    core.apply(&settings)?;
                    if core.draining_xray.len()!=1 || !core.draining_xray[0].has_live_connections()? {
                        return Err("Existing stream ownership was not preserved".into());
                    }
                    core.last_drain_check=None;
                    core.reap_draining_xray();
                    if core.draining_xray.len()!=1 {return Err("Live worker was reaped".into());}
                }
                stream.write_all(b"live").map_err(|e| format!("step {step} write: {e}"))?;
                let mut reply = [0; 4]; stream.read_exact(&mut reply).map_err(|e| format!("step {step} read: {e}"))?;
                if &reply != b"live" { return Err("Corrupted stream".into()); }
            }
            if drain_before_stop {
                drop(stream);
                let deadline=Instant::now()+Duration::from_secs(5);
                while !core.draining_xray.is_empty() && Instant::now()<deadline {
                    core.last_drain_check=None; core.reap_draining_xray();
                    thread::sleep(Duration::from_millis(20));
                }
                if !core.draining_xray.is_empty() {return Err("Idle retired worker was not released".into());}
            } else {
                held_stream=Some(stream);
            }
            Ok(())
        })();
        let stopped = core.stop();
        let clean=core.draining_xray.is_empty() && core.xray.is_none() && !core.running();
        drop(held_stream);
        worker.join().unwrap();
        drop(core); std::fs::remove_dir_all(directory).unwrap();
        stopped.unwrap();
        assert!(clean,"Exit must release both current and retired workers");
        result.unwrap();
    }
    #[test]
    fn real_xray_bridge_survives_selection_reload_and_stops_with_core() {
        use std::io::{Read,Write};
        let directory=std::env::temp_dir().join(format!("atlas-xray-chain-{}",uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let binary=PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/Atlas.Core.exe");
        let mut core=Core::new(binary,directory.clone());core.use_ephemeral_ports().unwrap();
        let mut settings=Settings::default();settings.mode="system".into();settings.selected="fixture".into();
        settings.routing_mode=crate::model::RoutingMode::Global;
        settings.subscriptions.push(Subscription { source: Default::default(),options:Default::default(),id:"fixture".into(),name:"fixture".into(),masked_url:String::new(),updated_at:0,error:None,
            servers:vec![json!({"name":"fixture","type":"xray","server":"127.0.0.1","port":443,"xray":{"outbounds":[{"protocol":"freedom"}]}})]});
        core.start(&settings).unwrap();
        let target=std::net::TcpListener::bind(("127.0.0.1",0)).unwrap();target.set_nonblocking(true).unwrap();
        let target_port=target.local_addr().unwrap().port();
        let worker=thread::spawn(move || {
            for _ in 0..2 {
                let deadline=Instant::now()+Duration::from_secs(10);
                let (mut stream,_)=loop {match target.accept() {Ok(v)=>break v,Err(_) if Instant::now()<deadline=>thread::sleep(Duration::from_millis(10)),Err(e)=>panic!("{e}")}};
                stream.set_nonblocking(false).unwrap();stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                let mut data=[0;4096];let read=stream.read(&mut data).unwrap();assert!(read>0);
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\natlas").unwrap();
            }
        });
        let client=reqwest::blocking::Client::builder().no_proxy().proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{}",core.ports[0])).unwrap()).timeout(Duration::from_secs(5)).build().unwrap();
        for reload in [false,true] {
            if reload {core.apply(&settings).unwrap();}
            core.select("fixture").unwrap();
            let response=client.get(format!("http://127.0.0.1:{target_port}/")).send().unwrap().text().unwrap();
            assert_eq!(response,"atlas");assert!(core.running());
        }
        core.stop().unwrap();assert!(core.xray.is_none());assert!(!core.running());
        worker.join().unwrap();drop(core);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn privileged_stop_closes_tun_before_terminating_owned_process() {
        check_privileged_stop(Duration::from_millis(700), true);
    }
    #[test]
    fn healthy_shutdown_does_not_wait_for_grace_period_deadlines() {
        check_privileged_stop(Duration::ZERO, true);
    }
    #[test]
    fn stuck_tun_does_not_hold_shutdown_for_the_old_twenty_second_grace_period() {
        check_privileged_stop(Duration::ZERO, false);
    }
    fn check_privileged_stop(close_delay: Duration, release_tun: bool) {
        use std::io::{Read, Write};
        use std::os::windows::io::AsRawHandle;
        let api = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let directory=std::env::temp_dir().join(format!("atlas-stop-order-{}",uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let marker=directory.join("tun-guard.active");
        std::fs::write(&marker,b"owned session").unwrap();
        let mut core = Core::privileged(
            PathBuf::from("unused.exe"),
            directory.clone(),
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
        let released = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_released = released.clone();
        let worker = thread::spawn(move || {
            let (mut socket, _) = api.accept().unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
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
            // Keep Close inside its one-second bound and verify the owner stays
            // alive until the synchronous TUN release reply arrives.
            thread::sleep(close_delay);
            assert_eq!(
                unsafe {
                    windows_sys::Win32::System::Threading::WaitForSingleObject(handle as _, 0)
                },
                windows_sys::Win32::Foundation::WAIT_TIMEOUT
            );
            if release_tun {
                worker_released.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            socket
                .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
                .unwrap();
        });
        // This fixture has a mock API and no TUN. Do not observe the user's
        // unrelated live Atlas adapter when testing owned-process stop order.
        let started = Instant::now();
        let stopped = core.stop_with_tun_observer(|| {
            (!released.load(std::sync::atomic::Ordering::SeqCst)).then_some(1)
        });
        if release_tun {
            stopped.unwrap();
            assert!(!marker.exists(),"confirmed shutdown must clear the ownership marker");
        } else {
            assert!(stopped.unwrap_err().contains("освобождение Atlas-TUN не подтверждено"));
            assert!(marker.exists(),"failed shutdown must retain ownership evidence for recovery");
            assert!(started.elapsed() < Duration::from_secs(4), "shutdown took {:?}", started.elapsed());
        }
        if release_tun && close_delay.is_zero() { assert!(started.elapsed()<Duration::from_secs(2)); }
        worker.join().unwrap();
        assert!(core.child.is_none());
        assert!(!core.running());
        drop(core);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn shutdown_timeout_does_not_slow_status_or_other_config_requests() {
        assert_eq!(controller_timeout("PATCH", "/configs", Some(&json!({"tun":{"enable":false}}))), Duration::from_secs(1));
        assert_eq!(controller_timeout("GET", "/configs", None), Duration::from_secs(3));
        assert_eq!(controller_timeout("PATCH", "/configs", Some(&json!({"tun":{"enable":true}}))), Duration::from_secs(3));
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
    fn fifty_probe_only_core_cycles_leave_no_owned_process_or_network_listener() {
        let directory = std::env::temp_dir().join(format!("atlas-fifty-cycles-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let binary = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/Atlas.Core.exe");
        let mut core = Core::new(binary, directory.clone());
        core.use_ephemeral_ports().unwrap();
        let mut settings = Settings::default();
        settings.mode = "system".into();
        settings.subscriptions.push(Subscription { source: Default::default(), options: Default::default(), id:"fixture".into(), name:"fixture".into(),
            masked_url:String::new(), updated_at:0, error:None,
            servers:vec![json!({"name":"fixture","type":"direct"})] });
        for cycle in 0..50 {
            core.start(&settings).unwrap_or_else(|e| panic!("cycle {cycle} start: {e}"));
            assert!(core.running(), "cycle {cycle}: core stopped before disconnect");
            assert_eq!(core.api("GET", "/configs", None).unwrap()["tun"]["enable"], false);
            core.stop().unwrap_or_else(|e| panic!("cycle {cycle} stop: {e}"));
            assert!(!core.running(), "cycle {cycle}: owned process survived disconnect");
            assert!(std::net::TcpListener::bind(("127.0.0.1", core.ports[1])).is_ok(),
                "cycle {cycle}: local controller listener survived exit");
        }
        drop(core);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn real_core_accepts_empty_full_tunnel_and_route_exceptions() {
        let directory =
            std::env::temp_dir().join(format!("atlas-config-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let core = Core::new(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/Atlas.Core.exe"),
            directory.clone(),
        );
        let mut s = Settings::default();
        s.default_route = Route::Proxy;
        s.subscriptions.push(Subscription { source: Default::default(), options: Default::default(),id:"fixture".into(),name:"fixture".into(),masked_url:"hidden".into(),updated_at:0,error:None,servers:vec![json!({"name":"fixture","type":"ss","server":"127.0.0.1","port":1,"cipher":"aes-128-gcm","password":"fixture-only"})]});
        for text in ["version: 1\ndefault-route: proxy\nrules: []", "version: 1\ndefault-route: proxy\nrules:\n - {domain-suffix: example.com, route: direct}\n - {ip-cidr: 192.0.2.0/24, route: block, no-resolve: true}"] {
            let import = crate::portable::parse(text).unwrap(); s.groups = import.groups;
            for stack in [crate::model::TunStack::Gvisor, crate::model::TunStack::Mixed] {
                s.tun_stack = stack;
                for ipv6 in [false, true] { s.dns.ipv6 = ipv6; core.validate(&s).expect("Mihomo TUN configuration must be valid"); }
            }
        }
        drop(core);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn nested_selectors_survive_real_reload_rotation_and_rollback() {
        let directory=std::env::temp_dir().join(format!("atlas-selector-test-{}",uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let binary=PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/Atlas.Core.exe");
        let mut core=Core::new(binary,directory.clone());
        core.use_ephemeral_ports().unwrap();
        let mut settings=Settings::default(); settings.mode="system".into();
        settings.subscriptions.push(Subscription { source:Default::default(), options:Default::default(),id:"fixture".into(),name:"fixture".into(),masked_url:"hidden".into(),updated_at:0,error:None,
            servers:vec![json!({"name":"first","type":"direct"}),json!({"name":"chosen · source-1111111111111111","type":"direct"})] });
        core.start(&settings).unwrap();
        for group in ["AUTO","FAILOVER"] { core.api("PUT",&format!("/proxies/{group}"),Some(json!({"name":"chosen · source-1111111111111111"}))).unwrap(); }
        core.apply(&settings).unwrap();
        for group in ["AUTO","FAILOVER"] { assert_eq!(core.api("GET",&format!("/proxies/{group}"),None).unwrap()["now"],"chosen · source-1111111111111111"); }
        settings.subscriptions[0].servers[1]["name"]=json!("chosen · source-2222222222222222");
        core.apply(&settings).unwrap();
        let mut candidate=settings.clone();candidate.subscriptions[0].servers[1]["name"]=json!("chosen · source-3333333333333333");
        assert!(core.apply_verified(&candidate,|_,rollback| if rollback {Ok(())} else {Err("injected failure".into())}).is_err());
        for group in ["AUTO","FAILOVER"] { assert_eq!(core.api("GET",&format!("/proxies/{group}"),None).unwrap()["now"],"chosen · source-2222222222222222"); }
        core.stop().unwrap();drop(core);std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn real_mihomo_validation_reload_selector_and_rollback() {
        let directory =
            std::env::temp_dir().join(format!("atlas-core-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let binary = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/Atlas.Core.exe");
        let mut core = Core::new(binary, directory.clone());
        core.use_ephemeral_ports().unwrap();
        let mut settings = Settings::default();
        settings.mode = "system".into();
        // Isolated fixture. No real subscription, credentials or OS proxy changes.
        settings.subscriptions.push(Subscription { source: Default::default(), options: Default::default(),id:"fixture".into(),name:"fixture".into(),masked_url:"hidden".into(),updated_at:0,error:None,servers:vec![json!({"name":"fixture","type":"ss","server":"127.0.0.1","port":1,"cipher":"aes-128-gcm","password":"fixture-only"})]});
        let mut tun_candidate = settings.clone();
        tun_candidate.mode = "tun".into();
        core.validate(&tun_candidate)
            .expect("Mihomo must accept dual-stack TUN configuration");
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
        // A candidate health failure must run rollback BEFORE committing any
        // last-working bytes, using the old live selector and same core PID.
        let mut candidate = settings.clone();
        candidate.selected = "fixture".into();
        assert!(core.preflight_verified(&candidate, |_| Err("injected preflight failure".into())).is_err());
        assert!(core.running());
        assert_eq!(core.child.as_ref().unwrap().id(), process);
        assert_eq!(std::fs::read(directory.join("last-working.yaml")).unwrap(), working);
        let mut phases = Vec::new();
        let error = core.apply_verified(&candidate, |core, rollback| {
            phases.push(rollback);
            assert_eq!(std::fs::read(directory.join("last-working.yaml")).unwrap(), working);
            assert_eq!(core.api("GET", "/proxies/ATLAS", None)?["now"],
                if rollback { "FAILOVER" } else { "fixture" });
            if rollback { Ok(()) } else { Err("injected candidate health failure".into()) }
        }).unwrap_err();
        assert!(error.contains("injected candidate health failure"));
        assert_eq!(phases, vec![false, true]);
        assert!(core.running());
        assert_eq!(core.child.as_ref().unwrap().id(), process);
        assert_eq!(std::fs::read(directory.join("last-working.yaml")).unwrap(), working);
        let mut invalid = settings.clone();
        invalid.subscriptions[0].servers[0]["type"] = json!("invalid-proxy-protocol");
        assert!(core.apply(&invalid).is_err());
        assert_eq!(
            std::fs::read(directory.join("last-working.yaml")).unwrap(),
            working
        );
        assert!(core.api("GET", "/version", None).is_ok());
        // Force a disk commit failure AFTER the real core applied the candidate.
        // The live manual choice differs from the selector default in the old YAML.
        core.select("fixture").unwrap();
        let previous_path = directory.join("previous.yaml");
        if previous_path.exists() { std::fs::remove_file(&previous_path).unwrap(); }
        std::fs::create_dir(&previous_path).unwrap();
        let mut changed = settings.clone();
        changed.selected = "AUTO".into();
        assert!(core.apply(&changed).is_err());
        assert!(core.running());
        assert_eq!(core.api("GET", "/proxies/ATLAS", None).unwrap()["now"], "fixture");
        assert_eq!(std::fs::read(directory.join("last-working.yaml")).unwrap(), working);
        std::fs::remove_dir(previous_path).unwrap();
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
                    stream.set_nonblocking(false).unwrap();
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
        // If rollback itself cannot be persisted, do not keep an unknown session alive.
        std::fs::remove_file(directory.join("previous.yaml")).unwrap();
        std::fs::create_dir(directory.join("previous.yaml")).unwrap();
        std::fs::create_dir(directory.join("rollback.yaml")).unwrap();
        let error = core.apply(&settings).unwrap_err();
        assert!(error.contains("Откат не подтверждён"));
        assert!(!core.running());
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
    pub(crate) fn session_running(&self) -> Result<bool,String> {
        if let Some(broker)=&self.broker {
            if !broker.alive() {
                let process = crate::service::process_snapshot()?;
                if matches!(process.state, "Stopped" | "Missing") { return Ok(false); }
                broker.reconnect()?;
            }
            return broker.call("status",Value::Null).map(|s|s["running"]==true && s["guard"]==true);
        }
        self.api("GET","/version",None).map(|_|true)
    }
    #[cfg(test)]
    pub(crate) fn loopback_fixture(port: u16) -> Self {
        Self { controller_port:port, secret:String::new(), broker:None, logs:Default::default() }
    }
    pub(crate) fn event(&self, value: Value) {
        // Service-side events travel through the existing authenticated logs pipe.
        if let Ok(mut logs) = self.logs.lock() {
            logs.push_back(format!("ATLAS_EVENT {}", value));
            while logs.len() > 4096 { logs.pop_front(); }
        }
    }
    pub fn support_snapshot(&self) -> Result<Value, String> {
        if let Some(broker) = &self.broker {
            let job = broker.call("support_snapshot", Value::Null)?;
            let deadline = Instant::now() + Duration::from_secs(28);
            loop {
                if Instant::now() >= deadline { return Err("Privileged snapshot exceeded 28 seconds".into()); }
                let result = broker.call("delay_result", json!({"id":job["id"]}))?;
                if result["done"] == true { return Ok(result["value"].clone()); }
                thread::sleep(Duration::from_millis(200));
            }
        }
        Err("No privileged service session; WFP evidence unavailable".into())
    }
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
        self.api_cancellable(method, path, body, None)
    }
    pub(crate) fn api_cancellable(&self, method: &str, path: &str, body: Option<Value>,
        cancelled: Option<&std::sync::atomic::AtomicBool>) -> Result<Value, String> {
        if let Some(broker) = &self.broker {
            if method == "GET" {
                // Reads, including connections/status, must not hold the command
                // loop while the core is stalled. Stop/select retain access to IPC.
                let delay = path.contains("/delay?");
                // The service worker reserves the shared network slot when it
                // actually begins the Mihomo request. Reserving one here too
                // would consume two slots per probe and can deadlock a full
                // batch while every caller waits for its own worker.
                let _read_permit = if delay { None } else {
                    Some(crate::query_admission::acquire(false)?)
                };
                let job = crate::query_admission::retry_busy(Instant::now()+Duration::from_secs(5), ||
                    broker.call(if delay { "delay" } else { "query" }, json!({"path":path})))?;
                let deadline = Instant::now() + Duration::from_secs(if delay { 40 } else { 15 });
                loop {
                    if Instant::now() >= deadline {
                        return Err("Проверка сервера превысила время ожидания".into());
                    }
                    // A busy IPC lock must not abandon a running job and fill
                    // the service queue with orphan results for 30 seconds.
                    let result = crate::query_admission::retry_busy(deadline, ||
                        broker.call("delay_result", json!({"id":job["id"]})))?;
                    if result["done"] == true {
                        return Ok(result["value"].clone());
                    }
                    thread::sleep(Duration::from_millis(if delay { 150 } else { 30 }));
                }
            }
            return broker.call("api", json!({"method":method,"path":path}));
        }
        // Service-owned scheduled probes and desktop-submitted probes share
        // one eight-slot admission gate before the network timeout starts.
        let _permit = if path.contains("/delay?") {
            Some(crate::query_admission::acquire_cancellable(true, cancelled)?)
        } else { None };
        if cancelled.is_some_and(|flag|flag.load(std::sync::atomic::Ordering::SeqCst)) {
            return Err("CANCELLED: конфигурация сети изменилась".into());
        }
        // Reuse connections to the local controller across status/traffic polls.
        // Authentication remains per request, so sessions never share credentials.
        static HTTP: std::sync::OnceLock<Result<reqwest::blocking::Client, String>> =
            std::sync::OnceLock::new();
        let client = HTTP.get_or_init(|| reqwest::blocking::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(1))
            .pool_max_idle_per_host(4)
            .pool_idle_timeout(Duration::from_secs(15))
            .build().map_err(|e| e.to_string())).as_ref().map_err(Clone::clone)?;
        let timeout = controller_timeout(method, path, body.as_ref());
        let method =
            reqwest::Method::from_bytes(method.as_bytes()).map_err(|_| "Некорректный метод")?;
        let mut req = client
            .request(
                method,
                format!("http://127.0.0.1:{}{path}", self.controller_port),
            )
            .bearer_auth(&self.secret)
            .timeout(timeout);
        if let Some(b) = body {
            req = req.json(&b)
        }
        let response = req.send().map_err(|e| {
            let mut chain=e.to_string();
            let mut source=std::error::Error::source(&e);
            while let Some(error)=source {chain.push_str(&format!(" -> {error}"));source=error.source();}
            let detail=crate::support_report::redact(&chain,&[self.secret.clone()]);
            self.event(json!({"at":crate::model::now(),"kind":"controller_transport_failure","path":path,
                "timeout":e.is_timeout(),"connectError":e.is_connect(),"error":detail}));
            format!("Mihomo API недоступен: {detail}")
        })?;
        let status = response.status();
        if !status.is_success() {
            let detail = response.text().unwrap_or_default().chars().take(1024).collect::<String>();
            self.event(json!({"at":crate::model::now(),"kind":"controller_http_failure","path":path,
                "status":status.as_u16(),"detail":crate::support_report::redact(&detail,&[self.secret.clone()])}));
            if path.starts_with("/dns/query?") {
                return Err(format!("Mihomo API: HTTP {}: {}",status.as_u16(),crate::support_report::redact(&detail,&[self.secret.clone()])));
            }
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

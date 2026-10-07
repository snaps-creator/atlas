//! Privileged network broker. Installed builds use an SCM-managed service;
//! the desktop client authenticates the pipe server against its SCM identity.
use crate::{core::Core, model::Settings, network_guard};
use serde_json::{json, Value};
use std::{
    fs::File,
    io::{Read, Write},
    os::windows::io::{AsRawHandle, FromRawHandle},
    path::PathBuf,
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    Security::{
        Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW, SECURITY_ATTRIBUTES,
    },
    Storage::FileSystem::*,
    System::{Pipes::*, Threading::*},
};
const LIMIT: usize = 16 * 1024 * 1024;
const SERVICE_PIPE: &str = r"\\.\pipe\Atlas-Network-Service";
const EMBEDDED_CORE: &[u8] = include_bytes!("../resources/Atlas.Core.exe");
const EMBEDDED_XRAY: &[u8] = include_bytes!("../resources/Atlas.Xray.exe");
const XRAY_GEOIP: &[u8] = include_bytes!("../resources/xray-assets/geoip.dat");
const XRAY_GEOSITE: &[u8] = include_bytes!("../resources/xray-assets/geosite.dat");
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
fn error(action: &str) -> String {
    format!("{action}: код Windows {}", unsafe { GetLastError() })
}
fn nonblocking(file: &File) -> Result<(), String> {
    let mode = PIPE_READMODE_BYTE | PIPE_NOWAIT;
    if unsafe {
        SetNamedPipeHandleState(
            file.as_raw_handle(),
            &mode,
            std::ptr::null(),
            std::ptr::null(),
        )
    } == 0
    {
        return Err(error("Режим канала сетевой службы"));
    }
    Ok(())
}
#[cfg(test)]
fn write_bytes(file: &mut File, bytes: &[u8], deadline: Instant) -> Result<(), String> {
    write_bytes_cancellable(file, bytes, deadline, None)
}
fn write_bytes_cancellable(file: &mut File, bytes: &[u8], deadline: Instant, running: Option<&std::sync::atomic::AtomicBool>) -> Result<(), String> {
    let mut offset = 0;
    while offset < bytes.len() {
        if running.is_some_and(|flag| !flag.load(std::sync::atomic::Ordering::SeqCst)) {
            return Err("Подключение отменено".into());
        }
        if Instant::now() >= deadline || crate::service::is_stopping() {
            return Err("Время записи в канал сетевой службы истекло".into());
        }
        // A byte-mode NOWAIT pipe can accept only part of a frame, including
        // zero bytes when its buffer is full. Never use blocking write_all here.
        let end = (offset + 4096).min(bytes.len());
        match file.write(&bytes[offset..end]) {
            Ok(0) => thread::sleep(Duration::from_millis(10)),
            Ok(n) => offset += n,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(_) => return Err("Канал сетевой службы закрыт".into()),
        }
    }
    Ok(())
}
fn send(file: &mut File, value: &Value) -> Result<(), String> {
    send_cancellable(file, value, None)
}
fn send_cancellable(file: &mut File, value: &Value, running: Option<&std::sync::atomic::AtomicBool>) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|_| "Ошибка кодирования команды")?;
    if bytes.len() > LIMIT {
        return Err("Команда превышает 16 МБ".into());
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    write_bytes_cancellable(file, &(bytes.len() as u32).to_le_bytes(), deadline, running)?;
    write_bytes_cancellable(file, &bytes, deadline, running)
}
fn read_bytes_cancellable(file: &mut File, buffer: &mut [u8], deadline: Instant, running: Option<&std::sync::atomic::AtomicBool>) -> Result<(), String> {
    let mut offset = 0;
    while offset < buffer.len() {
        if running.is_some_and(|flag| !flag.load(std::sync::atomic::Ordering::SeqCst)) {
            return Err("Подключение отменено".into());
        }
        if Instant::now() >= deadline || crate::service::is_stopping() {
            return Err("Время ожидания сетевой службы истекло".into());
        }
        let mut available = 0;
        unsafe {
            if PeekNamedPipe(
                file.as_raw_handle(),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut available,
                std::ptr::null_mut(),
            ) == 0
            {
                return Err("Канал сетевой службы закрыт".into());
            }
        }
        if available > 0 {
            let size = (available as usize).min(buffer.len() - offset);
            file.read_exact(&mut buffer[offset..offset + size])
                .map_err(|_| "Ошибка канала сетевой службы")?;
            offset += size;
        } else {
            if Instant::now() >= deadline {
                return Err("Время ожидания сетевой службы истекло".into());
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
    Ok(())
}
fn receive(file: &mut File, timeout: Duration) -> Result<Value, String> {
    receive_cancellable(file, timeout, None)
}
fn receive_cancellable(file: &mut File, timeout: Duration, running: Option<&std::sync::atomic::AtomicBool>) -> Result<Value, String> {
    let deadline = Instant::now() + timeout;
    let mut len = [0; 4];
    read_bytes_cancellable(file, &mut len, deadline, running)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > LIMIT {
        return Err("Ответ сетевой службы превышает лимит".into());
    }
    let mut data = vec![0; len];
    read_bytes_cancellable(file, &mut data, deadline, running)?;
    serde_json::from_slice(&data).map_err(|_| "Некорректный ответ сетевой службы".into())
}
pub struct Broker {
    pipe: Mutex<Option<File>>,
}
#[derive(Debug, PartialEq, Eq)]
enum ChannelRecoveryAction { Reconnect, ServiceStopped, WaitForStartup, Unknown }
fn channel_recovery_action(process_state: &str) -> ChannelRecoveryAction {
    match process_state {
        "Running" => ChannelRecoveryAction::Reconnect,
        "Stopped" | "Missing" => ChannelRecoveryAction::ServiceStopped,
        "Starting" | "Stopping" => ChannelRecoveryAction::WaitForStartup,
        _ => ChannelRecoveryAction::Unknown,
    }
}
impl Broker {
    fn reconnect_pipe(running: Option<&std::sync::atomic::AtomicBool>) -> Result<File, String> {
        let process = crate::service::process_snapshot()?;
        if channel_recovery_action(process.state) != ChannelRecoveryAction::Reconnect {
            return Err(format!("Служба Atlas: {}; канал отключён", process.state));
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if running.is_some_and(|flag| !flag.load(std::sync::atomic::Ordering::SeqCst)) {
                return Err("Подключение отменено".into());
            }
            if let Ok(mut file) = std::fs::OpenOptions::new().read(true).write(true).open(SERVICE_PIPE) {
                let mut pid = 0;
                if unsafe { GetNamedPipeServerProcessId(file.as_raw_handle(), &mut pid) } == 0 || pid != process.pid {
                    return Err("Процесс службы изменился во время восстановления канала".into());
                }
                crate::service::verify_server_pid(pid)?;
                nonblocking(&file)?;
                receive_service_ready(&mut file, running)?;
                crate::incident_history::record("service_channel_transition", json!({
                    "previousState":"Reconnecting","nextState":"Connected","serviceProcess":process}), &[]);
                return Ok(file);
            }
            if Instant::now() >= deadline { return Err("Восстановление канала службы превысило 3 секунды".into()); }
            crate::service::verify_server_pid(process.pid)?;
            thread::sleep(Duration::from_millis(50));
        }
    }
    pub(crate) fn reconnect(&self) -> Result<(), String> {
        let mut slot = self.pipe.try_lock().map_err(|_| "Канал занят текущей операцией; повторите проверку")?;
        if slot.as_ref().is_some_and(pipe_alive) { return Ok(()); }
        *slot = None;
        *slot = Some(Self::reconnect_pipe(None)?);
        Ok(())
    }
    pub(crate) fn close_channel(&self) -> Result<(), String> {
        let mut slot = self.pipe.try_lock().map_err(|_| "Канал занят текущей операцией")?;
        *slot = None;
        Ok(())
    }
    pub fn launch() -> Result<Self, String> { Self::launch_cancellable(None) }
    pub fn launch_cancellable(running: Option<&std::sync::atomic::AtomicBool>) -> Result<Self, String> {
        // A cold Windows service start can be delayed by signature/AV checks.
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if running.is_some_and(|flag| !flag.load(std::sync::atomic::Ordering::SeqCst)) {
                return Err("Подключение отменено".into());
            }
            // A previous session may still be shutting down when StartService
            // reports ALREADY_RUNNING. Retry startup until the new pipe exists.
            let start_error = crate::service::start_on_demand().err();
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(SERVICE_PIPE)
            {
                let mut server_pid = 0;
                unsafe {
                    if GetNamedPipeServerProcessId(file.as_raw_handle(), &mut server_pid) == 0 {
                        return Err("Нельзя подтвердить сетевую службу".into());
                    }
                }
                crate::service::verify_server_pid(server_pid)?;
                nonblocking(&file)?;
                receive_service_ready(&mut file, running)?;
                return Ok(Self {
                    pipe: Mutex::new(Some(file)),
                });
            }
            if Instant::now() >= deadline {
                return Err(start_error.unwrap_or_else(|| {
                    "Системная служба Atlas не открыла канал за 15 секунд".into()
                }));
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
    pub fn alive(&self) -> bool {
        let file = match self.pipe.try_lock() {
            Ok(file) => file,
            Err(std::sync::TryLockError::WouldBlock) => return true,
            Err(_) => return false,
        };
        let Some(file) = file.as_ref() else {
            return false;
        };
        let mut available = 0;
        unsafe {
            PeekNamedPipe(
                file.as_raw_handle(),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut available,
                std::ptr::null_mut(),
            ) != 0
        }
    }
    pub fn call(&self, op: &str, payload: Value) -> Result<Value, String> {
        self.call_cancellable(op, payload, None)
    }
    pub fn call_cancellable(&self, op: &str, payload: Value, running: Option<&std::sync::atomic::AtomicBool>) -> Result<Value, String> {
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut slot = loop {
            if running.is_some_and(|flag| !flag.load(std::sync::atomic::Ordering::SeqCst)) {
                return Err("Подключение отменено".into());
            }
            match self.pipe.try_lock() {
                Ok(slot) => break slot,
                Err(std::sync::TryLockError::WouldBlock) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(10))
                }
                _ => return Err("Сетевая служба занята. Повторите операцию.".into()),
            }
        };
        if slot.is_none() { *slot = Some(Self::reconnect_pipe(running)?); }
        let file = slot.as_mut().ok_or("Канал сетевой службы закрыт")?;
        let reply = (|| {
            send_cancellable(file, &json!({"op":op,"payload":payload}), running)?;
            // Starting can include driver installation; its IPC deadline must outlive readiness.
            receive_cancellable(
                file,
                Duration::from_secs(if matches!(op, "start" | "apply") { 240 }
                    else if op == "select" { 180 }
                    // Outlive graceful core/TUN teardown and final filter/DNS cleanup.
                    else if op == "stop" { 4 }
                    else if op == "status" { 2 } else { 15 }),
                running,
            )
        })();
        let reply = match reply {
            Ok(reply) => reply,
            Err(error) => {
                // A late reply must never be mistaken for the next command's reply.
                // A mutation may have completed: never replay it on a new pipe.
                crate::incident_history::record("service_transport_failed",json!({"operation":op,"error":error,
                    "channelState":"Disconnected","serviceProcess":crate::service::process_snapshot().ok()}),&[]);
                *slot = None;
                return Err(error);
            }
        };
        if let Some(e) = reply["error"].as_str() {
            crate::incident_history::record("service_operation_failed",json!({"operation":op,"error":e,"evidence":reply["evidence"]}),&[]);
            Err(e.into())
        } else {
            Ok(reply["result"].clone())
        }
    }
}
fn pipe_alive(file: &File) -> bool {
    let mut available = 0;
    unsafe { PeekNamedPipe(file.as_raw_handle(), std::ptr::null_mut(), 0,
        std::ptr::null_mut(), &mut available, std::ptr::null_mut()) != 0 }
}
fn receive_service_ready(pipe: &mut File, running: Option<&std::sync::atomic::AtomicBool>) -> Result<(), String> {
    let hello = receive_cancellable(pipe, Duration::from_secs(10), running)?;
    if hello["ready"] == true { return Ok(()); }
    let reason = hello["error"].as_str().unwrap_or("Сетевая служба не готова").to_owned();
    let _ = send(pipe, &json!({"startupErrorReceived":true}));
    crate::incident_history::record("service_startup_failed", json!({"error":reason}), &[]);
    Err(reason)
}
fn send_startup_failure(pipe: &mut File, reason: &str) -> Result<(), String> {
    send(pipe, &json!({"ready":false,"error":reason}))?;
    // Keep the frame alive until the desktop consumes it; never wait indefinitely.
    let _ = receive(pipe, Duration::from_secs(1));
    Ok(())
}
pub(crate) fn verify_process_image(pid: u32, expected: &std::path::Path) -> Result<(), String> {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return Err("Процесс сетевой службы недоступен".into());
    }
    let mut path = vec![0u16; 32768];
    let mut size = path.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(handle, 0, path.as_mut_ptr(), &mut size) };
    unsafe {
        CloseHandle(handle);
    }
    if ok == 0 {
        return Err("Нельзя проверить процесс сетевой службы".into());
    }
    let actual = PathBuf::from(String::from_utf16_lossy(&path[..size as usize]));
    match (actual.canonicalize(), expected.canonicalize()) {
        (Ok(actual), Ok(expected)) if actual == expected => Ok(()),
        _ => Err("Подключена посторонняя сетевая служба".into()),
    }
}
fn secure_directory() -> Result<PathBuf, String> {
    unsafe {
        let root = std::env::var_os("ProgramData").ok_or("Не найден ProgramData")?;
        let directory = PathBuf::from(root).join(format!("Atlas-session-{}", uuid::Uuid::new_v4()));
        let sddl = wide("D:P(A;;FA;;;SY)(A;;FA;;;BA)");
        let mut descriptor = std::ptr::null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor,
            std::ptr::null_mut(),
        ) == 0
        {
            return Err(error("Защита файлов ядра"));
        }
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        let ok = CreateDirectoryW(wide(&directory.to_string_lossy()).as_ptr(), &sa);
        LocalFree(descriptor);
        if ok == 0 {
            return Err(error("Не удалось создать защищённый каталог"));
        }
        std::fs::write(directory.join("Atlas.Core.exe"), EMBEDDED_CORE).map_err(|e| e.to_string())?;
        std::fs::write(directory.join("Atlas.Xray.exe"), EMBEDDED_XRAY).map_err(|e| e.to_string())?;
        std::fs::create_dir(directory.join("xray-assets")).map_err(|e|e.to_string())?;
        std::fs::write(directory.join("xray-assets/geoip.dat"),XRAY_GEOIP).map_err(|e|e.to_string())?;
        std::fs::write(directory.join("xray-assets/geosite.dat"),XRAY_GEOSITE).map_err(|e|e.to_string())?;
        Ok(directory)
    }
}
/// Reject features that could make an elevated core read/write caller-selected files.
pub fn validate_settings(settings: &Settings) -> Result<(), String> {
    if settings.mode != "tun" {
        return Err("Сетевая служба принимает только режим всей системы".into());
    }
    for node in settings.servers() {
        let kind = node["type"].as_str().ok_or("Отсутствует тип сервера")?;
        if kind=="xray" {
            crate::xray_config::controlled_profile(&node["xray"],json!({"tag":"atlas-validation"}))?;
            continue;
        }
        if ![
            "ss",
            "vmess",
            "vless",
            "trojan",
            "hysteria2",
            "socks5",
            "http",
            "tuic",
        ]
        .contains(&kind)
        {
            return Err(format!("Тип {kind} не разрешён в привилегированном ядре"));
        }
        reject_file_keys(&node)?;
    }
    crate::config::generate(settings, "validation").map(|_| ())
}
fn reject_file_keys(value: &Value) -> Result<(), String> {
    match value {
        Value::Object(map) => {
            for (key, v) in map {
                if [
                    "plugin",
                    "plugin-opts",
                    "private-key",
                    "private-key-path",
                    "certificate",
                    "certificate-path",
                    "ca",
                    "ca-str",
                    "path",
                    "dialer-proxy",
                    "interface-name",
                    "routing-mark",
                    "ip-version",
                ]
                .contains(&key.as_str())
                    && key != "path"
                {
                    return Err(format!("Параметр сервера {key} не разрешён для TUN"));
                }
                if key == "type"
                    && v.as_str()
                        .is_some_and(|s| ["direct", "reject", "dns"].contains(&s))
                {
                    return Err("Прямой маршрут нельзя использовать как VPN-сервер".into());
                }
                reject_file_keys(v)?;
            }
        }
        Value::Array(a) => {
            for v in a {
                reject_file_keys(v)?
            }
        }
        _ => {}
    }
    Ok(())
}
fn allowed_api(method: &str, path: &str) -> bool {
    if method == "GET" && path.starts_with("/dns/query?") {
        if let Ok(url) = url::Url::parse(&format!("http://localhost{path}")) {
            let pairs: Vec<_> = url.query_pairs().collect();
            return pairs.len() == 2 && pairs.iter().all(|(key,value)| match key.as_ref() {
                "name" => crate::support_probes::valid_dns_name(value),
                "type" => matches!(value.as_ref(),"A"|"AAAA"), _ => false,
            }) && pairs.iter().any(|(k,_)|k == "name") && pairs.iter().any(|(k,_)|k == "type");
        }
    }
    if method == "GET" && ["/version", "/connections", "/proxies"].contains(&path) {
        return true;
    }
    if method == "DELETE" && path.starts_with("/connections/") && !path.contains(['?', '#']) {
        return true;
    }
    if method == "GET" && (path.starts_with("/proxies/") || path.starts_with("/group/AUTO/delay?")) && path.contains("/delay?") {
        if let Ok(url) = url::Url::parse(&format!("http://localhost{path}")) {
            return url.query_pairs().all(|(k, v)| match k.as_ref() {
                "url" => crate::latency::ENDPOINTS.contains(&v.as_ref()),
                "timeout" => v.parse::<u64>().is_ok_and(|n| n <= 10000),
                "expected" => v == "204" || v == "200",
                _ => false,
            });
        }
    }
    false
}
struct PipeWatch {
    done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl PipeWatch {
    fn start(pipe: &File, running: std::sync::Arc<std::sync::atomic::AtomicBool>) -> Result<Self, String> {
        let pipe = pipe.try_clone().map_err(|e| e.to_string())?;
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stopped = done.clone();
        let worker = thread::Builder::new().name("atlas-pipe-watch".into()).spawn(move || {
            while !stopped.load(std::sync::atomic::Ordering::SeqCst) {
                let mut available = 0;
                let alive = unsafe { PeekNamedPipe(pipe.as_raw_handle(), std::ptr::null_mut(), 0,
                    std::ptr::null_mut(), &mut available, std::ptr::null_mut()) } != 0;
                if !alive || crate::service::is_stopping() {
                    running.store(false, std::sync::atomic::Ordering::SeqCst);
                    break;
                }
                thread::sleep(Duration::from_millis(25));
            }
        }).map_err(|e| e.to_string())?;
        Ok(Self { done, thread: Some(worker) })
    }
}
impl Drop for PipeWatch {
    fn drop(&mut self) {
        self.done.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(worker) = self.thread.take() { let _ = worker.join(); }
    }
}
struct Controller {
    session: NetworkSession,
    scheduler: Scheduler,
    selection: crate::resilient_selection::Recovery,
}
struct NetworkSession {
    core: Core,
    desktop_pid: u32,
    session_id: Option<uuid::Uuid>,
    network_epoch: u64,
    configured: bool,
    cleanup_observer: Option<std::process::Child>,
    guard: Option<network_guard::Guard>,
    active_settings: Option<Settings>,
}
struct Scheduler {
    health: HealthState,
    queries: crate::query_jobs::QueryJobs,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExistingSessionAction { StartFresh, Attach, Reconcile, WaitForRecovery, Inconsistent }

fn existing_session_action(configured: bool, core_running: bool, guard_owned: bool, settings_match: bool) -> ExistingSessionAction {
    match (configured, core_running, guard_owned, settings_match) {
        (false, _, _, _) => ExistingSessionAction::StartFresh,
        (true, true, _, true) => ExistingSessionAction::Attach,
        (true, true, _, false) => ExistingSessionAction::Reconcile,
        (true, false, true, _) => ExistingSessionAction::WaitForRecovery,
        (true, false, false, _) => ExistingSessionAction::Inconsistent,
    }
}

struct HealthState {
    last_recovery_scan: Instant,
    last_health: Instant,
    failures: u32,
    probe: crate::background_probe::Probe,
    tun_identity: Option<u64>,
    retry_at: Option<Instant>,
    attempts: std::collections::VecDeque<Instant>,
    uplink: crate::network_change::Monitor,
    path_due: bool,
    path_retry_at: Option<Instant>,
    path_probe: Option<(u64, (u64,u64), crate::background_probe::Probe<Option<bool>>)>,
}
impl Default for HealthState {
    fn default() -> Self {
        Self { last_recovery_scan: Instant::now(), last_health: Instant::now(), failures: 0,
            probe: Default::default(), tun_identity: network_guard::tun_identity(),
            retry_at: None, attempts: Default::default(), uplink: Default::default(),
            path_due: false, path_retry_at: None, path_probe: None }
    }
}
impl HealthState {
    fn reset(&mut self) {
        self.failures = 0;
        self.probe = Default::default();
        self.tun_identity = network_guard::tun_identity();
        self.last_health = Instant::now();
        self.retry_at = None;
        self.path_due = false;
        self.path_retry_at = None;
        self.path_probe = None;
    }
    fn tick(&mut self, core: &mut Core, configured: &mut bool,
        guard: &mut Option<network_guard::Guard>, settings: &mut Option<Settings>,
        recovery: &mut crate::resilient_selection::Recovery,
        observer: &mut Option<std::process::Child>,
        queries: &mut crate::query_jobs::QueryJobs,
        session_id: &mut Option<uuid::Uuid>, network_epoch: &mut u64, desktop_pid: u32) {
        let now = Instant::now();
        if settings.is_some() && self.uplink.tick() {
            *network_epoch = network_epoch.wrapping_add(1);
            queries.invalidate();
            recovery.network_changed();
            self.path_due = true;
            self.path_retry_at = None;
            // Retry exhaustion on the office uplink says nothing about home.
            self.attempts.clear();
            if !*configured && self.retry_at.is_some() { self.retry_at=Some(now+Duration::from_secs(2)); }
            core.client().event(json!({"at":crate::model::now(),"kind":"uplink_changed",
                "networkEpoch":*network_epoch,"action":"validate selected path before recovery"}));
        }
        if *configured && self.last_recovery_scan.elapsed() >= Duration::from_millis(500) {
            self.last_recovery_scan = now;
            if let Some(s) = settings { recovery.tick(core.client(), s); }
        }
        if *configured {
            if let Some(healthy) = self.probe.poll() {
                self.failures = if healthy { 0 } else { self.failures + 1 };
            }
            if self.last_health.elapsed() >= Duration::from_secs(5) {
                let observer_ok = observer.as_mut().is_some_and(|child| matches!(child.try_wait(), Ok(None)));
                let observed = network_guard::tun_identity();
                let guard_ready = match tun_guard_action(self.tun_identity, observed) {
                    TunGuardAction::Keep => true,
                    TunGuardAction::Missing => false,
                    TunGuardAction::Unowned => { self.failures = 3; false }
                };
                if !observer_ok || !core.running() { self.failures = 3; }
                else {
                    let client = core.client();
                    self.probe.start(move || guard_ready && client.api("GET", "/version", None).is_ok());
                }
                self.last_health = now;
            }
            if let Some((epoch, route_epoch, probe)) = &mut self.path_probe {
                if let Some(healthy) = probe.poll() {
                    let current=*epoch==*network_epoch && *route_epoch==recovery.path_epoch();
                    self.path_probe=None;
                    if !current { self.path_due=true; }
                    if current {
                        core.client().event(json!({"at":crate::model::now(),"kind":"uplink_path_checked","healthy":healthy}));
                        if healthy==Some(false) { self.failures=3; }
                        if healthy.is_none() {
                            self.path_due=true;
                            self.path_retry_at=Some(now+Duration::from_secs(5));
                        }
                    }
                }
            }
            if self.path_due && self.path_probe.is_none() && self.path_retry_at.is_none_or(|at|now>=at) {
                if let Some(s)=settings.as_ref() {
                    // Direct mode must not depend on the selected VPN node.
                    // Its local core/TUN health is still checked above.
                    if s.routing_mode==crate::model::RoutingMode::Direct {
                        self.path_due=false;
                    } else {
                        let client=core.client(); let selected=s.selected.clone();
                        let mut probe=crate::background_probe::Probe::default();
                        if probe.start(move || selected_path_responds(&client,&selected)) {
                            self.path_probe=Some((*network_epoch,recovery.path_epoch(),probe)); self.path_due=false;
                        }
                    }
                }
            }
            if self.failures >= 3 {
                *network_epoch = network_epoch.wrapping_add(1);
                queries.invalidate();
                // Commit protected pause before tearing down the owned core.
                let paused = guard.as_ref().map(|g| g.pause(&core.binary)).unwrap_or(Ok(()));
                let stopped = core.stop();
                *configured = false;
                recovery.invalidate();
                if paused.is_ok() && stopped.is_ok() && guard.as_ref().is_some_and(|g| g.reset_for_recovery().is_ok()) {
                    self.retry_at = Some(now + Duration::from_secs(3));
                } else {
                    self.retry_at = None;
                    *settings = None;
                    // Keep the dynamic WFP session until the owned core has
                    // been terminated by the service's final teardown.
                    *session_id = None;
                }
                self.probe = Default::default();
                self.path_probe = None;
                self.path_due = false;
                self.path_retry_at = None;
            }
        }
        if !*configured && settings.is_some() && self.retry_at.is_some_and(|t| now >= t) {
            while self.attempts.front().is_some_and(|t| now.duration_since(*t) > Duration::from_secs(600)) {
                self.attempts.pop_front();
            }
            if self.attempts.len() >= 5 {
                self.retry_at = None;
                // Recovery is exhausted. The service loop exits and drops its
                // dynamic WFP handle after stopping the owned core.
                *settings = None;
                *session_id = None;
                return;
            }
            let Ok(_update_admission) = crate::update_lock::UpdateLock::admit_session() else { return; };
            self.attempts.push_back(now);
            if observer.as_mut().is_none_or(|child| !matches!(child.try_wait(), Ok(None))) {
                match crate::session_cleanup::start_observer(&core.directory, desktop_pid) {
                    Ok(child) => *observer = Some(child),
                    Err(_) => {
                        self.retry_at = Some(now + Duration::from_secs(30));
                        return;
                    }
                }
            }
            let s = settings.as_ref().unwrap();
            let restarted = core.start(s)
                .and_then(|_| guard.as_ref().ok_or_else(|| "Защита сети недоступна".to_owned())?.install(&core.binary))
                .and_then(|_| confirm_route(&core.client(), s, &crate::latency::ENDPOINTS));
            if restarted.is_ok() {
                *configured = true;
                *session_id = Some(uuid::Uuid::new_v4());
                *network_epoch = network_epoch.wrapping_add(1);
                queries.invalidate();
                self.reset();
                recovery.invalidate();
            } else {
                let paused = guard.as_ref().map(|g| g.pause(&core.binary)).unwrap_or(Ok(()));
                let stopped = core.stop();
                if paused.is_err() || stopped.is_err() {
                    // A failed teardown must not feed the next retry back
                    // into a possibly live TUN or an unconfirmed WFP policy.
                    self.retry_at = None;
                    *settings = None;
                    *session_id = None;
                    return;
                }
                let backoff = (3u64.saturating_mul(1u64 << self.attempts.len().min(5))).min(60);
                self.retry_at = Some(Instant::now() + Duration::from_secs(backoff));
            }
        }
    }
}
impl Controller {
    fn new() -> Result<Self, String> {
        // One-time migration of the previous build's exact persistent WFP
        // keys. Current protection is dynamic and dies with this service.
        network_guard::clear()?;
        let directory = secure_directory()?;
        Ok(Self {
            session: NetworkSession {
                core: Core::privileged(directory.join("Atlas.Core.exe"), directory),
                desktop_pid: 0,
                session_id: None,
                network_epoch: 0,
                configured: false,
                cleanup_observer: None,
                guard: None,
                active_settings: None,
            },
            scheduler: Scheduler { health: Default::default(), queries: Default::default() },
            selection: Default::default(),
        })
    }
    fn tick(&mut self) {
        let session = &mut self.session;
        self.scheduler.health.tick(&mut session.core, &mut session.configured, &mut session.guard,
            &mut session.active_settings, &mut self.selection, &mut session.cleanup_observer,
            &mut self.scheduler.queries, &mut session.session_id, &mut session.network_epoch, session.desktop_pid);
    }
}
// Read-only: a healthy connection survives Wi-Fi roaming and DNS renewal.
// A failed path enters the existing protected, bounded recovery state machine.
fn selected_path_responds(client: &crate::core::ApiClient, selected: &str) -> Option<bool> {
    let name=if matches!(selected,"AUTO"|"FAILOVER") {
        match client.api("GET",&format!("/proxies/{selected}"),None) {
            Ok(value) => value["now"].as_str()?.to_owned(),
            Err(_)=>return None,
        }
    } else { selected.to_owned() };
    path_verdict(crate::latency::ENDPOINTS.iter().map(|endpoint|
        crate::latency::verified_probe(client,&name,endpoint)))
}

fn path_verdict(results: impl IntoIterator<Item=Result<Value,String>>) -> Option<bool> {
    let mut failures=0;
    let mut uncertain=false;
    for result in results {
        match result {
            Ok(_) => return Some(true),
            Err(error) if matches!(error.as_str(),"Mihomo API: HTTP 503"|"Mihomo API: HTTP 504")
                || error.contains("Контрольный URL не подтвердил") => failures+=1,
            Err(_) => uncertain=true,
        }
    }
    if failures>=2 && !uncertain { Some(false) } else { None }
}

fn confirm_route(client: &crate::core::ApiClient, settings: &Settings, controls: &[&str]) -> Result<Option<String>, String> {
    let proxies = client.api("GET", "/proxies", None)?;
    let name = if matches!(settings.selected.as_str(), "AUTO" | "FAILOVER") {
        proxies["proxies"][settings.selected.as_str()]["now"].as_str()
    } else { Some(settings.selected.as_str()) }
        .filter(|name| settings.servers().iter().any(|n|n["name"] == *name))
        .ok_or("Ядро не указало активный узел для проверки")?.to_owned();
    let report = crate::resilient_selection::verify_names(client, std::slice::from_ref(&name), controls);
    if report["candidate"] == name { return Ok(None); }
    client.event(json!({"kind":"route_confirmation_failed","at":crate::model::now(),"selected":name,"checks":report}));
    if !matches!(settings.selected.as_str(), "AUTO" | "FAILOVER") {
        return Err("Закреплённый сервер не подтвердил путь; подробные результаты сохранены в диагностике".into());
    }
    let blocked = settings.servers().iter().filter(|n|n["name"] == name)
        .map(crate::resilient_selection::node_key).collect();
    let alternatives = crate::resilient_selection::verify(client, settings, &blocked, controls);
    let candidate = alternatives["candidate"].as_str()
        .ok_or("Ни один из первых резервных узлов не подтвердил доступность; подключение остаётся в защищённой паузе")?;
    let group = settings.selected.as_str();
    client.api("PUT", &format!("/proxies/{group}"), Some(json!({"name":candidate})))?;
    if client.api("GET", &format!("/proxies/{group}"), None)?["now"] != candidate {
        return Err("Ядро не подтвердило выбор проверенного узла".into());
    }
    Ok(Some(candidate.to_owned()))
}
fn confirm_session(core: &mut Core, guard: &network_guard::Guard, settings: &Settings) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if crate::service::is_stopping() || core.cancelled() {
            return Err("Проверка подключения отменена".into());
        }
        match guard.install(&core.binary) {
            Ok(()) => break,
            Err(error) if !core.running() || Instant::now() >= deadline => return Err(error),
            Err(_) => thread::sleep(Duration::from_millis(100)),
        }
    }
    confirm_route(&core.client(), settings, &crate::latency::ENDPOINTS).map(|_| ())
}
fn abandon_session(core: &mut Core, guard: &mut Option<network_guard::Guard>,
    configured: &mut bool, active_settings: &mut Option<Settings>, reason: String) -> Result<Value, String> {
    // Capture BEFORE stop/cleanup invalidates the only source of the primary failure.
    let logs = core.client().logs().unwrap_or_default();
    let mut secrets = Vec::new();
    if let Some(settings) = active_settings.as_ref() {
        crate::support_report::collect_secrets(&serde_json::to_value(settings).unwrap_or(Value::Null), &mut secrets);
    }
    let evidence = json!({"at":crate::model::now(),"stage":"session_abandon_before_cleanup",
        "reason":reason,"logs":logs.iter().rev().take(256).collect::<Vec<_>>(),
        "failures":crate::support_report::failure_evidence(&logs)});
    let path = core.directory.join("last-session-failure.json");
    let safe = crate::support_report::redact_value(&evidence,&secrets);
    if let Err(error) = std::fs::write(&path,safe.to_string()) {
        core.client().event(json!({"kind":"failure_evidence_write_failed","error":error.to_string()}));
    }
    let paused = guard.as_ref().map(|policy| policy.pause(&core.binary)).unwrap_or(Ok(()));
    let stopped = core.stop();
    if stopped.is_ok() { *guard = None; }
    *configured = false;
    *active_settings = None;
    let dns = crate::session_cleanup::flush_dns();
    let mut errors = vec![reason];
    errors.extend([paused, stopped, dns].into_iter().filter_map(Result::err));
    Err(errors.join("; "))
}
fn run_channel(mut pipe: File, state: &mut Controller, desktop_owner_pid: u32) -> Result<bool, String> {
    let Controller { session: NetworkSession { core, desktop_pid, session_id, network_epoch, configured, cleanup_observer, guard, active_settings },
        scheduler: Scheduler { health, queries }, selection: recovery } = state;
    let session_running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let _watch = PipeWatch::start(&pipe, session_running.clone())?;
    if *desktop_pid != 0 && *desktop_pid != desktop_owner_pid {
        return Err("Сетевая сессия принадлежит другому процессу Atlas".into());
    }
    *desktop_pid = desktop_owner_pid;
    core.continue_running = Some(session_running.clone());
    // Watch the desktop before validation or any other potentially blocking
    // request, including cancellation while a connection is still starting.
    if cleanup_observer.is_none() { match crate::session_cleanup::start_observer(&core.directory, *desktop_pid) {
        Ok(observer) => *cleanup_observer = Some(observer),
        Err(error) => { let _ = send_startup_failure(&mut pipe, &error); return Err(error); }
    } }
    send(&mut pipe, &json!({"ready":true}))?;
    let mut explicit_stop = false;
    loop {
        if crate::service::is_stopping()
            || !session_running.load(std::sync::atomic::Ordering::SeqCst)
            || core.cancelled() {
            break;
        }
        health.tick(core, configured, guard, active_settings, recovery, cleanup_observer,
            queries, session_id, network_epoch, *desktop_pid);
        if active_settings.is_none() && !*configured
            && (guard.is_some() || health.attempts.len() >= 5) {
            break;
        }
        let mut available = 0;
        if unsafe {
            PeekNamedPipe(
                pipe.as_raw_handle(),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut available,
                std::ptr::null_mut(),
            )
        } == 0
        {
            break;
        }
        if available == 0 {
            thread::sleep(Duration::from_millis(30));
            continue;
        }
        let request = match receive(&mut pipe, Duration::from_secs(10)) {
            Ok(v) => v,
            Err(_) => break,
        };
        let op = request["op"].as_str().unwrap_or("");
        let payload = &request["payload"];
        let result: Result<Value, String> = (|| match op {
            "support_snapshot" => {
                let id = queries.start(false, |_| crate::support_report::privileged_snapshot().map(|text| json!({"text":text})))?;
                Ok(json!({"id":id}))
            }
            "delay" | "query" => {
                let path = payload["path"].as_str().ok_or("Нет пути")?;
                if (op == "delay" && !path.contains("/delay?")) || !allowed_api("GET", path) {
                    return Err("Проверка сервера не разрешена".into());
                }
                let client = core.client();
                let path = path.to_owned();
                let key = path.clone();
                let id = queries.start_keyed(path.contains("/delay?"), Some(key), move |cancelled|
                    client.api_cancellable("GET", &path, None, Some(&cancelled)))?;
                Ok(json!({"id":id}))
            }
            "delay_result" => {
                let id = payload["id"].as_str().ok_or("Нет проверки")?;
                match queries.poll(id)? {
                    Some(value) => Ok(json!({"done":true,"value":value})),
                    None => Ok(json!({"done":false})),
                }
            }
            "start" => {
                let _update_admission = crate::update_lock::UpdateLock::admit_session()?;
                let s: Settings = serde_json::from_value(payload.clone())
                    .map_err(|_| "Некорректные настройки")?;
                validate_settings(&s)?;
                let settings_match = active_settings.as_ref().is_some_and(|previous|
                    crate::config::same_network_config(previous, &s) && previous.selected == s.selected);
                match existing_session_action(*configured, core.running(), guard.is_some() && active_settings.is_some(), settings_match) {
                    ExistingSessionAction::Attach => return Ok(json!({"running":true,"reattached":true})),
                    ExistingSessionAction::WaitForRecovery => {
                        // This service owns the paused session and its guard.
                        // Let its health state machine finish recovery; treating
                        // its own TUN as a competing client would deadlock recovery.
                        return Ok(json!({"running":false,"recovering":true}));
                    }
                    ExistingSessionAction::Inconsistent => return Err(
                        "Служба Atlas потеряла ядро сессии без подтверждённой защиты; автоматический запуск остановлен".into()),
                    ExistingSessionAction::StartFresh => {}
                    ExistingSessionAction::Reconcile => {
                    let previous = active_settings.clone().ok_or("Нет активной конфигурации службы")?;
                    core.preflight_verified(&s, |probe|
                        confirm_route(&probe.client(), &s, &crate::latency::ENDPOINTS).map(|_| ()))?;
                    // The desktop may have restarted with newer persisted
                    // settings than the still-running service. Reconcile the
                    // existing core transactionally instead of rejecting the
                    // attach or starting a second TUN.
                    queries.invalidate();
                    *network_epoch = network_epoch.wrapping_add(1);
                    recovery.invalidate();
                    if let Err(error) = core.apply_verified(&s, |core, rollback| {
                        let policy = guard.as_ref().ok_or("Защита сети отсутствует при проверке конфигурации")?;
                        confirm_session(core, policy, if rollback { &previous } else { &s })
                    }) {
                        if !core.running() {
                            explicit_stop = true;
                            return abandon_session(core, guard, configured, active_settings,
                                format!("Не удалось восстановить существующую сессию: {error}"));
                        }
                        health.reset();
                        return Err(format!("Существующая сессия работает; новые настройки не применены: {error}"));
                    }
                    *active_settings = Some(s);
                    health.reset();
                    return Ok(json!({"running":true,"reattached":true,"configurationUpdated":true}));
                    }
                }
                queries.invalidate();
                network_guard::check_competing_routes()?;
                core.validate(&s)?;
                *network_epoch = network_epoch.wrapping_add(1);
                *session_id = Some(uuid::Uuid::new_v4());
                if cleanup_observer.is_none() {
                    match crate::session_cleanup::start_observer(&core.directory, *desktop_pid) {
                        Ok(observer) => *cleanup_observer = Some(observer),
                        Err(error) => {
                            *session_id = None;
                            explicit_stop = true;
                            return Err(error);
                        }
                    }
                }
                match network_guard::Guard::prepare(&core.binary) {
                    Ok(policy) => *guard = Some(policy),
                    Err(error) => {
                        *session_id = None;
                        explicit_stop = true;
                        return Err(error);
                    }
                }
                let started = core.start(&s);
                if let Err(e) = started {
                    explicit_stop = true;
                    return abandon_session(core, guard, configured, active_settings,
                        format!("Запуск ядра не завершён: {e}"));
                }
                let deadline = Instant::now() + Duration::from_secs(60);
                loop {
                    if crate::service::is_stopping() || core.cancelled()
                    {
                        explicit_stop = true;
                        return abandon_session(core, guard, configured, active_settings,
                            "Подключение отменено: Atlas завершает работу".into());
                    }
                    match guard.as_ref().unwrap().install(&core.binary) {
                        Ok(()) => break,
                        Err(e) if !core.running() || Instant::now() >= deadline => {
                            explicit_stop = true;
                            return abandon_session(core, guard, configured, active_settings,
                                format!("TUN не подтвердил готовность: {e}"));
                        }
                        Err(_) => thread::sleep(Duration::from_millis(100)),
                    }
                }
                if let Err(error) = confirm_route(&core.client(), &s, &crate::latency::ENDPOINTS) {
                        explicit_stop = true;
                        return abandon_session(core, guard, configured, active_settings,
                            format!("Путь через сервер не подтверждён: {error}"));
                }
                if !session_running.load(std::sync::atomic::Ordering::SeqCst) {
                    explicit_stop = true;
                    return abandon_session(core, guard, configured, active_settings,
                        "Подключение отменено".into());
                }
                *configured = true;
                health.reset();
                health.attempts.clear();
                recovery.invalidate();
                *active_settings = Some(s);
                Ok(json!({"running":true}))
            }
            "apply" => {
                let _update_admission = crate::update_lock::UpdateLock::admit_session()?;
                let s: Settings = serde_json::from_value(payload.clone())
                    .map_err(|_| "Некорректные настройки")?;
                validate_settings(&s)?;
                if !*configured { return Err("Нет активной сетевой сессии для обновления".into()); }
                let previous = active_settings.clone().ok_or("Нет предыдущей конфигурации для отката")?;
                // Health policy changes must not reload Mihomo or recreate TUN.
                if previous.selected == s.selected
                    && crate::config::generate(&previous, "compare")? == crate::config::generate(&s, "compare")? {
                    recovery.invalidate();
                    *active_settings = Some(s);
                    return Ok(json!({"running":true,"policyUpdated":true}));
                }
                core.preflight_verified(&s, |probe|
                    confirm_route(&probe.client(), &s, &crate::latency::ENDPOINTS).map(|_| ()))?;
                queries.invalidate();
                *network_epoch = network_epoch.wrapping_add(1);
                recovery.invalidate();
                if let Err(error) = core.apply_verified(&s, |core, rollback| {
                    let policy = guard.as_ref().ok_or("Защита сети отсутствует при проверке конфигурации")?;
                    confirm_session(core, policy, if rollback { &previous } else { &s })
                }) {
                    if !core.running() {
                        explicit_stop = true;
                        return abandon_session(core, guard, configured, active_settings,
                            format!("Обновление остановило ядро: {error}"));
                    }
                    health.reset();
                    return Err(format!("Обновление отклонено; прежняя конфигурация сохранена: {error}"));
                }
                *active_settings = Some(s);
                health.reset();
                Ok(json!({}))
            }
            "select" => {
                let name = payload["name"].as_str().ok_or("Нет сервера")?;
                if !*configured || !core.running() { return Err("Нет подтверждённой сетевой сессии".into()); }
                let settings = active_settings.as_mut().ok_or("Ядро не подключено")?;
                if !["AUTO", "FAILOVER"].contains(&name)
                    && !settings.servers().iter().any(|p| p["name"] == name)
                {
                    return Err("Сервер отсутствует в подписках".into());
                }
                let node_paths = settings.servers().iter().filter_map(|node| node["name"].as_str()
                    .map(|name| format!("/proxies/{}/delay", crate::latency::encode_name(name)))).collect();
                queries.selection_changed(&node_paths);
                *network_epoch = network_epoch.wrapping_add(1);
                recovery.invalidate();
                let previous = settings.selected.clone();
                if let Err(error) = core.select(name) {
                    let rollback = core.select(&previous);
                    if let Err(rollback_error) = rollback {
                        explicit_stop = true;
                        return abandon_session(core, guard, configured, active_settings,
                            format!("Команда выбора не подтверждена: {error}; прежний маршрут не восстановлен: {rollback_error}"));
                    }
                    return Err(format!("Команда выбора не подтверждена: {error}; прежний маршрут восстановлен"));
                }
                settings.selected = name.to_owned();
                // Selection acknowledges the local selector, not remote reachability.
                // The existing asynchronous health monitor measures the new node.
                // Waiting for URL tests here monopolizes the IPC channel and makes
                // unrelated node tests fail with "service busy". WFP is unchanged.
                Ok(json!({}))
            }
            "status" => {
                let running = core.running();
                Ok(json!({"running":running,"guard":*configured && running,
                    "adapter":guard.as_ref().map(|g|g.diagnostic()).unwrap_or_else(network_guard::tun_diagnostic),
                    "serviceProcessState":"Running","serviceChannelState":"Connected",
                    "sessionId":session_id.as_ref().map(ToString::to_string),"networkEpoch":*network_epoch,
                    "state":if *configured && running { "Connected" }
                        else if guard.is_some() { "ProtectedPause" } else { "Disconnected" }}))
            },
            "logs" => Ok(json!(core.client().logs()?)),
            "api" => {
                let method = payload["method"].as_str().ok_or("Нет метода")?;
                let path = payload["path"].as_str().ok_or("Нет пути")?;
                if !allowed_api(method, path) {
                    return Err("Операция ядра не разрешена".into());
                }
                core.api(method, path, None)
            }
            "stop" => {
                queries.invalidate();
                recovery.invalidate();
                *network_epoch = network_epoch.wrapping_add(1);
                *session_id = None;
                let paused = guard.as_ref().map(|policy| policy.pause(&core.binary)).unwrap_or(Ok(()));
                let stopped = core.stop();
                // Wintun can keep the Atlas adapter installed after its core
                // exits. Once the core is confirmed stopped, remove only
                // routes bound to that verified, inactive adapter so the
                // disconnected machine cannot keep sending traffic into it.
                let tun_routes = if stopped.is_ok() {
                    network_guard::clear_routes_for_reusable_tun().map(|removed| {
                        core.client().event(json!({"kind":"inactive_tun_routes_cleared","removed":removed}));
                    })
                } else { Ok(()) };
                // Keep the dynamic protection until the service's final
                // owned-process teardown if the core did not stop cleanly.
                if stopped.is_ok() { *guard = None; }
                *configured = false;
                *active_settings = None;
                explicit_stop = true;
                // Attempt every cleanup even if core termination failed.
                // DNS is flushed by the observer after the service exits.
                let filters = network_guard::clear();
                // DNS cleanup runs in the service-exit observer so the desktop
                // stop reply never waits on the Windows DNS client RPC.
                let errors: Vec<_> = [paused, stopped, tun_routes, filters].into_iter()
                    .filter_map(Result::err).collect();
                if !errors.is_empty() { return Err(errors.join("; ")); }
                let shutdown = core.client().logs().unwrap_or_default().into_iter()
                    .filter(|line| line.starts_with("ATLAS_EVENT ") && line.contains("core_shutdown_"))
                    .collect::<Vec<_>>();
                Ok(json!({"shutdownEvents":shutdown}))
            }
            _ => Err("Неизвестная команда сетевой службы".into()),
        })();
        let reply = match result {
            Ok(v) => json!({"result":v}),
            Err(e) => {
                let persisted=std::fs::read_to_string(core.directory.join("last-session-failure.json")).ok()
                    .and_then(|v|serde_json::from_str::<Value>(&v).ok());
                let logs=core.client().logs().unwrap_or_default();
                let evidence=json!({"at":crate::model::now(),"sessionId":session_id,"networkEpoch":network_epoch,
                    "configured":configured,"beforeCleanup":persisted,
                    "logs":logs.iter().rev().take(128).collect::<Vec<_>>()});
                json!({"error":e,"evidence":crate::support_report::redact_value(&evidence,&[])})
            },
        };
        if send(&mut pipe, &reply).is_err() {
            break;
        }
        if explicit_stop || (active_settings.is_some() && !*configured) {
            break;
        }
    }
    // A committed session survives transport loss for the bounded accept
    // window. Its authenticated desktop observer still controls its lifetime.
    queries.invalidate();
    core.continue_running = None;
    Ok(!explicit_stop && ((*configured && guard.is_some()) || (!*configured && guard.is_none())))
}

pub fn serve_service() -> Result<(), String> {
    let mut state = Controller::new()?;
    let mut channel_result = Ok(());
    while !crate::service::is_stopping() {
        state.tick();
        let sddl = wide("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;AU)");
        let mut descriptor = std::ptr::null_mut();
        unsafe {
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                std::ptr::null_mut(),
            ) == 0
            {
                return Err(error("Защита канала сетевой службы"));
            }
            let mut attributes = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor,
                bInheritHandle: 0,
            };
            let handle = CreateNamedPipeW(
                wide(SERVICE_PIPE).as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_NOWAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                65536,
                65536,
                0,
                &mut attributes,
            );
            LocalFree(descriptor);
            if handle == INVALID_HANDLE_VALUE {
                return Err(error("Не удалось открыть канал сетевой службы"));
            }
            let mut connected = false;
            let deadline = Instant::now() + Duration::from_secs(if state.session.configured { 30 } else { 3 });
            while !crate::service::is_stopping() {
                state.tick();
                if Instant::now() >= deadline {
                    break;
                }
                if ConnectNamedPipe(handle, std::ptr::null_mut()) != 0
                    || GetLastError() == ERROR_PIPE_CONNECTED
                {
                    connected = true;
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }
            if !connected {
                CloseHandle(handle);
                break;
            }
            let mut client_pid = 0;
            if GetNamedPipeClientProcessId(handle, &mut client_pid) == 0
                || verify_process_image(
                    client_pid,
                    &std::env::current_exe()
                        .map_err(|e| e.to_string())?
                        .with_file_name("Atlas.exe"),
                )
                .is_err()
            {
                DisconnectNamedPipe(handle);
                CloseHandle(handle);
                channel_result = Err("Канал сетевой службы отклонён: клиент не подтверждён".into());
                break;
            }
            let mode = PIPE_READMODE_BYTE | PIPE_NOWAIT;
            if SetNamedPipeHandleState(handle, &mode, std::ptr::null(), std::ptr::null()) == 0 {
                DisconnectNamedPipe(handle);
                CloseHandle(handle);
                channel_result = Err("Не удалось настроить канал сетевой службы".into());
                break;
            }
            let file = File::from_raw_handle(handle);
            match run_channel(file, &mut state, client_pid) {
                Ok(true) => continue,
                Ok(false) => channel_result = Ok(()),
                Err(error) => channel_result = Err(error),
            }
        }
        break;
    }
    let paused = state.session.guard.as_ref()
        .map(|guard| guard.pause(&state.session.core.binary)).unwrap_or(Ok(()));
    let core_result = state.session.core.stop();
    // The independent observer flushes DNS after this service process exits.
    let directory = state.session.core.directory.clone();
    drop(state);
    if directory.parent() == std::env::var_os("ProgramData").map(PathBuf::from).as_deref() {
        let _ = std::fs::remove_dir_all(&directory);
    }
    let errors: Vec<_> = [channel_result, paused, core_result].into_iter()
        .filter_map(Result::err).collect();
    if errors.is_empty() { Ok(()) } else { Err(errors.join("; ")) }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn channel_loss_with_live_process_requires_reconnect_not_process_restart() {
        assert_eq!(channel_recovery_action("Running"), ChannelRecoveryAction::Reconnect);
        assert_eq!(channel_recovery_action("Stopped"), ChannelRecoveryAction::ServiceStopped);
        assert_eq!(channel_recovery_action("Starting"), ChannelRecoveryAction::WaitForStartup);
        assert_eq!(channel_recovery_action("Unknown"), ChannelRecoveryAction::Unknown);
    }
    #[test]
    fn uplink_probe_checks_the_auto_selected_node_without_mutating_it() {
        use std::{io::{Read,Write},net::TcpListener};
        let listener=TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let client=crate::core::ApiClient::loopback_fixture(listener.local_addr().unwrap().port());
        let worker=thread::spawn(move || {
            let mut delays=0;
            for _ in 0..4 {
                let deadline=Instant::now()+Duration::from_secs(5);
                let mut stream=loop {
                    match listener.accept() {
                        Ok((stream,_))=>break stream,
                        Err(error) if error.kind()==std::io::ErrorKind::WouldBlock && Instant::now()<deadline=>thread::sleep(Duration::from_millis(5)),
                        Err(error)=>panic!("uplink fixture accept: {error}"),
                    }
                };
                // Winsock accepted sockets inherit the listener's nonblocking
                // mode. A read timeout does not clear it: an early read would
                // sporadically fail with WSAEWOULDBLOCK before the HTTP request.
                stream.set_nonblocking(false).unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut request=Vec::new(); let mut buffer=[0u8;1024];
                while !request.windows(4).any(|w|w==b"\r\n\r\n") {
                    let n=stream.read(&mut buffer).unwrap(); assert!(n>0); request.extend_from_slice(&buffer[..n]);
                }
                let request=String::from_utf8_lossy(&request);
                assert!(request.starts_with("GET "),"path validation must not change selection");
                let (status,body)=if request.starts_with("GET /proxies/AUTO ") { (200,json!({"now":"fixture"})) }
                    else if request.starts_with("GET /proxies/fixture/delay?") {
                        delays+=1;
                        if delays==1 { (504,json!({"message":"timeout"})) } else { (200,json!({"delay":30})) }
                    } else {
                        assert!(request.starts_with("GET /proxies "));
                        (200,json!({"proxies":{"fixture":{"extra":{crate::latency::SECONDARY_URL:{"alive":true}}}}}))
                    };
                let body=body.to_string();
                write!(stream,"HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
            }
        });
        assert_eq!(selected_path_responds(&client,"AUTO"),Some(true));
        worker.join().unwrap();
    }
    #[test]
    fn uplink_check_requires_two_remote_failures_and_preserves_a_working_control() {
        let timeout=||Err("Mihomo API: HTTP 504".to_owned());
        assert_eq!(path_verdict([timeout(),timeout()]),Some(false));
        assert_eq!(path_verdict([timeout(),Ok(json!({"delay":30}))]),Some(true));
        assert_eq!(path_verdict([Ok(json!({"delay":30})),timeout()]),Some(true));
        assert_eq!(path_verdict([timeout(),Err("Сетевая служба занята. Повторите операцию.".into())]),None);
        assert_eq!(path_verdict([Err("Mihomo API: HTTP 401".into()),timeout()]),None);
        assert_eq!(path_verdict([timeout()]),None);
    }
    #[test]
    fn observer_startup_error_reaches_desktop_before_pipe_closes() {
        let (mut server, mut client) = pipe_pair();
        let message = "Наблюдатель очистки Atlas: отказ проверки владельца";
        let worker = thread::spawn(move || send_startup_failure(&mut server, message));
        assert_eq!(receive_service_ready(&mut client, None).unwrap_err(), message);
        worker.join().unwrap().unwrap();
    }
    #[test]
    fn reconnect_reattaches_reconciles_or_waits_without_treating_own_tun_as_competing() {
        assert_eq!(existing_session_action(false, false, false, false), ExistingSessionAction::StartFresh);
        assert_eq!(existing_session_action(true, true, true, true), ExistingSessionAction::Attach);
        assert_eq!(existing_session_action(true, true, true, false), ExistingSessionAction::Reconcile);
        assert_eq!(existing_session_action(true, false, true, false), ExistingSessionAction::WaitForRecovery);
        assert_eq!(existing_session_action(true, false, false, false), ExistingSessionAction::Inconsistent);
    }

    #[test]
    fn cancel_interrupts_start_reply_and_reaches_service_without_another_command() {
        let (server, client) = pipe_pair();
        let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let _watch = PipeWatch::start(&server, running.clone()).unwrap();
        let broker = std::sync::Arc::new(Broker { pipe: Mutex::new(Some(client)) });
        let cancellation = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let worker_broker = broker.clone();
        let worker_cancel = cancellation.clone();
        let worker = thread::spawn(move || worker_broker.call_cancellable("start", Value::Null, Some(&worker_cancel)));
        let mut server = server;
        assert_eq!(receive(&mut server, Duration::from_secs(2)).unwrap()["op"], "start");
        let started = Instant::now();
        cancellation.store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(worker.join().unwrap().is_err());
        while running.load(std::sync::atomic::Ordering::SeqCst) {
            assert!(started.elapsed() < Duration::from_secs(2));
            thread::sleep(Duration::from_millis(5));
        }
        assert!(!broker.alive());
    }
    fn pipe_pair() -> (File, File) {
        let name = wide(&format!(r"\\.\pipe\atlas-test-{}", uuid::Uuid::new_v4()));
        unsafe {
            let handle = CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_NOWAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                4096,
                4096,
                0,
                std::ptr::null(),
            );
            assert_ne!(handle, INVALID_HANDLE_VALUE);
            let server = File::from_raw_handle(handle);
            let client = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(String::from_utf16_lossy(&name[..name.len() - 1]))
                .unwrap();
            assert!(
                ConnectNamedPipe(handle, std::ptr::null_mut()) != 0
                    || GetLastError() == ERROR_PIPE_CONNECTED
            );
            nonblocking(&client).unwrap();
            (server, client)
        }
    }
    #[test]
    fn stalled_peer_cannot_block_writes_in_either_direction() {
        for server_writes in [true, false] {
            let (mut server, mut client) = pipe_pair();
            let writer = if server_writes {
                &mut server
            } else {
                &mut client
            };
            let start = Instant::now();
            let result = write_bytes(
                writer,
                &vec![42; 1024 * 1024],
                start + Duration::from_millis(100),
            );
            assert!(result.is_err());
            assert!(start.elapsed() < Duration::from_secs(2));
        }
    }
    #[test]
    fn frames_larger_than_pipe_buffer_survive_partial_writes() {
        let (mut server, mut client) = pipe_pair();
        let value = json!({"data":"x".repeat(256 * 1024)});
        let expected = value.clone();
        let worker = thread::spawn(move || {
            let got = receive(&mut server, Duration::from_secs(5)).unwrap();
            send(&mut server, &got).unwrap();
        });
        send(&mut client, &value).unwrap();
        assert_eq!(
            receive(&mut client, Duration::from_secs(5)).unwrap(),
            expected
        );
        worker.join().unwrap();
    }
    #[test]
    fn concurrent_commands_keep_their_own_replies() {
        let (mut server, client) = pipe_pair();
        let worker = thread::spawn(move || {
            for _ in 0..32 {
                let command = receive(&mut server, Duration::from_secs(5)).unwrap();
                send(&mut server, &json!({"result":command["payload"]})).unwrap();
            }
        });
        let broker = std::sync::Arc::new(Broker {
            pipe: Mutex::new(Some(client)),
        });
        let workers: Vec<_> = (0..8)
            .map(|i| {
                let broker = broker.clone();
                thread::spawn(move || {
                    for j in 0..4 {
                        let payload = json!({"client":i,"request":j});
                        assert_eq!(broker.call("status", payload.clone()).unwrap(), payload);
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        worker.join().unwrap();
    }
    #[test]
    fn api_is_narrow() {
        assert!(!allowed_api("PUT", "/configs"));
        assert!(!allowed_api(
            "GET",
            "/proxies/x/delay?url=https://evil.example"
        ));
        assert!(allowed_api(
            "GET",
            "/proxies/x/delay?timeout=8000&url=http%3A%2F%2Fcp.cloudflare.com%2Fgenerate_204"
        ));
    }
    #[test]
    fn file_capabilities_rejected() {
        assert!(reject_file_keys(&json!({"private-key-path":"C:/secret"})).is_err());
        assert!(reject_file_keys(&json!({"ws-opts":{"path":"/ws"}})).is_ok());
    }
}
#[test]
fn busy_channel_is_not_reported_as_a_crashed_core() {
    let broker = Broker {
        pipe: Mutex::new(None),
    };
    let lock = broker.pipe.lock().unwrap();
    assert!(broker.alive());
    drop(lock);
    assert!(!broker.alive());
    assert!(broker.call("status", Value::Null).is_err());
}

#[derive(Debug, PartialEq)]
enum TunGuardAction {
    Keep,
    Unowned,
    Missing,
}
fn tun_guard_action(previous: Option<u64>, current: Option<u64>) -> TunGuardAction {
    match current {
        None => TunGuardAction::Missing,
        Some(_) if previous == current => TunGuardAction::Keep,
        Some(_) => TunGuardAction::Unowned,
    }
}
#[cfg(test)]
mod tun_guard_tests {
    use super::*;
    #[test]
    fn unchanged_adapter_preserves_the_session() {
        assert_eq!(tun_guard_action(Some(7), Some(7)), TunGuardAction::Keep);
    }
    #[test]
    fn replacement_adapter_is_not_trusted_as_the_original_session() {
        assert_eq!(tun_guard_action(Some(7), Some(8)), TunGuardAction::Unowned);
    }
    #[test]
    fn missing_adapter_is_not_treated_as_healthy() {
        assert_eq!(tun_guard_action(Some(7), None), TunGuardAction::Missing);
        assert_eq!(tun_guard_action(None, None), TunGuardAction::Missing);
    }
}

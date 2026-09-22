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
const EMBEDDED_CORE: &[u8] = include_bytes!("../resources/mihomo.exe");
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
impl Broker {
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
                let hello = receive_cancellable(&mut file, Duration::from_secs(10), running)?;
                if hello["ready"] != true {
                    return Err("Сетевая служба не готова".into());
                }
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
        let file = slot.as_mut().ok_or("Канал сетевой службы закрыт")?;
        let reply = (|| {
            send_cancellable(file, &json!({"op":op,"payload":payload}), running)?;
            // Starting can include driver installation; its IPC deadline must outlive readiness.
            receive_cancellable(
                file,
                Duration::from_secs(if matches!(op, "start" | "apply") {
                    120
                } else {
                    15
                }),
                running,
            )
        })();
        let reply = match reply {
            Ok(reply) => reply,
            Err(error) => {
                // A late reply must never be mistaken for the next command's reply.
                // Closing the session also lets the service clean up its TUN/WFP.
                *slot = None;
                return Err(error);
            }
        };
        if let Some(e) = reply["error"].as_str() {
            Err(e.into())
        } else {
            Ok(reply["result"].clone())
        }
    }
}
fn verify_process_image(pid: u32, expected: &std::path::Path) -> Result<(), String> {
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
        std::fs::write(directory.join("mihomo.exe"), EMBEDDED_CORE).map_err(|e| e.to_string())?;
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
                "url" => [
                    crate::latency::DISPLAY_URL,
                    "https://www.gstatic.com/generate_204",
                    "https://cp.cloudflare.com/generate_204",
                ]
                .contains(&v.as_ref()),
                "timeout" => v.parse::<u64>().is_ok_and(|n| n <= 10000),
                "expected" => v == "204",
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
fn run_channel(mut pipe: File, parent_handle: Option<HANDLE>) -> Result<(), String> {
    let directory = secure_directory()?;
    let mut core = Core::privileged(directory.join("mihomo.exe"), directory.clone());
    let session_running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let _watch = PipeWatch::start(&pipe, session_running.clone())?;
    core.continue_running = Some(session_running);
    send(&mut pipe, &json!({"ready":true}))?;
    let mut configured = false;
    let mut cleanup_observer: Option<std::process::Child> = None;
    let mut guard: Option<network_guard::Guard> = None;
    let mut explicit_stop = false;
    let mut active_settings: Option<Settings> = None;
    let mut recovery = crate::resilient_selection::Recovery::default();
    let mut last_recovery_scan = Instant::now();
    let mut last_health = Instant::now();
    let mut health_failures = 0;
    let mut health_probe = crate::background_probe::Probe::default();
    let mut tun_identity = network_guard::tun_identity();
    let mut queries = crate::query_jobs::QueryJobs::default();
    loop {
        if configured && last_recovery_scan.elapsed() >= Duration::from_millis(500) {
            last_recovery_scan = Instant::now();
            if let Some(settings) = &active_settings { recovery.tick(core.client(), settings); }
        }
        if crate::service::is_stopping() || core.cancelled() {
            break;
        }
        if let Some(parent) = parent_handle {
            if unsafe { WaitForSingleObject(parent, 0) } == WAIT_OBJECT_0 {
                break;
            }
        }
        if configured {
            if let Some(healthy) = health_probe.poll() {
                health_failures = if healthy { 0 } else { health_failures + 1 };
                if health_failures >= 3 { break; }
            }
        }
        if configured && last_health.elapsed() >= Duration::from_secs(5) {
            if cleanup_observer
                .as_mut()
                .is_none_or(|child| !matches!(child.try_wait(), Ok(None)))
            {
                break;
            }
            // Mihomo v1.19.31 already subscribes to Windows route/interface
            // notifications and resets resolver connections. Do not reload TUN
            // when DHCP, sleep or the physical egress changes.
            let observed = network_guard::tun_identity();
            let guard_ready = match tun_guard_action(tun_identity, observed) {
                TunGuardAction::Keep => true,
                TunGuardAction::Missing => false,
                TunGuardAction::Rebind => {
                    let ready = guard
                        .as_ref()
                        .is_some_and(|g| g.install(&core.binary).is_ok());
                    if ready {
                        tun_identity = observed;
                    }
                    ready
                }
            };
            if !core.running() {
                break;
            }
            let client = core.client();
            health_probe.start(move || guard_ready && client.api("GET", "/version", None).is_ok());
            last_health = Instant::now();
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
            if configured && !core.running() {
                break;
            }
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
                let id = queries.start(false, || crate::support_report::privileged_snapshot().map(|text| json!({"text":text})))?;
                Ok(json!({"id":id}))
            }
            "delay" | "query" => {
                let path = payload["path"].as_str().ok_or("Нет пути")?;
                if (op == "delay" && !path.contains("/delay?")) || !allowed_api("GET", path) {
                    return Err("Проверка сервера не разрешена".into());
                }
                let client = core.client();
                let path = path.to_owned();
                let id = queries.start(path.contains("/delay?"), move || client.api("GET", &path, None))?;
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
                let s: Settings = serde_json::from_value(payload.clone())
                    .map_err(|_| "Некорректные настройки")?;
                validate_settings(&s)?;
                network_guard::check_competing_routes()?;
                core.validate(&s)?;
                if cleanup_observer.is_none() {
                    cleanup_observer = Some(crate::session_cleanup::start_observer()?);
                }
                guard = Some(network_guard::Guard::prepare(&core.binary)?);
                if let Err(e) = core.start(&s) {
                    let _ = core.stop();
                    guard = None;
                    return Err(e);
                }
                let deadline = Instant::now() + Duration::from_secs(60);
                loop {
                    if crate::service::is_stopping() || core.cancelled()
                        || parent_handle.is_some_and(
                            |parent| unsafe { WaitForSingleObject(parent, 0) } == WAIT_OBJECT_0,
                        )
                    {
                        let _ = core.stop();
                        guard = None;
                        return Err("Подключение отменено: Atlas завершает работу".into());
                    }
                    match guard.as_ref().unwrap().install(&core.binary) {
                        Ok(()) => break,
                        Err(e) if !core.running() || Instant::now() >= deadline => {
                            let logs = core.client().logs().unwrap_or_default();
                            let _ = core.stop();
                            guard = None;
                            return Err(format!("{e}. Ядро: {logs:?}"));
                        }
                        Err(_) => thread::sleep(Duration::from_millis(100)),
                    }
                }
                configured = true;
                health_probe = Default::default();
                health_failures = 0;
                recovery.invalidate();
                active_settings = Some(s);
                last_health = Instant::now();
                tun_identity = network_guard::tun_identity();
                Ok(json!({"running":true}))
            }
            "apply" => {
                let s: Settings = serde_json::from_value(payload.clone())
                    .map_err(|_| "Некорректные настройки")?;
                validate_settings(&s)?;
                core.apply(&s)?;
                // API success alone is not proof of a working TUN adapter.
                // Apply the same adapter-readiness check used for initial startup.
                let deadline = Instant::now() + Duration::from_secs(60);
                while let Some(policy) = &guard {
                    match policy.install(&core.binary) {
                        Ok(()) => break,
                        Err(error) if !core.running() || Instant::now() >= deadline => {
                            let logs = core.client().logs().unwrap_or_default();
                            let _ = core.stop();
                            guard = None;
                            configured = false;
                            return Err(format!("{error}. Ядро: {logs:?}"));
                        }
                        Err(_) => thread::sleep(Duration::from_millis(100)),
                    }
                    if crate::service::is_stopping() || core.cancelled()
                        || parent_handle.is_some_and(
                            |parent| unsafe { WaitForSingleObject(parent, 0) } == WAIT_OBJECT_0,
                        )
                    {
                        let _ = core.stop();
                        guard = None;
                        configured = false;
                        return Err("Обновление подключения отменено".into());
                    }
                }
                recovery.invalidate();
                active_settings = Some(s);
                tun_identity = network_guard::tun_identity();
                health_probe = Default::default();
                health_failures = 0;
                last_health = Instant::now();
                Ok(json!({}))
            }
            "select" => {
                let name = payload["name"].as_str().ok_or("Нет сервера")?;
                let settings = active_settings.as_mut().ok_or("Ядро не подключено")?;
                if !["AUTO", "FAILOVER"].contains(&name)
                    && !settings.servers().iter().any(|p| p["name"] == name)
                {
                    return Err("Сервер отсутствует в подписках".into());
                }
                recovery.invalidate();
                core.select(name)?;
                settings.selected = name.to_owned();
                Ok(json!({}))
            }
            "status" => Ok(json!({"running":core.running(),"guard":configured})),
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
                let stopped = core.stop();
                guard = None;
                explicit_stop = true;
                // Attempt every cleanup even if core termination failed. Never
                // acknowledge Stop before releasing filters and cached fake IPs.
                let filters = network_guard::clear();
                let dns = crate::session_cleanup::flush_dns();
                let errors: Vec<_> = [stopped, filters, dns].into_iter()
                    .filter_map(Result::err).collect();
                if !errors.is_empty() { return Err(errors.join("; ")); }
                Ok(json!({}))
            }
            _ => Err("Неизвестная команда сетевой службы".into()),
        })();
        let reply = match result {
            Ok(v) => json!({"result":v}),
            Err(e) => json!({"error":e}),
        };
        if send(&mut pipe, &reply).is_err() {
            break;
        }
        if explicit_stop || (active_settings.is_some() && !configured) {
            break;
        }
    }
    let stopped = core.stop();
    drop(core);
    drop(guard);
    let dns_cleanup = if cleanup_observer.is_some() {
        crate::session_cleanup::flush_dns()
    } else {
        Ok(())
    };
    if let Some(parent) = parent_handle {
        unsafe {
            CloseHandle(parent);
        }
    } // Dynamic WFP session is released after the core closes its adapter.
      // Directory was generated here with an administrator-only DACL, never supplied by IPC.
    if directory.parent()
        == std::env::var_os("ProgramData")
            .map(PathBuf::from)
            .as_deref()
    {
        let _ = std::fs::remove_dir_all(&directory);
    }
    stopped.and(dns_cleanup)
}

pub fn serve_service() -> Result<(), String> {
    while !crate::service::is_stopping() {
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
            let deadline = Instant::now() + Duration::from_secs(30);
            while !crate::service::is_stopping() {
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
                    &std::env::current_exe().map_err(|e| e.to_string())?,
                )
                .is_err()
            {
                DisconnectNamedPipe(handle);
                CloseHandle(handle);
                continue;
            }
            let mode = PIPE_READMODE_BYTE | PIPE_NOWAIT;
            if SetNamedPipeHandleState(handle, &mode, std::ptr::null(), std::ptr::null()) == 0 {
                DisconnectNamedPipe(handle);
                CloseHandle(handle);
                continue;
            }
            let file = File::from_raw_handle(handle);
            let parent = OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE,
                0,
                client_pid,
            );
            if parent.is_null() {
                break;
            }
            let _watch = match crate::service::watch_session(client_pid) {
                Ok(watch) => watch,
                Err(error) => {
                    CloseHandle(parent);
                    return Err(error);
                }
            };
            let _ = run_channel(file, Some(parent));
            // A service session belongs to a single Atlas connection, not the OS lifetime.
            break;
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
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
            "/proxies/x/delay?timeout=8000&url=https%3A%2F%2Fcp.cloudflare.com%2Fgenerate_204"
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
    Rebind,
    Missing,
}
fn tun_guard_action(previous: Option<u64>, current: Option<u64>) -> TunGuardAction {
    match current {
        None => TunGuardAction::Missing,
        Some(_) if previous == current => TunGuardAction::Keep,
        Some(_) => TunGuardAction::Rebind,
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
    fn replacement_adapter_requires_filter_rebinding_only() {
        assert_eq!(tun_guard_action(Some(7), Some(8)), TunGuardAction::Rebind);
    }
    #[test]
    fn missing_adapter_is_not_treated_as_healthy() {
        assert_eq!(tun_guard_action(Some(7), None), TunGuardAction::Missing);
        assert_eq!(tun_guard_action(None, None), TunGuardAction::Missing);
    }
}

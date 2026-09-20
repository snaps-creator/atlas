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
fn send(file: &mut File, value: &Value) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|_| "Ошибка кодирования команды")?;
    if bytes.len() > LIMIT {
        return Err("Команда превышает 16 МБ".into());
    }
    file.write_all(&(bytes.len() as u32).to_le_bytes())
        .and_then(|_| file.write_all(&bytes))
        .map_err(|_| "Канал сетевой службы закрыт".into())
}
fn read_bytes(file: &mut File, buffer: &mut [u8], deadline: Instant) -> Result<(), String> {
    let mut offset = 0;
    while offset < buffer.len() {
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
    let deadline = Instant::now() + timeout;
    let mut len = [0; 4];
    read_bytes(file, &mut len, deadline)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > LIMIT {
        return Err("Ответ сетевой службы превышает лимит".into());
    }
    let mut data = vec![0; len];
    read_bytes(file, &mut data, deadline)?;
    serde_json::from_slice(&data).map_err(|_| "Некорректный ответ сетевой службы".into())
}
pub struct Broker {
    pipe: Mutex<File>,
}
impl Broker {
    pub fn launch() -> Result<Self, String> {
        Self::connect_service()
    }
    fn connect_service() -> Result<Self, String> {
        // A cold Windows service start can be delayed by signature/AV checks.
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
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
                let hello = receive(&mut file, Duration::from_secs(10))?;
                if hello["ready"] != true {
                    return Err("Сетевая служба не готова".into());
                }
                return Ok(Self {
                    pipe: Mutex::new(file),
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
        let Ok(file) = self.pipe.lock() else {
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
        let mut file = self
            .pipe
            .lock()
            .map_err(|_| "Ошибка канала сетевой службы")?;
        send(&mut file, &json!({"op":op,"payload":payload}))?;
        // Starting can include driver installation; its IPC deadline must outlive readiness.
        let reply = receive(
            &mut file,
            Duration::from_secs(if matches!(op, "start" | "apply") {
                120
            } else {
                45
            }),
        )?;
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
    if method == "GET" && ["/version", "/connections", "/proxies"].contains(&path) {
        return true;
    }
    if method == "DELETE" && path.starts_with("/connections/") && !path.contains(['?', '#']) {
        return true;
    }
    if method == "GET" && path.starts_with("/proxies/") && path.contains("/delay?") {
        if let Ok(url) = url::Url::parse(&format!("http://localhost{path}")) {
            return url.query_pairs().all(|(k, v)| match k.as_ref() {
                "url" => [
                    "https://www.gstatic.com/generate_204",
                    "https://cp.cloudflare.com/generate_204",
                ]
                .contains(&v.as_ref()),
                "timeout" => v.parse::<u64>().is_ok_and(|n| n <= 10000),
                _ => false,
            });
        }
    }
    false
}
fn run_channel(mut pipe: File, parent_handle: Option<HANDLE>) -> Result<(), String> {
    let directory = secure_directory()?;
    let mut core = Core::privileged(directory.join("mihomo.exe"), directory.clone());
    send(&mut pipe, &json!({"ready":true}))?;
    let mut configured = false;
    let mut guard: Option<network_guard::Guard> = None;
    let mut explicit_stop = false;
    loop {
        if crate::service::is_stopping() {
            break;
        }
        if let Some(parent) = parent_handle {
            if unsafe { WaitForSingleObject(parent, 0) } == WAIT_OBJECT_0 {
                break;
            }
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
            "start" => {
                let s: Settings = serde_json::from_value(payload.clone())
                    .map_err(|_| "Некорректные настройки")?;
                validate_settings(&s)?;
                network_guard::check_competing_routes()?;
                core.validate(&s)?;
                guard = Some(network_guard::Guard::prepare(&core.binary)?);
                if let Err(e) = core.start(&s) {
                    let _ = core.stop();
                    guard = None;
                    return Err(e);
                }
                let deadline = Instant::now() + Duration::from_secs(60);
                loop {
                    if crate::service::is_stopping()
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
                    if crate::service::is_stopping()
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
                stopped?;
                network_guard::clear()?;
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
        if explicit_stop {
            break;
        }
    }
    let stopped = core.stop();
    drop(core);
    drop(guard);
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
    stopped
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
            let mode = PIPE_READMODE_BYTE | PIPE_WAIT;
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

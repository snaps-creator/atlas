//! Per-session elevated broker. The UI owns a local-only named pipe; both ends
//! check peer PIDs. No shell, arbitrary executable, arbitrary YAML or file API.
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
    UI::{Shell::*, WindowsAndMessaging::SW_HIDE},
};
const LIMIT: usize = 16 * 1024 * 1024;
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
    process: isize,
}
impl Broker {
    pub fn launch() -> Result<Self, String> {
        unsafe {
            let pipe_name = format!(r"\\.\pipe\Atlas-{}", uuid::Uuid::new_v4().simple());
            let pipe = CreateNamedPipeW(
                wide(&pipe_name).as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_NOWAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                65536,
                65536,
                0,
                std::ptr::null(),
            );
            if pipe == INVALID_HANDLE_VALUE {
                return Err(error("Не удалось создать канал сетевой службы"));
            }
            let mut file = File::from_raw_handle(pipe);
            let exe = wide(
                &std::env::current_exe()
                    .map_err(|e| e.to_string())?
                    .to_string_lossy(),
            );
            let verb = wide("runas");
            let args = wide(&format!(
                "--network-helper {} {}",
                std::process::id(),
                pipe_name
            ));
            let mut launch: SHELLEXECUTEINFOW = std::mem::zeroed();
            launch.cbSize = std::mem::size_of_val(&launch) as u32;
            launch.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
            launch.lpVerb = verb.as_ptr();
            launch.lpFile = exe.as_ptr();
            launch.lpParameters = args.as_ptr();
            launch.nShow = SW_HIDE;
            if ShellExecuteExW(&mut launch) == 0 {
                return Err(error("Запуск сетевой службы отменён или запрещён"));
            }
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                let connected = ConnectNamedPipe(pipe, std::ptr::null_mut()) != 0
                    || GetLastError() == ERROR_PIPE_CONNECTED;
                if connected {
                    break;
                }
                if Instant::now() > deadline
                    || WaitForSingleObject(launch.hProcess, 0) == WAIT_OBJECT_0
                {
                    CloseHandle(launch.hProcess);
                    return Err("Сетевая служба не подключилась".into());
                }
                thread::sleep(Duration::from_millis(20));
            }
            let mut peer = 0;
            if GetNamedPipeClientProcessId(pipe, &mut peer) == 0
                || peer != GetProcessId(launch.hProcess)
            {
                CloseHandle(launch.hProcess);
                return Err("Не удалось подтвердить сетевую службу".into());
            }
            let hello = receive(&mut file, Duration::from_secs(10))?;
            if hello["ready"] != true {
                CloseHandle(launch.hProcess);
                return Err("Сетевая служба не готова".into());
            }
            let mode = PIPE_READMODE_BYTE | PIPE_WAIT;
            if SetNamedPipeHandleState(pipe, &mode, std::ptr::null(), std::ptr::null()) == 0 {
                CloseHandle(launch.hProcess);
                return Err(error("Не удалось настроить канал сетевой службы"));
            }
            Ok(Self {
                pipe: Mutex::new(file),
                process: launch.hProcess as isize,
            })
        }
    }
    pub fn alive(&self) -> bool {
        unsafe { WaitForSingleObject(self.process as _, 0) == WAIT_TIMEOUT }
    }
    pub fn call(&self, op: &str, payload: Value) -> Result<Value, String> {
        let mut file = self
            .pipe
            .lock()
            .map_err(|_| "Ошибка канала сетевой службы")?;
        send(&mut file, &json!({"op":op,"payload":payload}))?;
        let reply = receive(&mut file, Duration::from_secs(45))?;
        if let Some(e) = reply["error"].as_str() {
            Err(e.into())
        } else {
            Ok(reply["result"].clone())
        }
    }
}
impl Drop for Broker {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.process as _);
        }
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
pub fn serve(parent: u32, pipe_name: &str) -> Result<(), String> {
    if !pipe_name.starts_with(r"\\.\pipe\Atlas-") || pipe_name.len() > 100 {
        return Err("Некорректный канал".into());
    }
    let parent_handle =
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, 0, parent) };
    if parent_handle.is_null() {
        return Err("Родительский процесс недоступен".into());
    }
    let mut parent_path = vec![0u16; 32768];
    let mut size = parent_path.len() as u32;
    unsafe {
        if QueryFullProcessImageNameW(parent_handle, 0, parent_path.as_mut_ptr(), &mut size) == 0 {
            CloseHandle(parent_handle);
            return Err("Нельзя проверить родительский процесс".into());
        }
    }
    let parent_exe = PathBuf::from(String::from_utf16_lossy(&parent_path[..size as usize]));
    if parent_exe.canonicalize().ok()
        != std::env::current_exe()
            .ok()
            .and_then(|p| p.canonicalize().ok())
    {
        unsafe {
            CloseHandle(parent_handle);
        }
        return Err("Родительский процесс не является Атласом".into());
    }
    let mut pipe = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(pipe_name)
        .map_err(|_| "Канал Атласа недоступен")?;
    let mut server_pid = 0;
    unsafe {
        if GetNamedPipeServerProcessId(pipe.as_raw_handle(), &mut server_pid) == 0
            || server_pid != parent
        {
            CloseHandle(parent_handle);
            return Err("Не удалось подтвердить интерфейс Атласа".into());
        }
    }
    let directory = secure_directory()?;
    let mut core = Core::privileged(directory.join("mihomo.exe"), directory.clone());
    send(&mut pipe, &json!({"ready":true}))?;
    let mut configured = false;
    let mut guard: Option<network_guard::Guard> = None;
    let mut explicit_stop = false;
    loop {
        if unsafe { WaitForSingleObject(parent_handle, 0) } == WAIT_OBJECT_0 {
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
                core.validate(&s)?;
                guard = Some(network_guard::Guard::prepare(&core.binary)?);
                if let Err(e) = core.start(&s) {
                    let _ = core.stop();
                    guard = None;
                    return Err(e);
                }
                let deadline = Instant::now() + Duration::from_secs(10);
                loop {
                    match guard.as_ref().unwrap().install(&core.binary) {
                        Ok(()) => break,
                        Err(e) if Instant::now() >= deadline => {
                            let _ = core.stop();
                            guard = None;
                            return Err(e);
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
                if let Some(policy) = &guard {
                    if let Err(error) = policy.install(&core.binary) {
                        let _ = core.stop();
                        guard = None;
                        configured = false;
                        return Err(error);
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
                core.stop()?;
                guard = None;
                network_guard::clear()?;
                explicit_stop = true;
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
    core.stop()?;
    drop(core);
    drop(guard);
    unsafe {
        CloseHandle(parent_handle);
    } // Dynamic WFP session is released after the core closes its adapter.
      // Directory was generated here with an administrator-only DACL, never supplied by IPC.
    if directory.parent()
        == std::env::var_os("ProgramData")
            .map(PathBuf::from)
            .as_deref()
    {
        let _ = std::fs::remove_dir_all(&directory);
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

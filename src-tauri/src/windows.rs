use serde::{Deserialize, Serialize};
use std::path::Path;
use winreg::{enums::*, RegKey};
const INTERNET: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings";
#[derive(Serialize, Deserialize)]
struct Snapshot {
    enabled: Option<u32>,
    server: Option<String>,
    bypass: Option<String>,
    pac: Option<String>,
}
fn key() -> Result<RegKey, String> {
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(INTERNET, KEY_READ | KEY_WRITE)
        .map_err(|e| e.to_string())
}
fn notify() {
    unsafe {
        use windows_sys::Win32::Networking::WinInet::*;
        InternetSetOptionW(
            std::ptr::null_mut(),
            INTERNET_OPTION_SETTINGS_CHANGED,
            std::ptr::null_mut(),
            0,
        );
        InternetSetOptionW(
            std::ptr::null_mut(),
            INTERNET_OPTION_REFRESH,
            std::ptr::null_mut(),
            0,
        );
    }
}
pub fn enable(path: &Path) -> Result<(), String> {
    let k = key()?;
    if path.exists() {
        return Err("Есть незавершённое восстановление системного прокси".into());
    }
    let s = Snapshot {
        enabled: k.get_value("ProxyEnable").ok(),
        server: k.get_value("ProxyServer").ok(),
        bypass: k.get_value("ProxyOverride").ok(),
        pac: k.get_value("AutoConfigURL").ok(),
    };
    std::fs::write(path, serde_json::to_vec(&s).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let result = (|| {
        k.set_value("ProxyServer", &"127.0.0.1:17890")
            .map_err(|e| e.to_string())?;
        k.set_value("ProxyOverride", &"<local>;localhost;127.*;[::1]")
            .map_err(|e| e.to_string())?;
        if s.pac.is_some() {
            k.delete_value("AutoConfigURL").map_err(|e| e.to_string())?
        }
        k.set_value("ProxyEnable", &1u32)
            .map_err(|e| e.to_string())?;
        notify();
        Ok(())
    })();
    if result.is_err() {
        let _ = restore(path);
    }
    result
}
pub fn restore(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    let s: Snapshot = serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|_| "Повреждена копия системного прокси")?;
    let k = key()?;
    // Do not overwrite a proxy subsequently selected by another application.
    let current: String = k.get_value("ProxyServer").unwrap_or_default();
    if current != "127.0.0.1:17890" {
        std::fs::remove_file(path).map_err(|e| e.to_string())?;
        return Ok(());
    }
    match s.enabled {
        Some(v) => k.set_value("ProxyEnable", &v).map_err(|e| e.to_string())?,
        None => {
            let _ = k.delete_value("ProxyEnable");
        }
    }
    for (name, value) in [
        ("ProxyServer", s.server),
        ("ProxyOverride", s.bypass),
        ("AutoConfigURL", s.pac),
    ] {
        match value {
            Some(v) => k.set_value(name, &v).map_err(|e| e.to_string())?,
            None => {
                let _ = k.delete_value(name);
            }
        }
    }
    notify();
    std::fs::remove_file(path).map_err(|e| e.to_string())
}

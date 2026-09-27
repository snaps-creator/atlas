use serde::Deserialize;
use std::path::{Path, PathBuf};
use winreg::{enums::*, RegKey};
const INTERNET: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings";
#[derive(Deserialize)]
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
fn still_owned_proxy(enabled: u32, server: &str) -> bool {
    enabled == 1 && server == "127.0.0.1:17890"
}
pub fn restore(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    restore_with_key(path, key()?)
}
fn restore_with_key(path: &Path, k: RegKey) -> Result<(), String> {
    let s: Snapshot = serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|_| "Повреждена копия системного прокси")?;
    // Do not overwrite a proxy subsequently selected by another application.
    let current: String = k.get_value("ProxyServer").unwrap_or_default();
    let enabled: u32 = k.get_value("ProxyEnable").unwrap_or(0);
    if !still_owned_proxy(enabled, &current) {
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

/// Per-machine uninstall may be elevated under a different account from the
/// interactive Atlas user. Inspect only registered user profiles, and write
/// only when that user's exact Atlas marker and localhost proxy still match.
/// An unloaded hive with a marker blocks uninstall rather than claiming a
/// cleanup that happened only in the administrator's HKCU.
pub fn restore_loaded_profiles() -> Result<(), String> {
    let profiles = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList")
        .map_err(|e| format!("Список профилей Windows: {e}"))?;
    let users = RegKey::predef(HKEY_USERS);
    for entry in profiles.enum_keys() {
        let sid = entry.map_err(|e| format!("Чтение списка профилей Windows: {e}"))?;
        if !sid.starts_with("S-1-5-21-") || sid.contains('_') { continue; }
        let profile = profiles.open_subkey(&sid).map_err(|e| e.to_string())?;
        let raw: String = profile.get_value("ProfileImagePath").map_err(|e| e.to_string())?;
        let profile_path = PathBuf::from(expand_profile_path(&raw)?);
        let shell = format!(r"{}\Software\Microsoft\Windows\CurrentVersion\Explorer\User Shell Folders", sid);
        let local = if let Some(value) = users.open_subkey(&shell).ok()
            .and_then(|k| k.get_value::<String, _>("Local AppData").ok()) {
            let placeholder = regex::Regex::new("(?i)%userprofile%").unwrap();
            let profile_text = profile_path.to_string_lossy().into_owned();
            let replaced = placeholder.replace_all(&value, regex::NoExpand(&profile_text));
            PathBuf::from(expand_profile_path(&replaced)?)
        } else { profile_path.join("AppData").join("Local") };
        let marker = local.join("net.atlasvpn.desktop").join("proxy-restore.json");
        if !marker.is_file() { continue; }
        let internet = format!(r"{}\{}", sid, INTERNET);
        let registry = users.open_subkey_with_flags(&internet, KEY_READ | KEY_WRITE)
            .map_err(|_| format!("Профиль {sid} содержит состояние прокси Atlas, но его реестр не загружен; удаление отменено"))?;
        restore_with_key(&marker, registry)?;
    }
    Ok(())
}
fn expand_profile_path(path: &str) -> Result<String, String> {
    // ProfileList values normally use %SystemDrive%. User-specific
    // %USERPROFILE% is substituted from that same profile above.
    let drive = std::env::var("SystemDrive").unwrap_or_default();
    let placeholder = regex::Regex::new("(?i)%systemdrive%").unwrap();
    if placeholder.is_match(path) && drive.is_empty() {
        return Err("Не удалось определить системный диск Windows".into());
    }
    let value = placeholder.replace_all(path, regex::NoExpand(&drive)).into_owned();
    if value.contains('%') { Err("Не удалось определить каталог профиля Windows".into()) }
    else { Ok(value) }
}

#[cfg(test)]
mod tests {
    use super::{expand_profile_path,still_owned_proxy};

    #[test]
    fn disabled_or_replaced_proxy_is_not_restored_from_legacy_marker() {
        assert!(!still_owned_proxy(0, "127.0.0.1:17890"));
        assert!(!still_owned_proxy(1, "127.0.0.1:7897"));
        assert!(still_owned_proxy(1, "127.0.0.1:17890"));
    }
    #[test]
    fn unknown_profile_environment_is_not_silently_assigned_to_the_admin() {
        assert_eq!(expand_profile_path(r"D:\Users\atlas").unwrap(), r"D:\Users\atlas");
        assert!(expand_profile_path(r"%UNKNOWN_PROFILE_ROOT%\atlas").is_err());
    }
}

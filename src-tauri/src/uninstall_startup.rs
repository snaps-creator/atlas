//! Remove only startup registrations owned by this installation. The elevated
//! helper may run under a different account, so inspect registered profiles.
use std::path::Path;
use winreg::{enums::*, types::FromRegValue, RegKey};
const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
fn normalized(path: &str) -> String {
    path.strip_prefix(r"\\?\")
        .unwrap_or(path)
        .replace('/', "\\")
        .to_lowercase()
}
fn owned(command: &str, root: &Path) -> bool {
    let Some(rest) = command.strip_prefix('"') else {
        return false;
    };
    let Some((target, args)) = rest.split_once('"') else {
        return false;
    };
    let args = args.split_whitespace().collect::<Vec<_>>();
    let target = normalized(target);
    let root = normalized(&root.to_string_lossy());
    (target == format!("{root}\\atlasupdater.exe") && args == ["--launch", "--autostart"])
        || (target == format!("{root}\\atlas.exe") && args == ["--autostart"])
}
fn optional_key(user: &RegKey, path: &str) -> Result<Option<RegKey>, String> {
    match user.open_subkey_with_flags(path, KEY_READ | KEY_SET_VALUE) {
        Ok(key) => Ok(Some(key)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "Cannot inspect Atlas startup registration: {error}"
        )),
    }
}
fn delete(key: &RegKey) -> Result<(), String> {
    match key.delete_value("Atlas") {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Cannot remove Atlas startup registration: {error}")),
    }
}
fn cleanup_user(user: &RegKey, root: &Path) -> Result<(), String> {
    let Some(run) = optional_key(user, RUN)? else {
        return Ok(());
    };
    let value = match run.get_raw_value("Atlas") {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("Cannot read Atlas startup registration: {error}")),
    };
    // Unknown value types cannot be a command registered by Atlas. Preserve
    // them instead of letting another user's unrelated entry block uninstall.
    if !matches!(value.vtype, REG_SZ | REG_EXPAND_SZ) {
        return Ok(());
    }
    let command = String::from_reg_value(&value).map_err(|error| error.to_string())?;
    if !owned(&command, root) {
        return Ok(());
    }
    // Approved first: an interrupted cleanup retains the ownership evidence.
    if let Some(approved) = optional_key(user, APPROVED)? {
        delete(&approved)?;
    }
    delete(&run)
}
fn user_sid(sid: &str) -> bool {
    (sid.starts_with("S-1-5-21-") || sid.starts_with("S-1-12-1-"))
        && !sid.ends_with("_Classes")
        && sid
            .split('-')
            .skip(1)
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
}
fn profile_directory(raw: &str) -> Result<std::path::PathBuf, String> {
    let drive = std::env::var("SystemDrive").unwrap_or_default();
    let value = regex::Regex::new("(?i)%systemdrive%")
        .unwrap()
        .replace_all(raw, regex::NoExpand(&drive));
    let path = std::path::PathBuf::from(value.as_ref());
    if value.contains('%')
        || !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err("Invalid registered user profile path; startup cleanup incomplete".into());
    }
    Ok(path)
}
pub fn cleanup(root: &Path) -> Result<(), String> {
    let users = RegKey::predef(HKEY_USERS);
    let mut loaded = std::collections::HashSet::new();
    for sid in users.enum_keys() {
        let sid = sid.map_err(|error| error.to_string())?;
        if !user_sid(&sid) {
            continue;
        }
        let user = users.open_subkey(&sid).map_err(|error| error.to_string())?;
        cleanup_user(&user, root)?;
        loaded.insert(sid);
    }
    let profiles = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList")
        .map_err(|e| e.to_string())?;
    for sid in profiles.enum_keys() {
        let sid = sid.map_err(|e| e.to_string())?;
        if !user_sid(&sid) || loaded.contains(&sid) {
            continue;
        }
        // Prefer a hive loaded since the initial inventory (logon race).
        if let Ok(user) = users.open_subkey(&sid) {
            cleanup_user(&user, root)?;
            continue;
        }
        let profile = profiles.open_subkey(&sid).map_err(|e| e.to_string())?;
        let raw: String = profile
            .get_value("ProfileImagePath")
            .map_err(|e| e.to_string())?;
        let directory = profile_directory(&raw)?;
        let hive = directory.join("NTUSER.DAT");
        if !directory.exists() {
            continue;
        }
        use std::os::windows::fs::MetadataExt;
        // Never follow a reparse point to an unrelated registry hive.
        for path in [&directory, &hive] {
            let meta = std::fs::symlink_metadata(path).map_err(|e| {
                format!("Cannot inspect offline profile; startup cleanup incomplete: {e}")
            })?;
            if meta.file_attributes() & 0x400 != 0 {
                return Err(
                    "Offline profile is a reparse point; startup cleanup incomplete".into(),
                );
            }
        }
        // Private process-scoped app hive: no global mount/name, RAII unload.
        let user=RegKey::load_app_key_with_flags(&hive,KEY_READ|KEY_WRITE,REG_PROCESS_APPKEY)
            .map_err(|e|format!("Cannot inspect offline user startup; uninstall stopped before payload removal: {e}"))?;
        cleanup_user(&user, root)?;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn offline_profile_resolution_does_not_use_the_elevated_users_home() {
        assert_eq!(
            profile_directory(r"D:\Users\fixture").unwrap(),
            Path::new(r"D:\Users\fixture")
        );
        for raw in [
            r"%USERPROFILE%",
            r"%UNKNOWN%\fixture",
            r"relative\fixture",
            r"D:\Users\..\Windows",
        ] {
            assert!(profile_directory(raw).is_err());
        }
        assert!(user_sid("S-1-5-21-1-2-3-1001"));
        assert!(user_sid("S-1-12-1-1-2-3-4"));
        assert!(!user_sid("S-1-5-21-1-2-3-1001_Classes"));
        assert!(!user_sid("S-1-5-21-invalid"));
    }
    #[test]
    fn ownership_is_exact_and_understands_canonical_windows_paths() {
        let root = Path::new(r"\\?\C:\Program Files\Atlas");
        assert!(owned(
            r#""C:\Program Files\Atlas\AtlasUpdater.exe" --launch --autostart"#,
            root
        ));
        assert!(owned(
            r#""c:\program files\atlas\Atlas.exe" --autostart"#,
            root
        ));
        for value in [
            r#""C:\Other\Atlas.exe" --autostart"#,
            r#""C:\Program Files\Atlas\Atlas.exe" --autostart --other"#,
            "Atlas",
        ] {
            assert!(!owned(value, root));
        }
    }
    #[test]
    fn cleanup_is_scoped_to_own_user_entry_and_repeat_safe() {
        let name = format!(r"Software\AtlasTests\uninstall-{}", uuid::Uuid::new_v4());
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (fixture, _) = hkcu.create_subkey(&name).unwrap();
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(&self.0);
            }
        }
        let _cleanup = Cleanup(name);
        let root = Path::new(r"C:\Atlas");
        let (foreign, _) = fixture.create_subkey("foreign-non-string").unwrap();
        let (foreign_run, _) = foreign.create_subkey(RUN).unwrap();
        foreign_run.set_value("Atlas", &7u32).unwrap();
        cleanup_user(&foreign, root).unwrap();
        assert_eq!(foreign_run.get_value::<u32, _>("Atlas").unwrap(), 7);

        for (account, command) in [
            (
                "interactive",
                r#""C:\Atlas\AtlasUpdater.exe" --launch --autostart"#,
            ),
            ("elevated-other", r#""C:\Other\Atlas.exe" --autostart"#),
        ] {
            let (user, _) = fixture.create_subkey(account).unwrap();
            let (run, _) = user.create_subkey(RUN).unwrap();
            let (approved, _) = user.create_subkey(APPROVED).unwrap();
            run.set_value("Atlas", &command).unwrap();
            run.set_value("Unrelated", &"keep").unwrap();
            approved.set_value("Atlas", &1u32).unwrap();
            approved.set_value("Unrelated", &2u32).unwrap();
            cleanup_user(&user, root).unwrap();
            cleanup_user(&user, root).unwrap();
            assert_eq!(
                run.get_value::<String, _>("Atlas").is_ok(),
                account != "interactive"
            );
            assert_eq!(
                approved.get_value::<u32, _>("Atlas").is_ok(),
                account != "interactive"
            );
            assert_eq!(run.get_value::<String, _>("Unrelated").unwrap(), "keep");
            assert_eq!(approved.get_value::<u32, _>("Unrelated").unwrap(), 2);
        }
    }
}

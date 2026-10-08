//! Remove only startup registrations owned by this installation. The elevated
//! helper may run under a different account, so inspect loaded user hives.
use std::path::Path;
use winreg::{enums::*, RegKey};
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
    let command = match run.get_value::<String, _>("Atlas") {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("Cannot read Atlas startup registration: {error}")),
    };
    if !owned(&command, root) {
        return Ok(());
    }
    // Approved first: an interrupted cleanup retains the ownership evidence.
    if let Some(approved) = optional_key(user, APPROVED)? {
        delete(&approved)?;
    }
    delete(&run)
}
pub fn cleanup(root: &Path) -> Result<(), String> {
    let users = RegKey::predef(HKEY_USERS);
    for sid in users.enum_keys() {
        let sid = sid.map_err(|error| error.to_string())?;
        if !(sid.starts_with("S-1-5-21-") || sid.starts_with("S-1-12-1-"))
            || sid.ends_with("_Classes")
        {
            continue;
        }
        let user = users.open_subkey(&sid).map_err(|error| error.to_string())?;
        cleanup_user(&user, root)?;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
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

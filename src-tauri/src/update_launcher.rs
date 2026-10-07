//! Stable native entry point. The durable journal, never the working directory,
//! selects the desktop executable. Critical transactions cannot start a desktop.
use crate::update_transaction::{Stage, Transaction};
use std::{path::{Path, PathBuf}, process::{Command, Stdio}};

fn launchable(stage: Stage) -> bool {
    matches!(stage, Stage::Created | Stage::Downloading | Stage::Downloaded |
        Stage::Verified | Stage::Prepared | Stage::Failed | Stage::RolledBack | Stage::Committed)
}

pub fn resolve(transaction: &Transaction) -> Result<PathBuf, String> {
    if !launchable(transaction.journal.stage) {
        return Err(format!("Atlas update requires recovery at stage {:?}", transaction.journal.stage));
    }
    let active = &transaction.journal.active;
    if active.absent{return Err("Atlas installation was rolled back; no application is installed".into());}
    let directory = transaction.version_path(active);
    active.verify(&directory)?;
    for required in ["Atlas.exe", "Atlas.Service.exe", "AtlasMaintenance.exe", "libcef.dll"] {
        if !active.files.iter().any(|f| f.path == required) {
            return Err(format!("Active Atlas payload is missing {required}"));
        }
    }
    // The service deliberately grants desktop users status/start/stop only.
    // Its machine-owned registry configuration is readable without granting
    // SERVICE_QUERY_CONFIG or elevating the desktop launcher.
    let service_key = winreg::RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(r"SYSTEM\CurrentControlSet\Services\AtlasNetworkService",winreg::enums::KEY_READ)
        .map_err(|e|format!("Cannot read registered Atlas service: {e}"))?;
    let registered:String=service_key.get_value("ImagePath")
        .map_err(|e|format!("Cannot read Atlas service image: {e}"))?;
    if active.build=="legacy" && registered.eq_ignore_ascii_case(&crate::update_service::command(transaction.root())?) {
        active.verify_files(transaction.root())?;
        return Ok(transaction.root().to_path_buf());
    }
    if !registered.eq_ignore_ascii_case(&crate::update_service::command(&directory)?) {
        return Err("Atlas service points outside the active version; update recovery is required".into());
    }
    Ok(directory)
}

pub fn launch(root: &Path, autostart: bool) -> Result<(), String> {
    // Hold the update lease through CreateProcess so activation cannot delete or
    // switch the selected payload between verification and process creation.
    // The desktop takes its own admission lease during setup after we release it.
    let transaction = Transaction::open(root)?;
    if !launchable(transaction.journal.stage) {
        // An interrupted update (including reboot) is reconciled through the
        // same supervisor. Release its mutex before the elevated helper opens it.
        drop(transaction);
        return crate::update_install::elevated(&root.join("AtlasUpdater.exe"),"--recover");
    }
    let directory = resolve(&transaction)?;
    let arguments:Vec<&str>=if autostart{vec!["--autostart"]}else{Vec::new()};
    if let Some((child,pipe))=crate::update_user::limited(&directory.join("Atlas.exe"),&arguments)? {
        drop(transaction);drop(pipe);drop(child);return Ok(());
    }
    let mut command = Command::new(directory.join("Atlas.exe"));
    command.current_dir(&directory).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    if autostart { command.arg("--autostart"); }
    // Tauri's existing single-instance IPC restores the existing desktop window.
    // Do not hold a second lifetime lock which would prevent that notification.
    let child = command.spawn().map_err(|error| format!("Cannot launch active Atlas desktop: {error}"))?;
    drop(transaction);
    drop(child);
    Ok(())
}

pub fn record_failure(error: &str) {
    // Per-user logs remain writable when the installation root is protected.
    // Only native launcher diagnostics are passed here, never subscription data.
    use std::io::Write;
    let Some(local) = std::env::var_os("LOCALAPPDATA") else { return; };
    let directory = PathBuf::from(local).join("Atlas").join("logs");
    if std::fs::create_dir_all(&directory).is_err() { return; }
    let path = directory.join("launcher.ndjson");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 1024 * 1024) {
        let previous = directory.join("launcher.previous.ndjson");
        let _ = std::fs::remove_file(&previous);
        let _ = std::fs::rename(&path, previous);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let event = serde_json::json!({
            "time": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs(),
            "version": env!("CARGO_PKG_VERSION"), "operation": "launch", "error": error
        });
        let _ = writeln!(file, "{event}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interrupted_activation_never_launches_uncommitted_desktop() {
        for stage in [Stage::Quiescing, Stage::Quiesced, Stage::SwitchIntent,
            Stage::Activated, Stage::HealthPending, Stage::RollbackPending] {
            assert!(!launchable(stage), "{stage:?}");
        }
        for stage in [Stage::Created, Stage::Downloading, Stage::Downloaded,
            Stage::Verified, Stage::Prepared, Stage::Failed, Stage::RolledBack, Stage::Committed] {
            assert!(launchable(stage), "{stage:?}");
        }
    }
}

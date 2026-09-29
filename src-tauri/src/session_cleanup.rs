//! Survives a hard kill of the service; never edits routes/adapters/proxy settings.
use std::{
    io::{Read, Write},
    os::windows::{
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
    process::{Command, Stdio},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::WAIT_OBJECT_0,
    Storage::FileSystem::SYNCHRONIZE,
    System::{
        Pipes::PeekNamedPipe,
        Threading::{OpenProcess, WaitForMultipleObjects, WaitForSingleObject, INFINITE,
            PROCESS_TERMINATE, TerminateProcess},
    },
};

#[cfg(not(test))]
#[link(name = "dnsapi")]
extern "system" {
    fn DnsFlushResolverCache() -> i32;
}

pub(crate) fn flush_dns() -> Result<(), String> {
    // Isolated tests must not mutate the host resolver cache.
    #[cfg(not(test))]
    if unsafe { DnsFlushResolverCache() } == 0 {
        return Err("Не удалось очистить DNS-кэш после завершения Atlas".into());
    }
    Ok(())
}

/// SYSTEM service only, before TUN startup. Ready byte confirms the exact
/// service process handle is held; PID reuse cannot change the observed owner.
pub(crate) fn start_observer(directory: &Path, desktop_pid: u32) -> Result<std::process::Child, String> {
    owned_session_directory(directory, &PathBuf::from(std::env::var_os("ProgramData").ok_or("ProgramData is unavailable")?))?;
    let mut child = Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
        .args(["--watch-network-session", &std::process::id().to_string(), &desktop_pid.to_string()])
        .arg(directory)
        .creation_flags(0x08000000)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Наблюдатель очистки Atlas: {e}"))?;
    let mut output = child.stdout.take().ok_or("Нет канала наблюдателя")?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let mut available = 0;
        let ok = unsafe {
            PeekNamedPipe(
                output.as_raw_handle(),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut available,
                std::ptr::null_mut(),
            )
        };
        if ok != 0 && available > 0 {
            let mut ready = [0];
            if output.read_exact(&mut ready).is_ok() && ready == [1] {
                return Ok(child);
            }
            break;
        }
        if ok == 0
            || child.try_wait().map_err(|e| e.to_string())?.is_some()
            || Instant::now() >= deadline
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
    let mut detail = String::new();
    if let Some(stderr) = child.stderr.take() { let _ = stderr.take(4096).read_to_string(&mut detail); }
    let detail = detail.trim();
    Err(if detail.is_empty() { "Наблюдатель очистки Atlas не готов; запуск TUN отменён".into() }
        else { format!("Наблюдатель очистки Atlas: {detail}; запуск TUN отменён") })
}

pub(crate) fn after_process_exit(
    handle: &OwnedHandle,
    cleanup: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    if unsafe { WaitForSingleObject(handle.as_raw_handle(), INFINITE) } != WAIT_OBJECT_0 {
        return Err("Не удалось дождаться завершения сетевой службы".into());
    }
    cleanup()
}

/// A SYSTEM observer makes the VPN service's lifetime follow the desktop owner.
/// If Task Manager kills the UI, allow IPC-loss cleanup briefly, then terminate
/// only the already-verified Atlas service after a short grace period. Its job
/// object closes the owned core before this observer removes session residue.
fn stop_service_when_owner_exits(
    service: &OwnedHandle,
    owner: &OwnedHandle,
    grace: Duration,
) -> Result<(), String> {
    use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
    let handles = [service.as_raw_handle(), owner.as_raw_handle()];
    loop {
        match unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, 100) } {
            WAIT_OBJECT_0 => return Ok(()),
            x if x == WAIT_OBJECT_0 + 1 => {
                // IPC loss already cancels the service. Never stop it by name:
                // a new service instance could replace the old one meanwhile.
                if unsafe { WaitForSingleObject(service.as_raw_handle(), grace.as_millis() as u32) } == WAIT_OBJECT_0 {
                    return Ok(());
                }
                if unsafe { TerminateProcess(service.as_raw_handle(), 1) } == 0
                    && unsafe { WaitForSingleObject(service.as_raw_handle(), 0) } != WAIT_OBJECT_0 {
                    return Err("Не удалось завершить зависшую службу Atlas после выхода владельца".into());
                }
                if unsafe { WaitForSingleObject(service.as_raw_handle(), 500) } == WAIT_OBJECT_0 {
                    return Ok(());
                }
                return Err("Служба Atlas не завершилась после принудительной остановки".into());
            }
            WAIT_TIMEOUT => continue,
            _ => return Err("Не удалось наблюдать за процессами Atlas".into()),
        }
    }
}

/// Internal executable mode authenticated against SCM, without starting the UI.
fn owned_session_directory(directory: &Path, root: &Path) -> Result<(), String> {
    let directory = directory.canonicalize().map_err(|e| format!("Каталог сессии Atlas: {e}"))?;
    let root = root.canonicalize().map_err(|e| format!("Корень ProgramData: {e}"))?;
    let name = directory.file_name().and_then(|n| n.to_str()).ok_or("Некорректное имя сессии Atlas")?;
    let suffix = name.strip_prefix("Atlas-session-").ok_or("Посторонний каталог не может быть очищен Atlas")?;
    uuid::Uuid::parse_str(suffix).map_err(|_| "Некорректный идентификатор каталога Atlas")?;
    if directory.parent() != Some(root.as_path()) {
        return Err("Каталог сессии находится вне ProgramData".into());
    }
    Ok(())
}

// The observer runs as Atlas.Service.exe, while its desktop owner is Atlas.exe.
// Keep the exact protected installation directory; never accept a name alone.
pub fn verify_desktop_owner(pid: u32, service_image: &Path) -> Result<(), String> {
    crate::broker::verify_process_image(pid, &service_image.with_file_name("Atlas.exe"))
        .map_err(|e| format!("Не подтверждён процесс интерфейса Atlas: {e}"))
}

/// Internal executable mode authenticated against SCM, without starting the UI.
pub(crate) fn run(service_pid: u32, desktop_pid: u32, directory: &Path) -> Result<(), String> {
    let handle = unsafe { OpenProcess(PROCESS_TERMINATE | SYNCHRONIZE, 0, service_pid) };
    if handle.is_null() {
        return Err("Процесс сетевой службы не найден".into());
    }
    let service = unsafe { OwnedHandle::from_raw_handle(handle) };
    crate::service::verify_server_pid(service_pid)?;
    let owner = unsafe { OpenProcess(SYNCHRONIZE, 0, desktop_pid) };
    if owner.is_null() { return Err("Владелец сетевой сессии Atlas не найден".into()); }
    let owner = unsafe { OwnedHandle::from_raw_handle(owner) };
    verify_desktop_owner(desktop_pid, &std::env::current_exe().map_err(|e| e.to_string())?)?;
    let root = PathBuf::from(std::env::var_os("ProgramData").ok_or("ProgramData is unavailable")?);
    owned_session_directory(directory, &root)?;
    std::io::stdout()
        .write_all(&[1])
        .map_err(|e| e.to_string())?;
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    stop_service_when_owner_exits(&service, &owner, Duration::from_millis(200))?;
    // Windows closes the service's job and terminates its owned core. Keep
    // post-exit cleanup bounded even if the DNS client RPC stalls.
    after_process_exit(&service, || {
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(900));
            std::process::exit(1);
        });
        // A hard-killed service cannot remove its credential-bearing work
        // directory. Its Job Object has closed by this point, so the owned
        // core is gone and this exact validated session can be removed.
        let routes = crate::network_guard::clear_routes_for_reusable_tun().map(|_| ());
        let files = match std::fs::remove_dir_all(directory) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("Очистка каталога сессии Atlas: {error}")),
        };
        let dns = flush_dns();
        let errors: Vec<_> = [routes, dns, files].into_iter().filter_map(Result::err).collect();
        if errors.is_empty() { Ok(()) } else { Err(errors.join("; ")) }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    #[test]
    fn installed_service_accepts_its_desktop_but_rejects_another_directory() {
        let root = std::env::temp_dir().join(format!("atlas-owner-test-{}", uuid::Uuid::new_v4()));
        let foreign = root.join("other");
        std::fs::create_dir_all(&foreign).unwrap();
        let desktop = root.join("Atlas.exe");
        let service = root.join("Atlas.Service.exe");
        let shell = std::env::var_os("COMSPEC").unwrap();
        for path in [&desktop, &service, &foreign.join("Atlas.exe")] { std::fs::copy(&shell, path).unwrap(); }
        let mut child = Command::new(&desktop).args(["/d", "/c", "set /p ATLAS_OWNER_TEST="])
            .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).creation_flags(0x08000000).spawn().unwrap();
        let job = crate::job::Job::attach(&child).unwrap();
        let old_check = crate::broker::verify_process_image(child.id(), &service);
        let valid = verify_desktop_owner(child.id(), &service);
        let wrong_directory = verify_desktop_owner(child.id(), &foreign.join("Atlas.Service.exe"));
        let _ = child.kill(); let _ = child.wait(); drop(job);
        std::fs::remove_dir_all(root).unwrap();
        assert!(old_check.is_err(), "regression fixture must reproduce the old mismatch");
        assert!(valid.is_ok(), "installed Atlas.exe must be accepted: {valid:?}");
        assert!(wrong_directory.is_err(), "an identically named foreign executable must stay rejected");
    }
    #[test]
    fn cleanup_waits_for_real_process_death_including_forced_termination() {
        let mut child = Command::new("powershell.exe")
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
        let job = crate::job::Job::attach(&child).unwrap();
        let handle = unsafe { OpenProcess(SYNCHRONIZE, 0, child.id()) };
        assert!(!handle.is_null());
        let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
        let cleaned = Arc::new(AtomicBool::new(false));
        let worker_cleaned = cleaned.clone();
        let worker = std::thread::spawn(move || {
            after_process_exit(&handle, || {
                worker_cleaned.store(true, Ordering::SeqCst);
                Ok(())
            })
        });
        std::thread::sleep(Duration::from_millis(100));
        assert!(!cleaned.load(Ordering::SeqCst));
        child.kill().unwrap();
        child.wait().unwrap();
        worker.join().unwrap().unwrap();
        assert!(cleaned.load(Ordering::SeqCst));
        drop(job);
    }
    #[test]
    fn killing_desktop_owner_terminates_only_its_verified_service_process() {
        let mut service_child = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", "Start-Sleep -Seconds 60"])
            .creation_flags(0x08000000).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let mut owner_child = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", "Start-Sleep -Seconds 60"])
            .creation_flags(0x08000000).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let mut unrelated_child = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", "Start-Sleep -Seconds 60"])
            .creation_flags(0x08000000).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let service = unsafe { OpenProcess(PROCESS_TERMINATE | SYNCHRONIZE, 0, service_child.id()) };
        let owner = unsafe { OpenProcess(SYNCHRONIZE, 0, owner_child.id()) };
        assert!(!service.is_null() && !owner.is_null());
        let service = unsafe { OwnedHandle::from_raw_handle(service) };
        let owner = unsafe { OwnedHandle::from_raw_handle(owner) };
        let worker = std::thread::spawn(move ||
            stop_service_when_owner_exits(&service, &owner, Duration::from_millis(100)));
        let started = Instant::now();
        owner_child.kill().unwrap();
        owner_child.wait().unwrap();
        worker.join().unwrap().unwrap();
        assert!(started.elapsed() < Duration::from_secs(2), "owner cleanup took {:?}", started.elapsed());
        assert!(service_child.try_wait().unwrap().is_some());
        assert!(unrelated_child.try_wait().unwrap().is_none(), "unrelated process was terminated");
        unrelated_child.kill().unwrap();
        unrelated_child.wait().unwrap();
    }
    #[test]
    fn only_an_exact_owned_session_directory_can_be_recursively_removed() {
        let root = std::env::temp_dir().join(format!("atlas-cleanup-root-{}", uuid::Uuid::new_v4()));
        let owned = root.join(format!("Atlas-session-{}", uuid::Uuid::new_v4()));
        let foreign = root.join("other-product");
        std::fs::create_dir_all(&owned).unwrap();
        std::fs::create_dir_all(&foreign).unwrap();
        assert!(owned_session_directory(&owned, &root).is_ok());
        assert!(owned_session_directory(&foreign, &root).is_err());
        assert!(owned_session_directory(&root, &root).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}

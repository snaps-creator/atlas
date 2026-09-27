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
        Threading::{OpenProcess, WaitForSingleObject, INFINITE},
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
pub(crate) fn start_observer(directory: &Path) -> Result<std::process::Child, String> {
    owned_session_directory(directory, &PathBuf::from(std::env::var_os("ProgramData").ok_or("ProgramData is unavailable")?))?;
    let mut child = Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
        .args(["--watch-network-session", &std::process::id().to_string()])
        .arg(directory)
        .creation_flags(0x08000000)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
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
    Err("Наблюдатель очистки Atlas не готов; запуск TUN отменён".into())
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

/// Internal executable mode authenticated against SCM, without starting the UI.
pub(crate) fn run(service_pid: u32, directory: &Path) -> Result<(), String> {
    let handle = unsafe { OpenProcess(SYNCHRONIZE, 0, service_pid) };
    if handle.is_null() {
        return Err("Процесс сетевой службы не найден".into());
    }
    let service = unsafe { OwnedHandle::from_raw_handle(handle) };
    crate::service::verify_server_pid(service_pid)?;
    let root = PathBuf::from(std::env::var_os("ProgramData").ok_or("ProgramData is unavailable")?);
    owned_session_directory(directory, &root)?;
    std::io::stdout()
        .write_all(&[1])
        .map_err(|e| e.to_string())?;
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    after_process_exit(&service, || {
        // Windows closes the service's job and terminates its owned core.
        // Bound even an unexpectedly stuck DNS-client RPC in this helper.
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_secs(5));
            std::process::exit(1);
        });
        std::thread::sleep(Duration::from_millis(250));
        let dns = flush_dns();
        // A hard-killed service cannot remove its credential-bearing work
        // directory. Its Job Object has closed by this point, so the owned
        // core is gone and this exact validated session can be removed.
        let files = match std::fs::remove_dir_all(directory) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("Очистка каталога сессии Atlas: {error}")),
        };
        let errors: Vec<_> = [dns, files].into_iter().filter_map(Result::err).collect();
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

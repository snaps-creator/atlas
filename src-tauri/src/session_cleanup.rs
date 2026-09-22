//! Survives a hard kill of the service; never edits routes/adapters/proxy settings.
use std::{
    io::{Read, Write},
    os::windows::{
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
    process::{Command, Stdio},
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
pub(crate) fn start_observer() -> Result<std::process::Child, String> {
    let mut child = Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
        .args(["--watch-network-session", &std::process::id().to_string()])
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
pub(crate) fn run(service_pid: u32) -> Result<(), String> {
    let handle = unsafe { OpenProcess(SYNCHRONIZE, 0, service_pid) };
    if handle.is_null() {
        return Err("Процесс сетевой службы не найден".into());
    }
    let service = unsafe { OwnedHandle::from_raw_handle(handle) };
    crate::service::verify_server_pid(service_pid)?;
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
        flush_dns()
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
}

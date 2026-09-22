use std::{process::Child, time::{Duration, Instant}};

pub fn stop(child: &mut Child, release_job: impl FnOnce(), timeout: Duration) -> Result<(), String> {
    let wait = process_waiter(child);
    stop_with(
        || child.kill().map_err(|e| e.to_string()),
        release_job,
        // Waiting on the handle does not need another mutable borrow of Child.
        // The handle stays owned by the caller throughout this function.
        wait,
        timeout,
    )
}
fn process_waiter(child: &Child) -> impl FnMut(Duration) -> Result<bool, String> + 'static {
    use std::os::windows::io::AsRawHandle;
    let handle = child.as_raw_handle();
    move |timeout| {
        use windows_sys::Win32::{Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT}, System::Threading::WaitForSingleObject};
        match unsafe { WaitForSingleObject(handle, timeout.as_millis().min(u32::MAX as u128 - 1) as u32) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            _ => Err(std::io::Error::last_os_error().to_string()),
        }
    }
}
fn stop_with(
    kill: impl FnOnce() -> Result<(), String>,
    release_job: impl FnOnce(),
    mut wait: impl FnMut(Duration) -> Result<bool, String>,
    timeout: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    let kill_error = kill().err();
    // KILL_ON_JOB_CLOSE is the independent fallback. Release it BEFORE waiting.
    release_job();
    match wait(deadline.saturating_duration_since(Instant::now())) {
        Ok(true) => Ok(()), // A failed kill is harmless only if exit is confirmed.
        result => Err(format!("Завершение Mihomo не подтверждено: {}; kill: {}",
            match result { Err(e) => e, _ => "истекло время ожидания".into() },
            kill_error.unwrap_or_else(|| "запрос принят".into()))),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_kill_releases_job_before_wait_and_reports_unconfirmed_exit() {
        let released = std::cell::Cell::new(false);
        let result = stop_with(|| Err("access denied".into()), || released.set(true), |duration| {
            assert!(released.get());
            assert!(duration <= Duration::from_millis(20));
            Ok(false)
        }, Duration::from_millis(20));
        assert!(result.unwrap_err().contains("access denied"));
    }
    #[test]
    fn confirmed_job_exit_recovers_from_failed_kill() {
        assert!(stop_with(|| Err("kill failed".into()), || {}, |_| Ok(true), Duration::ZERO).is_ok());
    }
    #[test]
    fn job_fallback_really_terminates_owned_process_when_kill_fails() {
        use std::os::windows::process::CommandExt;
        let mut child = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", "Start-Sleep -Seconds 60"])
            .creation_flags(0x08000000).stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null()).spawn().unwrap();
        let job = crate::job::Job::attach(&child).unwrap();
        let wait = process_waiter(&child);
        let result = stop_with(|| Err("injected kill failure".into()), || drop(job), wait, Duration::from_secs(2));
        if result.is_err() { let _ = child.kill(); }
        assert!(result.is_ok(), "{result:?}");
        assert!(child.try_wait().unwrap().is_some());
    }
}

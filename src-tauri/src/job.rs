use std::os::windows::io::AsRawHandle;
use windows_sys::Win32::{Foundation::CloseHandle, System::JobObjects::*};
/// Closing the application's handle terminates its owned core, even after a crash.
pub struct Job(isize);
impl Job {
    pub fn attach(child: &std::process::Child) -> Result<Self, String> {
        let job = Self::create(std::ptr::null())?;
        if unsafe { AssignProcessToJobObject(job.0 as _, child.as_raw_handle()) } == 0 {
            return Err("Не удалось связать жизненный цикл ядра с приложением".into());
        }
        Ok(job)
    }
    fn create(name: *const u16) -> Result<Self, String> {
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), name);
            if handle.is_null() {
                return Err("Не удалось создать Windows Job Object".into());
            }
            let job = Self(handle as isize);
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                std::mem::size_of_val(&limits) as u32,
            ) == 0
            {
                return Err("Не удалось настроить завершение дочерних процессов Atlas".into());
            }
            Ok(job)
        }
    }
}

const DESKTOP_JOB_ENV: &str = "ATLAS_DESKTOP_JOB";
/// Chromium utility processes can reject nested job assignment (access denied).
/// An owned Windows mutex instead reports owner death, including crashes, to
/// helpers without changing Chromium's job/security settings. Installers never
/// subscribe. Create and drop this owner on the same desktop thread.
pub struct DesktopJob {
    owner: isize,
}
impl DesktopJob {
    pub fn new() -> Result<Self, String> {
        let name = format!("Local\\Atlas-UI-{}", uuid::Uuid::new_v4());
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let owner = unsafe { windows_sys::Win32::System::Threading::CreateMutexW(std::ptr::null(), 1, wide.as_ptr()) };
        if owner.is_null() { return Err(format!("Не удалось создать наблюдателя интерфейса: {}", std::io::Error::last_os_error())); }
        std::env::set_var(DESKTOP_JOB_ENV, name);
        Ok(Self { owner: owner as isize })
    }
    /// Called before CEF initializes in each renderer/GPU/utility process.
    /// SYNCHRONIZE access is sufficient; no process/job mutation is needed.
    pub fn join() -> Result<(), String> {
        use windows_sys::Win32::System::Threading::{OpenMutexW,WaitForSingleObject,ExitProcess};
        let name = std::env::var(DESKTOP_JOB_ENV).map_err(|_| "Нет владельца процесса интерфейса Atlas")?;
        let suffix = name.strip_prefix("Local\\Atlas-UI-").ok_or("Некорректный владелец интерфейса Atlas")?;
        uuid::Uuid::parse_str(suffix).map_err(|_| "Некорректный идентификатор интерфейса Atlas")?;
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        unsafe {
            let handle = OpenMutexW(0x00100000, 0, wide.as_ptr()); // SYNCHRONIZE
            if handle.is_null() { return Err(format!("Основной процесс Atlas уже завершён: {}", std::io::Error::last_os_error())); }
            let raw = handle as isize;
            if let Err(error) = std::thread::Builder::new().name("atlas-desktop-lifetime".into()).spawn(move || {
                let result = WaitForSingleObject(raw as _, u32::MAX);
                // Both a normal owner release and an abandoned mutex mean
                // the desktop is gone. ExitProcess also releases ownership,
                // waking the next helper; no polling or PID reuse is involved.
                if result == 0 || result == 0x80 { ExitProcess(0); }
                ExitProcess(1);
            }) {
                CloseHandle(handle);
                return Err(format!("Не удалось запустить наблюдателя интерфейса: {error}"));
            }
        }
        Ok(())
    }
}
impl Drop for DesktopJob {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::System::Threading::ReleaseMutex(self.owner as _);
            CloseHandle(self.owner as _);
        }
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0 as _);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::windows::process::CommandExt,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    #[test]
    fn desktop_process_fixture() {
        let Ok(mode) = std::env::var("ATLAS_JOB_TEST_MODE") else { return; };
        let directory = std::path::PathBuf::from(std::env::var_os("ATLAS_JOB_TEST_DIR").unwrap());
        if mode == "child" {
            DesktopJob::join().unwrap();
            DesktopJob::join().unwrap(); // Descendants can inherit membership.
            std::fs::write(directory.join("ready"), b"joined").unwrap();
            std::thread::sleep(Duration::from_secs(20));
        } else {
            let _job = DesktopJob::new().unwrap();
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "job::tests::desktop_process_fixture"])
                .env("ATLAS_JOB_TEST_MODE", "child")
                .creation_flags(0x08000000).stdout(Stdio::null()).stderr(Stdio::null())
                .spawn().unwrap();
            std::fs::write(directory.join("pid"), child.id().to_string()).unwrap();
            let mut installer = Command::new("powershell.exe")
                .args(["-NoProfile", "-NonInteractive", "-Command", "Start-Sleep -Seconds 20"])
                .creation_flags(0x08000000).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
            std::fs::write(directory.join("installer-pid"), installer.id().to_string()).unwrap();
            if mode == "release" {
                let deadline = Instant::now() + Duration::from_secs(5);
                while !directory.join("release-owner").exists() {
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(10));
                }
                drop(_job);
            }
            std::thread::sleep(Duration::from_secs(20));
            let _ = child.kill();
            let _ = child.wait();
            let _ = installer.kill(); let _ = installer.wait();
        }
    }
    #[test]
    fn killing_desktop_kills_joined_helpers_but_not_an_installer() {
        check_desktop_lifetime(false);
    }
    #[test]
    fn normal_desktop_release_stops_helpers_but_not_an_installer() {
        check_desktop_lifetime(true);
    }
    fn check_desktop_lifetime(release: bool) {
        use std::os::windows::io::{FromRawHandle, OwnedHandle};
        use windows_sys::Win32::{Storage::FileSystem::SYNCHRONIZE,
            System::Threading::{OpenProcess, WaitForSingleObject}, Foundation::WAIT_OBJECT_0};
        let directory = std::env::temp_dir().join(format!("atlas-ui-job-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let mut owner = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "job::tests::desktop_process_fixture"])
            .env("ATLAS_JOB_TEST_MODE", if release { "release" } else { "owner" }).env("ATLAS_JOB_TEST_DIR", &directory)
            .creation_flags(0x08000000).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !(directory.join("ready").exists() && directory.join("pid").exists() && directory.join("installer-pid").exists()) {
            if Instant::now() >= deadline || owner.try_wait().unwrap().is_some() {
                let _ = owner.kill();
                panic!("desktop/helper handshake failed");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let pid = std::fs::read_to_string(directory.join("pid")).unwrap().parse().unwrap();
        let raw = unsafe { OpenProcess(SYNCHRONIZE, 0, pid) };
        assert!(!raw.is_null());
        let helper = unsafe { OwnedHandle::from_raw_handle(raw) };
        let installer_pid = std::fs::read_to_string(directory.join("installer-pid")).unwrap().parse().unwrap();
        let installer_raw = unsafe { OpenProcess(SYNCHRONIZE | windows_sys::Win32::System::Threading::PROCESS_TERMINATE, 0, installer_pid) };
        assert!(!installer_raw.is_null());
        let installer = unsafe { OwnedHandle::from_raw_handle(installer_raw) };
        if release { std::fs::write(directory.join("release-owner"), b"release").unwrap(); }
        else { owner.kill().unwrap(); owner.wait().unwrap(); }
        let helper_exited = unsafe { WaitForSingleObject(helper.as_raw_handle(), 1000) };
        let installer_survived = unsafe { WaitForSingleObject(installer.as_raw_handle(), 0) } == windows_sys::Win32::Foundation::WAIT_TIMEOUT;
        if release { owner.kill().unwrap(); owner.wait().unwrap(); }
        unsafe {
            windows_sys::Win32::System::Threading::TerminateProcess(installer.as_raw_handle(), 0);
            WaitForSingleObject(installer.as_raw_handle(), 1000);
        }
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(helper_exited, WAIT_OBJECT_0);
        assert!(installer_survived);
    }
    #[test]
    fn closing_job_terminates_only_owned_child() {
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
        let job = Job::attach(&child).unwrap();
        assert!(child.try_wait().unwrap().is_none());
        drop(job);
        let deadline = Instant::now() + Duration::from_secs(3);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("job close did not terminate owned child");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

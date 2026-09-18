use std::os::windows::io::AsRawHandle;
use windows_sys::Win32::{Foundation::CloseHandle, System::JobObjects::*};
/// Closing the application's handle terminates its owned core, even after a crash.
pub struct Job(isize);
impl Job {
    pub fn attach(child: &std::process::Child) -> Result<Self, String> {
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
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
                || AssignProcessToJobObject(handle, child.as_raw_handle()) == 0
            {
                return Err("Не удалось связать жизненный цикл Mihomo с приложением".into());
            }
            Ok(job)
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

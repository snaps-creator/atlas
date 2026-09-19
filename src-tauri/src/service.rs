use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use windows_sys::Win32::{
    Foundation::NO_ERROR,
    System::Services::{
        RegisterServiceCtrlHandlerExW, SetServiceStatus, StartServiceCtrlDispatcherW,
        SERVICE_ACCEPT_SHUTDOWN, SERVICE_ACCEPT_STOP, SERVICE_CONTROL_SHUTDOWN,
        SERVICE_CONTROL_STOP, SERVICE_RUNNING, SERVICE_START_PENDING, SERVICE_STATUS,
        SERVICE_STOPPED, SERVICE_STOP_PENDING, SERVICE_TABLE_ENTRYW, SERVICE_WIN32_OWN_PROCESS,
    },
};

pub const NAME: &str = "AtlasNetworkService";
static STOPPING: AtomicBool = AtomicBool::new(false);
static STATUS_HANDLE: AtomicPtr<core::ffi::c_void> = AtomicPtr::new(std::ptr::null_mut());

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

pub(crate) fn is_stopping() -> bool {
    STOPPING.load(Ordering::SeqCst)
}

fn report(state: u32, accepted: u32, exit_code: u32, wait_hint: u32) {
    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: accepted,
        dwWin32ExitCode: exit_code,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: 0,
        dwWaitHint: wait_hint,
    };
    let handle = STATUS_HANDLE.load(Ordering::SeqCst);
    if !handle.is_null() {
        unsafe {
            SetServiceStatus(handle, &status);
        }
    }
}

unsafe extern "system" fn control(
    code: u32,
    _event_type: u32,
    _event_data: *mut core::ffi::c_void,
    _context: *mut core::ffi::c_void,
) -> u32 {
    if code == SERVICE_CONTROL_STOP || code == SERVICE_CONTROL_SHUTDOWN {
        STOPPING.store(true, Ordering::SeqCst);
        report(SERVICE_STOP_PENDING, 0, NO_ERROR, 5000);
    }
    NO_ERROR
}

unsafe extern "system" fn service_main(_argc: u32, _argv: *mut *mut u16) {
    let name = wide(NAME);
    let handle = RegisterServiceCtrlHandlerExW(name.as_ptr(), Some(control), std::ptr::null_mut());
    if handle.is_null() {
        return;
    }
    STATUS_HANDLE.store(handle, Ordering::SeqCst);
    STOPPING.store(false, Ordering::SeqCst);
    report(SERVICE_START_PENDING, 0, NO_ERROR, 3000);
    report(
        SERVICE_RUNNING,
        SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN,
        NO_ERROR,
        0,
    );
    let result = crate::broker::serve_service();
    report(
        SERVICE_STOPPED,
        0,
        if result.is_ok() { NO_ERROR } else { 1 },
        0,
    );
}

pub fn run() -> Result<(), String> {
    let mut name = wide(NAME);
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: name.as_mut_ptr(),
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW {
            lpServiceName: std::ptr::null_mut(),
            lpServiceProc: None,
        },
    ];
    if unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) } == 0 {
        Err("Windows не запустила сетевую службу Atlas".into())
    } else {
        Ok(())
    }
}

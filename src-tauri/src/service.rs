use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use windows_sys::Win32::{
    Foundation::{
        GetLastError, LocalFree, ERROR_SERVICE_ALREADY_RUNNING, ERROR_SERVICE_DOES_NOT_EXIST,
        ERROR_SERVICE_EXISTS, NO_ERROR,
    },
    Security::{
        Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW,
        DACL_SECURITY_INFORMATION,
    },
    System::Services::{
        ChangeServiceConfigW, CloseServiceHandle, ControlService, CreateServiceW, DeleteService,
        OpenSCManagerW, OpenServiceW, RegisterServiceCtrlHandlerExW, SetServiceObjectSecurity,
        SetServiceStatus, StartServiceCtrlDispatcherW, StartServiceW, SC_MANAGER_CONNECT,
        SC_MANAGER_CREATE_SERVICE, SERVICE_ACCEPT_SHUTDOWN, SERVICE_ACCEPT_STOP,
        SERVICE_ALL_ACCESS, SERVICE_CHANGE_CONFIG, SERVICE_CONTROL_SHUTDOWN, SERVICE_CONTROL_STOP,
        SERVICE_DEMAND_START, SERVICE_ERROR_NORMAL, SERVICE_QUERY_STATUS, SERVICE_RUNNING,
        SERVICE_START, SERVICE_START_PENDING, SERVICE_STATUS, SERVICE_STOP, SERVICE_STOPPED,
        SERVICE_STOP_PENDING, SERVICE_TABLE_ENTRYW, SERVICE_WIN32_OWN_PROCESS,
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

fn windows_error(action: &str) -> String {
    format!("{action}: код Windows {}", unsafe { GetLastError() })
}

pub fn install() -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let command = wide(&format!(
        "\"{}\" --network-service",
        executable.to_string_lossy()
    ));
    let name = wide(NAME);
    let display_name = wide("Atlas Network Service");
    unsafe {
        let manager = OpenSCManagerW(
            std::ptr::null(),
            std::ptr::null(),
            SC_MANAGER_CONNECT | SC_MANAGER_CREATE_SERVICE,
        );
        if manager.is_null() {
            return Err(windows_error("Не удалось открыть диспетчер служб"));
        }
        let mut service = CreateServiceW(
            manager,
            name.as_ptr(),
            display_name.as_ptr(),
            SERVICE_ALL_ACCESS,
            SERVICE_WIN32_OWN_PROCESS,
            SERVICE_DEMAND_START,
            SERVICE_ERROR_NORMAL,
            command.as_ptr(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
        );
        if service.is_null() && GetLastError() == ERROR_SERVICE_EXISTS {
            service = OpenServiceW(
                manager,
                name.as_ptr(),
                SERVICE_CHANGE_CONFIG | SERVICE_START | SERVICE_QUERY_STATUS | 0x0004_0000,
            );
            if !service.is_null()
                && ChangeServiceConfigW(
                    service,
                    SERVICE_WIN32_OWN_PROCESS,
                    SERVICE_DEMAND_START,
                    SERVICE_ERROR_NORMAL,
                    command.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    display_name.as_ptr(),
                ) == 0
            {
                let message = windows_error("Не удалось обновить сетевую службу Atlas");
                CloseServiceHandle(service);
                CloseServiceHandle(manager);
                return Err(message);
            }
        }
        if service.is_null() {
            let message = windows_error("Не удалось создать сетевую службу Atlas");
            CloseServiceHandle(manager);
            return Err(message);
        }
        // Users may start/query this service, but cannot reconfigure its privileged binary.
        let mut descriptor = std::ptr::null_mut();
        let security = wide("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;LCRP;;;AU)");
        let converted = ConvertStringSecurityDescriptorToSecurityDescriptorW(
            security.as_ptr(),
            1,
            &mut descriptor,
            std::ptr::null_mut(),
        );
        let secured = converted != 0
            && SetServiceObjectSecurity(service, DACL_SECURITY_INFORMATION, descriptor) != 0;
        let failure = if secured {
            None
        } else {
            Some(windows_error("Права запуска службы"))
        };
        if !descriptor.is_null() {
            LocalFree(descriptor);
        }
        CloseServiceHandle(service);
        CloseServiceHandle(manager);
        if let Some(error) = failure {
            return Err(error);
        }
    }
    Ok(())
}

pub fn start_on_demand() -> Result<(), String> {
    unsafe {
        let manager = OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT);
        if manager.is_null() {
            return Err(windows_error("Диспетчер служб"));
        }
        let service = OpenServiceW(manager, wide(NAME).as_ptr(), SERVICE_START);
        if service.is_null() {
            let error = windows_error("Служба Atlas недоступна");
            CloseServiceHandle(manager);
            return Err(error);
        }
        let ok = StartServiceW(service, 0, std::ptr::null()) != 0
            || GetLastError() == ERROR_SERVICE_ALREADY_RUNNING;
        let result = if ok {
            Ok(())
        } else {
            Err(windows_error("Запуск службы Atlas"))
        };
        CloseServiceHandle(service);
        CloseServiceHandle(manager);
        result
    }
}

/// SCM is the authority for the service PID; querying a SYSTEM process directly
/// requires permissions that the desktop user deliberately does not have.
pub fn verify_server_pid(pid: u32) -> Result<(), String> {
    use windows_sys::Win32::System::Services::{
        QueryServiceStatusEx, SC_STATUS_PROCESS_INFO, SERVICE_STATUS_PROCESS,
    };
    unsafe {
        let manager = OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT);
        if manager.is_null() {
            return Err(windows_error("Диспетчер служб"));
        }
        let service = OpenServiceW(manager, wide(NAME).as_ptr(), SERVICE_QUERY_STATUS);
        if service.is_null() {
            let error = windows_error("Проверка службы Atlas");
            CloseServiceHandle(manager);
            return Err(error);
        }
        let mut status: SERVICE_STATUS_PROCESS = std::mem::zeroed();
        let mut needed = 0;
        let ok = QueryServiceStatusEx(
            service,
            SC_STATUS_PROCESS_INFO,
            &mut status as *mut _ as *mut u8,
            std::mem::size_of_val(&status) as u32,
            &mut needed,
        );
        let result = if ok == 0 {
            Err(windows_error("Проверка процесса службы Atlas"))
        } else if pid != 0 && status.dwProcessId == pid && status.dwCurrentState == SERVICE_RUNNING
        {
            Ok(())
        } else {
            Err("Канал не принадлежит запущенной службе Atlas".into())
        };
        CloseServiceHandle(service);
        CloseServiceHandle(manager);
        result
    }
}

pub fn uninstall() -> Result<(), String> {
    let name = wide(NAME);
    unsafe {
        let manager = OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT);
        if manager.is_null() {
            return Err(windows_error("Не удалось открыть диспетчер служб"));
        }
        let service = OpenServiceW(
            manager,
            name.as_ptr(),
            SERVICE_STOP | SERVICE_QUERY_STATUS | 0x0001_0000,
        );
        if service.is_null() {
            let missing = GetLastError() == ERROR_SERVICE_DOES_NOT_EXIST;
            CloseServiceHandle(manager);
            return if missing {
                Ok(())
            } else {
                Err(windows_error("Не удалось открыть сетевую службу Atlas"))
            };
        }
        let mut status: SERVICE_STATUS = std::mem::zeroed();
        let _ = ControlService(service, SERVICE_CONTROL_STOP, &mut status);
        if DeleteService(service) == 0 && GetLastError() != ERROR_SERVICE_DOES_NOT_EXIST {
            let message = windows_error("Не удалось удалить сетевую службу Atlas");
            CloseServiceHandle(service);
            CloseServiceHandle(manager);
            return Err(message);
        }
        CloseServiceHandle(service);
        CloseServiceHandle(manager);
    }
    Ok(())
}

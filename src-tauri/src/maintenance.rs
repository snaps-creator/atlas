use std::{path::{Path, PathBuf}, ptr, time::{Duration, Instant}};
use windows_sys::Win32::{
    Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0},
    System::{
        Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS},
        Threading::{OpenProcess, QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE},
        Services::*,
    },
};
const SYNCHRONIZE: u32 = 0x00100000;
fn wide(value: &str) -> Vec<u16> { value.encode_utf16().chain(Some(0)).collect() }
fn error(context: &str) -> String { format!("{context}: Windows {}", unsafe {GetLastError()}) }
struct Handle(HANDLE);
impl Drop for Handle { fn drop(&mut self) { unsafe {CloseHandle(self.0);} } }
struct Service(SC_HANDLE);
impl Drop for Service { fn drop(&mut self) { unsafe {CloseServiceHandle(self.0);} } }

fn comparable(path: &Path) -> String {
    path.to_string_lossy().trim_start_matches(r"\\?\").replace('/', "\\").to_lowercase()
}
fn owned_image(root: &Path, image: &Path, desktop: bool) -> bool {
    let names: &[&str] = if desktop { &["Atlas.exe"] }
        else { &["Atlas.Service.exe", "resources/Atlas.Core.exe", "resources/Atlas.Xray.exe"] };
    names.iter().any(|name| comparable(&root.join(name)) == comparable(image))
}
fn image(handle: HANDLE) -> Result<PathBuf, String> {
    let mut value = vec![0u16;32768];
    let mut len = value.len() as u32;
    if unsafe {QueryFullProcessImageNameW(handle, 0, value.as_mut_ptr(), &mut len)} == 0 {return Err(error("Process image"));}
    Ok(PathBuf::from(String::from_utf16_lossy(&value[..len as usize])))
}
fn running_image(handle: HANDLE) -> Result<Option<PathBuf>, String> {
    image_while_running(|| unsafe { WaitForSingleObject(handle, 0) == WAIT_OBJECT_0 }, || image(handle))
}
fn image_while_running(exited: impl Fn() -> bool, query: impl FnOnce() -> Result<PathBuf,String>) -> Result<Option<PathBuf>,String> {
    if exited() { return Ok(None); }
    match query() {
        Ok(path) => Ok(Some(path)),
        Err(_) if exited() => Ok(None),
        Err(error) => Err(error),
    }
}

/// Hold the verified process handle through termination/wait: PID reuse cannot
/// redirect termination to another process. A matching name alone is NEVER enough.
fn stop_owned(root: &Path, desktop: bool) -> Result<(), String> {
    let snapshot = unsafe {CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)};
    if snapshot == INVALID_HANDLE_VALUE {return Err(error("Process snapshot"));}
    let snapshot = Handle(snapshot);
    let mut entry: PROCESSENTRY32W = unsafe {std::mem::zeroed()};
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut handles = Vec::new();
    let mut more = unsafe {Process32FirstW(snapshot.0, &mut entry)};
    while more != 0 {
        let name = String::from_utf16_lossy(&entry.szExeFile).trim_end_matches('\0').to_lowercase();
        let candidate = if desktop {name == "atlas.exe"}
            else {matches!(name.as_str(), "atlas.service.exe" | "atlas.core.exe" | "atlas.xray.exe")};
        if candidate {
            let raw = unsafe {OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, 0, entry.th32ProcessID)};
            if raw.is_null() {
                // A raced exit is harmless; access denied is not proof of cleanup.
                if unsafe {GetLastError()} != 87 {return Err(error("Open Atlas candidate"));}
            } else {
                let _handle = Handle(raw);
                if running_image(raw)?.is_some_and(|path| owned_image(root, &path, desktop)) {
                    let target=unsafe {OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | SYNCHRONIZE,0,entry.th32ProcessID)};
                    if target.is_null() {
                        if unsafe {WaitForSingleObject(raw,0)} != WAIT_OBJECT_0 {return Err(error("Open owned Atlas process for shutdown"));}
                    } else {
                        let target=Handle(target);
                        // Keep the original handle alive while opening the termination
                        // handle; verify again before granting this target to shutdown.
                        if running_image(target.0)?.is_some_and(|path| owned_image(root,&path,desktop)) {handles.push(target);}
                    }
                }
            }
        }
        more = unsafe {Process32NextW(snapshot.0, &mut entry)};
    }
    // Signal every owned process before waiting, so N workers don't cost N timeouts.
    for handle in &handles {
        if unsafe {WaitForSingleObject(handle.0, 0)} != WAIT_OBJECT_0
            && unsafe {TerminateProcess(handle.0, 0)} == 0
            && unsafe {WaitForSingleObject(handle.0, 0)} != WAIT_OBJECT_0 {return Err(error("Stop Atlas process"));}
    }
    let deadline = Instant::now() + Duration::from_secs(4);
    for handle in handles {
        let left = deadline.saturating_duration_since(Instant::now()).as_millis() as u32;
        if unsafe {WaitForSingleObject(handle.0, left)} != WAIT_OBJECT_0 {return Err("Atlas process did not exit".into());}
    }
    Ok(())
}

fn service(root: &Path) -> Result<Option<Service>, String> {
    unsafe {
        let manager = OpenSCManagerW(ptr::null(), ptr::null(), SC_MANAGER_CONNECT);
        if manager.is_null() {return Err(error("Open SCM"));}
        let manager = Service(manager);
        let raw = OpenServiceW(manager.0, wide("AtlasNetworkService").as_ptr(), SERVICE_QUERY_CONFIG | SERVICE_QUERY_STATUS | SERVICE_STOP);
        if raw.is_null() {
            return if GetLastError() == 1060 {Ok(None)} else {Err(error("Open Atlas service"))};
        }
        let service = Service(raw);
        let mut size = 0;
        QueryServiceConfigW(raw, ptr::null_mut(), 0, &mut size);
        if size == 0 {return Err(error("Service config size"));}
        let mut storage = vec![0usize; (size as usize + std::mem::size_of::<usize>()-1)/std::mem::size_of::<usize>()];
        let config = storage.as_mut_ptr().cast::<QUERY_SERVICE_CONFIGW>();
        if QueryServiceConfigW(raw, config, size, &mut size) == 0 {return Err(error("Service config"));}
        let address = (*config).lpBinaryPathName;
        let mut len = 0;
        while *address.add(len) != 0 {len+=1;}
        let command = String::from_utf16_lossy(std::slice::from_raw_parts(address,len));
        // Registration always quotes the EXE. Do not accept prefix matches or an
        // unrelated service merely reusing Atlas' SCM name.
        let expected = format!("\"{}\" --network-service", root.join("Atlas.Service.exe").display());
        if !command.eq_ignore_ascii_case(&expected) {return Err("Atlas service belongs to another installation; files were not replaced".into());}
        Ok(Some(service))
    }
}
fn stop_service(service: &Service) -> Result<(), String> {
    unsafe {
        let mut status: SERVICE_STATUS = std::mem::zeroed();
        if QueryServiceStatus(service.0, &mut status) == 0 {return Err(error("Service status"));}
        if status.dwCurrentState == SERVICE_STOPPED {return Ok(());}
        if ControlService(service.0, SERVICE_CONTROL_STOP, &mut status) == 0 && GetLastError() != 1062 {return Err(error("Stop service"));}
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if QueryServiceStatus(service.0, &mut status) == 0 {return Err(error("Service status"));}
            if status.dwCurrentState == SERVICE_STOPPED {return Ok(());}
            std::thread::sleep(Duration::from_millis(50));
        }
        // The caller then terminates only exact verified installation images.
        Ok(())
    }
}

pub fn prepare(directory: &Path) -> Result<(), String> {
    // On a first installation there is no previous session at this location.
    if !directory.exists() {return Ok(());}
    let root = directory.canonicalize().map_err(|e|e.to_string())?;
    let root = PathBuf::from(root.to_string_lossy().trim_start_matches(r"\\?\"));
    let service = service(&root)?; // Validate ownership BEFORE changing anything.
    let _ = crate::network_guard::tun_identity(); // Keep exact driver evidence across teardown.
    stop_owned(&root, true)?; // Quiesce reconnect/start jobs before stopping service.
    let graceful = service.as_ref().map(stop_service).transpose();
    stop_owned(&root, false)?;
    // A worker may have been spawned just before its parent received termination.
    // With the service and desktop gone, a second snapshot is now stable.
    stop_owned(&root, false)?;
    if let Some(service) = &service {
        unsafe {
            let mut status: SERVICE_STATUS=std::mem::zeroed();
            let deadline=Instant::now()+Duration::from_secs(2);
            loop {
                if QueryServiceStatus(service.0,&mut status)==0 {return Err(error("Verify service stopped"));}
                if status.dwCurrentState==SERVICE_STOPPED {break;}
                if Instant::now()>=deadline {return Err("Atlas service has not stopped; installation was not started".into());}
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
    // Even a failed graceful stop is recovered by verified handle termination.
    let _ = graceful;
    let mut failures = Vec::new();
    let mut run = |result: Result<(),String>| {if let Err(e)=result {failures.push(e);}};
    run(crate::network_guard::wait_for_tun_release(Duration::from_secs(3)));
    run(crate::network_guard::clear_routes_for_reusable_tun().map(|_|()));
    run(crate::network_guard::clear());
    run(crate::windows::restore_loaded_profiles());
    #[link(name="dnsapi")]
    extern "system" {fn DnsFlushResolverCache() -> i32;}
    if unsafe {DnsFlushResolverCache()} == 0 {failures.push(error("Flush DNS after Atlas shutdown"));}
    if failures.is_empty() {Ok(())} else {Err(failures.join("; "))}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exit_during_image_query_is_not_an_ownership_failure() {
        let exited=std::cell::Cell::new(false);
        let result=image_while_running(||exited.get(), || {
            exited.set(true); Err("process exited during QueryFullProcessImageName".into())
        });
        assert_eq!(result,Ok(None));
        assert_eq!(image_while_running(||false,||Err("access denied on live process".into())),
            Err("access denied on live process".into()));
        assert_eq!(image_while_running(||true,||panic!("must not query a dead process")),Ok(None));
    }
    #[test]
    fn ownership_requires_exact_installation_and_role() {
        let root=Path::new(r"C:\Program Files\Atlas");
        assert!(owned_image(root,Path::new(r"c:\program files\ATLAS\Atlas.exe"),true));
        assert!(owned_image(root,Path::new(r"\\?\C:\Program Files\Atlas\resources\Atlas.Xray.exe"),false));
        for foreign in [r"C:\Other\Atlas.exe",r"C:\Program Files\Atlas-old\Atlas.exe",r"C:\Program Files\Atlas\resources\foreign.exe"] {
            assert!(!owned_image(root,Path::new(foreign),true));
            assert!(!owned_image(root,Path::new(foreign),false));
        }
        assert!(!owned_image(root,&root.join("Atlas.Service.exe"),true));
        assert!(!owned_image(root,&root.join("Atlas.exe"),false));
    }

    #[test]
    fn terminates_only_the_verified_fixture_leaving_same_named_foreign_process_alive() {
        use std::process::{Command, Stdio};
        let temp=std::env::temp_dir().join(format!("atlas-maintenance-test-{}",uuid::Uuid::new_v4()));
        let own=temp.join("own");
        let foreign=temp.join("foreign");
        std::fs::create_dir_all(&own).unwrap();
        std::fs::create_dir_all(&foreign).unwrap();
        let command=PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32/cmd.exe");
        for dir in [&own,&foreign] {std::fs::copy(&command,dir.join("Atlas.exe")).unwrap();}
        let spawn=|dir:&Path| Command::new(dir.join("Atlas.exe")).args(["/d","/q","/k"])
            .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let mut owned=spawn(&own);
        let mut other=spawn(&foreign);
        let result=stop_owned(&own,true);
        let owned_exited=owned.try_wait().unwrap().is_some();
        let foreign_alive=other.try_wait().unwrap().is_none();
        let _=owned.kill(); let _=owned.wait();
        let _=other.kill(); let _=other.wait();
        std::fs::remove_dir_all(&temp).unwrap();
        result.unwrap();
        assert!(owned_exited);
        assert!(foreign_alive);
    }
}

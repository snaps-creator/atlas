//! Launch the desktop with the same user's existing shell token. Never fall back
//! to an elevated desktop when Windows refuses the requested user context.
use std::{fs::File, os::windows::{ffi::OsStrExt,io::{AsRawHandle,FromRawHandle,OwnedHandle}},path::Path,ptr};
use windows_sys::Win32::{Foundation::*, Security::*, System::{Threading::*,Pipes::CreatePipe}};

pub struct Process { handle:OwnedHandle, pub pid:u32 }
impl AsRawHandle for Process {fn as_raw_handle(&self)->HANDLE{self.handle.as_raw_handle()}}
impl Process {pub fn terminate(&self){unsafe{TerminateProcess(self.as_raw_handle(),1);WaitForSingleObject(self.as_raw_handle(),5000);}}}
fn failure(action:&str)->String {format!("{action}: {}",std::io::Error::last_os_error())}
fn token()->Result<OwnedHandle,String>{unsafe{
    let mut handle=ptr::null_mut();
    if OpenProcessToken(GetCurrentProcess(),TOKEN_QUERY|TOKEN_DUPLICATE,&mut handle)==0{return Err(failure("Cannot inspect updater user"));}
    Ok(OwnedHandle::from_raw_handle(handle))
}}
fn elevation(handle:HANDLE)->Result<TOKEN_ELEVATION_TYPE,String>{unsafe{
    let mut value=0;let mut size=0;
    if GetTokenInformation(handle,TokenElevationType,(&mut value as *mut TOKEN_ELEVATION_TYPE).cast(),std::mem::size_of_val(&value) as u32,&mut size)==0 {
        return Err(failure("Cannot inspect updater elevation"));
    }
    Ok(value)
}}
fn sid(handle:HANDLE)->Result<String,String>{unsafe{
    let mut size=0;GetTokenInformation(handle,TokenUser,ptr::null_mut(),0,&mut size);
    if size==0||size>65536{return Err("Invalid user token size".into());}
    let mut buffer=vec![0usize;(size as usize).div_ceil(std::mem::size_of::<usize>())];
    if GetTokenInformation(handle,TokenUser,buffer.as_mut_ptr().cast(),size,&mut size)==0{return Err(failure("Cannot inspect updater SID"));}
    let user=&*buffer.as_ptr().cast::<TOKEN_USER>();let mut text=ptr::null_mut();
    if Authorization::ConvertSidToStringSidW(user.User.Sid,&mut text)==0{return Err(failure("Cannot format updater SID"));}
    let mut len=0;while *text.add(len)!=0{len+=1;}
    let result=String::from_utf16(std::slice::from_raw_parts(text,len)).map_err(|_|"Invalid user SID".into());
    LocalFree(text.cast());result
}}
pub fn current_sid()->Result<String,String>{sid(token()?.as_raw_handle())}
pub fn database()->Result<std::path::PathBuf,String>{unsafe{
    use windows_sys::Win32::{UI::Shell::{SHGetKnownFolderPath,FOLDERID_LocalAppData},System::Com::CoTaskMemFree};
    let mut raw=ptr::null_mut();
    if SHGetKnownFolderPath(&FOLDERID_LocalAppData,0,ptr::null_mut(),&mut raw)<0 || raw.is_null() {
        return Err("Cannot locate original user's application data".into());
    }
    let mut length=0;while *raw.add(length)!=0{length+=1;}
    let path=String::from_utf16(std::slice::from_raw_parts(raw,length)).map_err(|_|"Invalid application data path");
    CoTaskMemFree(raw.cast());
    Ok(std::path::PathBuf::from(path?).join("net.atlasvpn.desktop").join("atlas.db"))
}}
pub fn report()->Result<serde_json::Value,String>{Ok(serde_json::json!({
    "sid":current_sid()?,"pid":std::process::id(),"elevation":elevation(token()?.as_raw_handle())?,
    "version":env!("CARGO_PKG_VERSION")
}))}
pub fn check()->Result<serde_json::Value,String>{
    let executable=std::env::current_exe().map_err(|e|e.to_string())?;
    check_executable(&executable)
}
pub fn check_executable(executable:&Path)->Result<serde_json::Value,String>{
    use std::{io::Read,time::{Duration,Instant}};
    // The probe must execute the same native helper, never a caller-selected
    // program. NSIS extraction files intentionally cannot execute unelevated;
    // use the verified copy in the read/execute-only version directory instead.
    let own=std::env::current_exe().map_err(|e|e.to_string())?;
    if crate::update_transaction::digest(&own)?!=crate::update_transaction::digest(executable)? {
        return Err("User-context probe differs from the current installer helper".into());
    }
    let (child,mut pipe)=limited(&executable,&["--user-context-report"])?.ok_or("User-context check must be started elevated")?;
    let result=(||{
        let deadline=Instant::now()+Duration::from_secs(15);let mut bytes=Vec::new();
        loop {
            let mut available=0;
            unsafe{windows_sys::Win32::System::Pipes::PeekNamedPipe(pipe.as_raw_handle(),ptr::null_mut(),0,ptr::null_mut(),&mut available,ptr::null_mut());}
            if available>0 {
                if bytes.len()+available as usize>8192{return Err("User-context response exceeds limit".into());}
                let mut chunk=vec![0;available as usize];pipe.read_exact(&mut chunk).map_err(|e|e.to_string())?;bytes.extend(chunk);
            } else if unsafe{WaitForSingleObject(child.as_raw_handle(),0)}==WAIT_OBJECT_0 {break;}
            if Instant::now()>=deadline{return Err("User-context check timed out".into());}
            std::thread::sleep(Duration::from_millis(20));
        }
        let value:serde_json::Value=serde_json::from_slice(&bytes).map_err(|_|"User-context IPC did not return JSON")?;
        if value["pid"]!=child.pid || value["sid"]!=current_sid()? || value["elevation"]!=TokenElevationTypeLimited {
            return Err("Child user identity or elevation is incorrect".into());
        }
        Ok(value)
    })();
    if result.is_err(){child.terminate();}result
}

// Arguments are internal switches and UUIDs. Paths are passed independently as
// lpApplicationName and lpCurrentDirectory, never through a command interpreter.
pub fn limited(executable:&Path,args:&[&str])->Result<Option<(Process,File)>,String>{
    let current=token()?;
    if elevation(current.as_raw_handle())?!=TokenElevationTypeFull{return Ok(None);}
    if args.iter().any(|s|s.contains(['"','\\','\0','\r','\n',' '])) {return Err("Invalid desktop launch argument".into());}
    // Use a normal DOS spelling for Win32 process creation, even when callers
    // obtained an extended-length spelling from canonicalize.
    let normalized=std::path::PathBuf::from(executable.to_string_lossy().trim_start_matches(r"\\?\"));
    let executable=normalized.as_path();
    let executable_text=executable.to_str().ok_or("Invalid desktop executable path")?;
    if executable_text.contains(['"','\0']){return Err("Invalid desktop executable path".into());}
    let directory=executable.parent().ok_or("Desktop directory missing")?;
    unsafe {
        // TokenLinkedToken can be identification-only. The interactive shell
        // supplies a primary token; bind both SID and session before using it.
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetShellWindow,GetWindowThreadProcessId};
        let shell=GetShellWindow();let mut shell_pid=0;
        if shell.is_null() || GetWindowThreadProcessId(shell,&mut shell_pid)==0 || shell_pid==0 {
            return Err("Original user's interactive shell is unavailable".into());
        }
        let shell_process=OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION,0,shell_pid);
        if shell_process.is_null(){return Err(failure("Cannot inspect interactive shell"));}
        let shell_process=OwnedHandle::from_raw_handle(shell_process);
        let mut linked=ptr::null_mut();
        if OpenProcessToken(shell_process.as_raw_handle(),TOKEN_QUERY|TOKEN_DUPLICATE,&mut linked)==0 {
            return Err(failure("Cannot inspect interactive user's token"));
        }
        let linked=OwnedHandle::from_raw_handle(linked);
        if elevation(linked.as_raw_handle())?!=TokenElevationTypeLimited || sid(current.as_raw_handle())?!=sid(linked.as_raw_handle())? {
            return Err("Desktop token does not match the original limited user".into());
        }
        let mut current_session=0u32;let mut shell_session=0u32;let mut size=0;
        if GetTokenInformation(current.as_raw_handle(),TokenSessionId,(&mut current_session as *mut u32).cast(),4,&mut size)==0 ||
            GetTokenInformation(linked.as_raw_handle(),TokenSessionId,(&mut shell_session as *mut u32).cast(),4,&mut size)==0 || current_session!=shell_session {
            return Err("Desktop token belongs to another Windows session".into());
        }
        let mut primary=ptr::null_mut();
        if DuplicateTokenEx(linked.as_raw_handle(),TOKEN_QUERY|TOKEN_DUPLICATE|TOKEN_ASSIGN_PRIMARY|TOKEN_ADJUST_DEFAULT|TOKEN_ADJUST_SESSIONID,
            ptr::null(),SecurityImpersonation,TokenPrimary,&mut primary)==0 {
            return Err(failure("Cannot create original user's primary process token"));
        }
        let primary=OwnedHandle::from_raw_handle(primary);
        let security=SECURITY_ATTRIBUTES{nLength:std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,lpSecurityDescriptor:ptr::null_mut(),bInheritHandle:1};
        let(mut read,mut write)=(ptr::null_mut(),ptr::null_mut());
        if CreatePipe(&mut read,&mut write,&security,0)==0{return Err(failure("Cannot create desktop health channel"));}
        let read=File::from_raw_handle(read);let write=OwnedHandle::from_raw_handle(write);
        if SetHandleInformation(read.as_raw_handle(),HANDLE_FLAG_INHERIT,0)==0{return Err(failure("Cannot isolate health reader"));}
        let null=File::options().read(true).write(true).open("NUL").map_err(|e|format!("Cannot open desktop standard streams: {e}"))?;
        if SetHandleInformation(null.as_raw_handle(),HANDLE_FLAG_INHERIT,HANDLE_FLAG_INHERIT)==0{return Err(failure("Cannot prepare desktop standard streams"));}
        #[link(name="userenv")]
        extern "system" {fn CreateEnvironmentBlock(environment:*mut *mut core::ffi::c_void,token:HANDLE,inherit:i32)->i32;fn DestroyEnvironmentBlock(environment:*const core::ffi::c_void)->i32;}
        let mut environment=ptr::null_mut();
        if CreateEnvironmentBlock(&mut environment,primary.as_raw_handle(),0)==0{return Err(failure("Cannot create original user environment"));}
        let application:Vec<u16>=executable.as_os_str().encode_wide().chain(Some(0)).collect();
        let working:Vec<u16>=directory.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut command:Vec<u16>=format!("\"{}\" {}",executable_text,args.join(" ")).encode_utf16().chain(Some(0)).collect();
        let mut desktop:Vec<u16>="winsta0\\default".encode_utf16().chain(Some(0)).collect();
        let mut startup:STARTUPINFOW=std::mem::zeroed();startup.cb=std::mem::size_of_val(&startup) as u32;
        startup.lpDesktop=desktop.as_mut_ptr();startup.dwFlags=STARTF_USESTDHANDLES;
        startup.hStdInput=null.as_raw_handle();startup.hStdError=null.as_raw_handle();
        startup.hStdOutput=if args.iter().any(|a|matches!(*a,"--update-health"|"--user-context-report")){write.as_raw_handle()}else{null.as_raw_handle()};
        let mut process:PROCESS_INFORMATION=std::mem::zeroed();
        let success=CreateProcessWithTokenW(primary.as_raw_handle(),0,application.as_ptr(),command.as_mut_ptr(),
            CREATE_UNICODE_ENVIRONMENT|CREATE_NO_WINDOW,environment,working.as_ptr(),&startup,&mut process);
        let error=if success==0{Some(failure(&format!("Cannot launch Atlas as original user (directory {})",directory.display())))}else{None};
        DestroyEnvironmentBlock(environment);
        if let Some(error)=error{return Err(error);}
        CloseHandle(process.hThread);
        Ok(Some((Process{handle:OwnedHandle::from_raw_handle(process.hProcess),pid:process.dwProcessId},read)))
    }
}

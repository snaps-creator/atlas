//! Launch the desktop with the same user's limited token. Split UAC tokens use
//! the interactive shell; unsplit administrators use a verified restricted token.
//! Never fall back to an administrative desktop when token preparation fails.
use std::{fs::File, os::windows::{ffi::OsStrExt,io::{AsRawHandle,FromRawHandle,OwnedHandle}},path::Path,ptr};
use windows_sys::Win32::{Foundation::*, Security::*, System::{Threading::*,Pipes::CreatePipe}};
use windows_sys::Win32::System::SystemServices::{SE_GROUP_ENABLED,SE_GROUP_INTEGRITY,SECURITY_MANDATORY_MEDIUM_RID};

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
fn token_info(handle:HANDLE,class:TOKEN_INFORMATION_CLASS)->Result<Vec<usize>,String>{unsafe{
    let mut size=0;GetTokenInformation(handle,class,ptr::null_mut(),0,&mut size);
    if size==0 || size>65536{return Err("Invalid token information size".into());}
    let mut buffer=vec![0usize;(size as usize).div_ceil(std::mem::size_of::<usize>())];
    if GetTokenInformation(handle,class,buffer.as_mut_ptr().cast(),size,&mut size)==0{return Err(failure("Cannot inspect desktop token"));}
    Ok(buffer)
}}
fn integrity(handle:HANDLE)->Result<u32,String>{unsafe{
    let buffer=token_info(handle,TokenIntegrityLevel)?;
    let label=&*buffer.as_ptr().cast::<TOKEN_MANDATORY_LABEL>();
    if IsValidSid(label.Label.Sid)==0{return Err("Invalid token integrity SID".into());}
    let count=*GetSidSubAuthorityCount(label.Label.Sid);
    if count==0{return Err("Missing token integrity level".into());}
    Ok(*GetSidSubAuthority(label.Label.Sid,u32::from(count-1)))
}}
fn admin_enabled(handle:HANDLE)->Result<bool,String>{unsafe{
    let buffer=token_info(handle,TokenGroups)?;
    let groups=&*buffer.as_ptr().cast::<TOKEN_GROUPS>();
    for group in std::slice::from_raw_parts(groups.Groups.as_ptr(),groups.GroupCount as usize) {
        if IsWellKnownSid(group.Sid,WinBuiltinAdministratorsSid)!=0 && group.Attributes & SE_GROUP_ENABLED as u32 != 0 {return Ok(true);}
    }
    Ok(false)
}}
fn session(handle:HANDLE)->Result<u32,String>{
    let buffer=token_info(handle,TokenSessionId)?;Ok(unsafe{*buffer.as_ptr().cast::<u32>()})
}
fn desktop_safe(handle:HANDLE)->Result<bool,String>{Ok(!admin_enabled(handle)? && integrity(handle)?<=SECURITY_MANDATORY_MEDIUM_RID as u32)}
// CreateRestrictedToken retains the elevated token's default DACL. That DACL
// can give full access only to Administrators (now deny-only) and SYSTEM.
// Objects created by the desktop must instead be usable by its actual user;
// otherwise console children fail during initialization with 0xc0000142.
fn set_desktop_default_dacl(handle:HANDLE)->Result<(),String>{unsafe{
    let text:Vec<u16>=format!("D:(A;;GA;;;SY)(A;;GA;;;{})",sid(handle)?)
        .encode_utf16().chain(Some(0)).collect();
    let mut descriptor=ptr::null_mut();
    if Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW(text.as_ptr(),1,&mut descriptor,ptr::null_mut())==0 {
        return Err(failure("Cannot create desktop object permissions"));
    }
    let mut acl=ptr::null_mut();let mut present=0;let mut defaulted=0;
    let result=if GetSecurityDescriptorDacl(descriptor,&mut present,&mut acl,&mut defaulted)==0 || present==0 || acl.is_null() {
        Err(failure("Cannot inspect desktop object permissions"))
    }else{
        let value=TOKEN_DEFAULT_DACL{DefaultDacl:acl};
        if SetTokenInformation(handle,TokenDefaultDacl,(&value as *const TOKEN_DEFAULT_DACL).cast(),std::mem::size_of_val(&value) as u32)==0 {
            Err(failure("Cannot assign desktop object permissions"))
        }else{Ok(())}
    };
    LocalFree(descriptor);result
}}
fn restricted_desktop(current:HANDLE)->Result<OwnedHandle,String>{unsafe{
    // TokenElevationTypeDefault means "no linked token", not "non-admin".
    // Keep the user's SID/session while removing administrative groups and all
    // privileges except traversal. Do not synthesize a token for another user.
    let user=token_info(current,TokenUser)?;
    let user=&*user.as_ptr().cast::<TOKEN_USER>();
    if [WinLocalSystemSid,WinLocalServiceSid,WinNetworkServiceSid].iter().any(|kind|IsWellKnownSid(user.User.Sid,*kind)!=0) {
        return Err("A service account cannot be used as the original desktop user".into());
    }
    let mut duplicate=ptr::null_mut();
    if DuplicateTokenEx(current,TOKEN_ALL_ACCESS,ptr::null(),SecurityImpersonation,TokenPrimary,&mut duplicate)==0{return Err(failure("Cannot duplicate unsplit user token"));}
    let duplicate=OwnedHandle::from_raw_handle(duplicate);let mut restricted=ptr::null_mut();
    if CreateRestrictedToken(duplicate.as_raw_handle(),DISABLE_MAX_PRIVILEGE|LUA_TOKEN,0,ptr::null(),0,ptr::null(),0,ptr::null(),&mut restricted)==0{return Err(failure("Cannot restrict unsplit user token"));}
    let restricted=OwnedHandle::from_raw_handle(restricted);
    let text:Vec<u16>="S-1-16-8192".encode_utf16().chain(Some(0)).collect();let mut medium=ptr::null_mut();
    if Authorization::ConvertStringSidToSidW(text.as_ptr(),&mut medium)==0{return Err(failure("Cannot create medium integrity SID"));}
    let label=TOKEN_MANDATORY_LABEL{Label:SID_AND_ATTRIBUTES{Sid:medium,Attributes:SE_GROUP_INTEGRITY as u32}};
    let result=SetTokenInformation(restricted.as_raw_handle(),TokenIntegrityLevel,(&label as *const TOKEN_MANDATORY_LABEL).cast(),std::mem::size_of_val(&label) as u32+GetLengthSid(medium));
    let error=if result==0{Some(failure("Cannot lower desktop integrity"))}else{None};LocalFree(medium);
    if let Some(error)=error{return Err(error);}
    if !desktop_safe(restricted.as_raw_handle())? || sid(current)?!=sid(restricted.as_raw_handle())? || session(current)?!=session(restricted.as_raw_handle())? {
        return Err("Restricted desktop token failed identity or privilege validation".into());
    }
    set_desktop_default_dacl(restricted.as_raw_handle())?;
    Ok(restricted)
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
    "adminEnabled":admin_enabled(token()?.as_raw_handle())?,"integrity":integrity(token()?.as_raw_handle())?,"session":session(token()?.as_raw_handle())?,
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
        if value["pid"]!=child.pid || value["sid"]!=current_sid()? || value["adminEnabled"]!=false ||
            value["session"]!=session(token()?.as_raw_handle())? || !value["integrity"].as_u64().is_some_and(|level|level<=SECURITY_MANDATORY_MEDIUM_RID as u64) {
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
    if desktop_safe(current.as_raw_handle())? {return Ok(None);}
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
        let linked=if elevation(current.as_raw_handle())?==TokenElevationTypeDefault {
            restricted_desktop(current.as_raw_handle())?
        }else{
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
        if !desktop_safe(linked.as_raw_handle())? || sid(current.as_raw_handle())?!=sid(linked.as_raw_handle())? {
            return Err("Desktop token does not match the original limited user".into());
        }
        let mut current_session=0u32;let mut shell_session=0u32;let mut size=0;
        if GetTokenInformation(current.as_raw_handle(),TokenSessionId,(&mut current_session as *mut u32).cast(),4,&mut size)==0 ||
            GetTokenInformation(linked.as_raw_handle(),TokenSessionId,(&mut shell_session as *mut u32).cast(),4,&mut size)==0 || current_session!=shell_session {
            return Err("Desktop token belongs to another Windows session".into());
        }
        linked
        };
        launch_with_token(executable,directory,args,linked.as_raw_handle()).map(Some)
    }
}
fn launch_with_token(executable:&Path,directory:&Path,args:&[&str],linked:HANDLE)->Result<(Process,File),String>{unsafe{
        let executable_text=executable.to_str().ok_or("Invalid desktop executable path")?;
        let mut primary=ptr::null_mut();
        if DuplicateTokenEx(linked,TOKEN_QUERY|TOKEN_DUPLICATE|TOKEN_ASSIGN_PRIMARY|TOKEN_ADJUST_DEFAULT|TOKEN_ADJUST_SESSIONID,
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
        Ok((Process{handle:OwnedHandle::from_raw_handle(process.hProcess),pid:process.dwProcessId},read))
    }
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn restricted_token_keeps_user_and_session_without_administrative_access() {
        let current=token().unwrap();let reduced=restricted_desktop(current.as_raw_handle()).unwrap();
        assert_eq!(sid(current.as_raw_handle()).unwrap(),sid(reduced.as_raw_handle()).unwrap());
        assert_eq!(session(current.as_raw_handle()).unwrap(),session(reduced.as_raw_handle()).unwrap());
        assert!(!admin_enabled(reduced.as_raw_handle()).unwrap());
        assert_eq!(integrity(reduced.as_raw_handle()).unwrap(),SECURITY_MANDATORY_MEDIUM_RID as u32);
        assert!(desktop_safe(reduced.as_raw_handle()).unwrap());
        let buffer=token_info(reduced.as_raw_handle(),TokenPrivileges).unwrap();
        let privileges=unsafe{&*buffer.as_ptr().cast::<TOKEN_PRIVILEGES>()};
        // DISABLE_MAX_PRIVILEGE removes every privilege except traversal.
        let mut traversal=LUID{LowPart:0,HighPart:0};
        let name:Vec<u16>="SeChangeNotifyPrivilege".encode_utf16().chain(Some(0)).collect();
        assert_ne!(unsafe{LookupPrivilegeValueW(ptr::null(),name.as_ptr(),&mut traversal)},0);
        for privilege in unsafe{std::slice::from_raw_parts(privileges.Privileges.as_ptr(),privileges.PrivilegeCount as usize)} {
            assert_eq!((privilege.Luid.LowPart,privilege.Luid.HighPart),(traversal.LowPart,traversal.HighPart));
        }
    }
    #[test] fn restricted_default_dacl_grants_its_user_full_access() {
        let current=token().unwrap();let reduced=restricted_desktop(current.as_raw_handle()).unwrap();
        let info=token_info(reduced.as_raw_handle(),TokenDefaultDacl).unwrap();
        let user=token_info(reduced.as_raw_handle(),TokenUser).unwrap();
        unsafe {
            let acl=(*info.as_ptr().cast::<TOKEN_DEFAULT_DACL>()).DefaultDacl;
            assert!(!acl.is_null());assert_ne!(IsValidAcl(acl),0);
            let user_sid=(*user.as_ptr().cast::<TOKEN_USER>()).User.Sid;
            let mut grants_user=false;
            for index in 0..u32::from((*acl).AceCount) {
                let mut ace=ptr::null_mut();assert_ne!(GetAce(acl,index,&mut ace),0);
                let ace=&*ace.cast::<ACCESS_ALLOWED_ACE>();
                assert_eq!(ace.Header.AceType,0); // ACCESS_ALLOWED_ACE_TYPE
                let target=(&ace.SidStart as *const u32).cast_mut().cast();
                let is_user=EqualSid(target,user_sid)!=0;
                assert!(is_user || IsWellKnownSid(target,WinLocalSystemSid)!=0);
                assert_eq!(ace.Mask,GENERIC_ALL);
                grants_user|=is_user;
            }
            assert!(grants_user,"Desktop objects must not rely on the disabled Administrators group");
        }
    }
    #[test] #[ignore="Invoked only in a restricted grandchild"]
    fn restricted_leaf_driver() {
        assert!(desktop_safe(token().unwrap().as_raw_handle()).unwrap());
    }
    #[test] #[ignore="Invoked in a real restricted child by restricted_process_runs_without_admin"]
    fn restricted_child_driver() {
        // CreateProcessWithTokenW does not carry the parent's error mode.
        #[link(name="kernel32")] extern "system" {fn SetErrorMode(mode:u32)->u32;}
        unsafe{SetErrorMode(3);}
        use std::os::windows::process::CommandExt;
        let current=token().unwrap();
        assert!(!admin_enabled(current.as_raw_handle()).unwrap());
        assert_eq!(integrity(current.as_raw_handle()).unwrap(),SECURITY_MANDATORY_MEDIUM_RID as u32);
        let exe=std::env::current_exe().unwrap();let root=exe.parent().unwrap();
        assert!(File::options().write(true).open(&exe).is_err(),"Installation must remain read/execute only");
        // Exercise both PE subsystems from the protected installation. Checking
        // only the GUI parent's token misses console initialization failures.
        for name in ["gui.exe","console.exe"] {
            let mut child=std::process::Command::new(root.join(name))
                .args(["--exact","update_user::tests::restricted_leaf_driver","--ignored"])
                .creation_flags(CREATE_NO_WINDOW)
                .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
                .spawn().unwrap_or_else(|e|panic!("Cannot start {name}: {e}"));
            let status=unsafe{WaitForSingleObject(child.as_raw_handle(),10000)};
            if status!=WAIT_OBJECT_0{let _=child.kill();let _=child.wait();panic!("{name} timed out");}
            assert!(child.wait().unwrap().success(),"{name} failed initialization or token validation");
        }
    }
    #[test] #[ignore="Requires an administrative caller; mandatory separate CI invocation"]
    fn restricted_process_runs_without_admin() {
        // Suppress Windows error dialogs in unattended failure-path tests.
        #[link(name="kernel32")] extern "system" {fn SetErrorMode(mode:u32)->u32;}
        struct Fixture(std::path::PathBuf,u32);
        impl Drop for Fixture {fn drop(&mut self){let _=std::fs::remove_dir_all(&self.0);unsafe{SetErrorMode(self.1);}}}
        let current=token().unwrap();assert!(admin_enabled(current.as_raw_handle()).unwrap(),"Administrative test caller required");
        let stamp=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root=std::env::temp_dir().join(format!("atlas-token-test-{}-{stamp}",std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let _fixture=Fixture(root.clone(),unsafe{SetErrorMode(3)});
        crate::update_windows::protect_installation(&root).unwrap();
        let mut bytes=std::fs::read(std::env::current_exe().unwrap()).unwrap();
        let pe=u32::from_le_bytes(bytes[60..64].try_into().unwrap()) as usize;
        assert_eq!(&bytes[pe..pe+4],b"PE\0\0");
        let subsystem=pe+24+68;
        bytes[subsystem..subsystem+2].copy_from_slice(&2u16.to_le_bytes());
        std::fs::write(root.join("gui.exe"),&bytes).unwrap();
        bytes[subsystem..subsystem+2].copy_from_slice(&3u16.to_le_bytes());
        std::fs::write(root.join("console.exe"),&bytes).unwrap();
        for _ in 0..2 {
            let reduced=restricted_desktop(current.as_raw_handle()).unwrap();
            let (child,_pipe)=launch_with_token(&root.join("gui.exe"),&root,
                &["--exact","update_user::tests::restricted_child_driver","--ignored"],reduced.as_raw_handle()).unwrap();
            let status=unsafe{WaitForSingleObject(child.as_raw_handle(),25000)};
            if status!=WAIT_OBJECT_0{child.terminate();panic!("Restricted child timed out");}
            let mut exit=1;assert_ne!(unsafe{GetExitCodeProcess(child.as_raw_handle(),&mut exit)},0);
            assert_eq!(exit,0,"Restricted desktop failed its child-process assertions");
        }
    }
}

//! SCM switching changes only ImagePath. Every other service configuration field
//! is compared with the durable snapshot before and after the operation.
use serde::{Deserialize, Serialize};
use std::{path::Path, ptr};
use windows_sys::Win32::System::Services::*;
type Result<T> = std::result::Result<T, String>;
fn wide(s:&str)->Vec<u16>{s.encode_utf16().chain(Some(0)).collect()}
fn error()->String{format!("Updater SCM operation failed: {}",std::io::Error::last_os_error())}
struct Handle(SC_HANDLE);
impl Drop for Handle{fn drop(&mut self){unsafe{CloseServiceHandle(self.0);}}}
#[derive(Clone,Debug,Serialize,Deserialize,PartialEq,Eq)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    pub binary:String,pub service_type:u32,pub start_type:u32,pub error_control:u32,
    pub account:String,pub display:String,pub group:String,pub dependencies:Vec<String>,pub tag:u32,
}
impl Configuration {
    pub fn with_binary(&self,binary:String)->Self{Self{binary,..self.clone()}}
    pub fn require_switch(&self,actual:&Self,allowed:&[String])->Result<()> {
        if !allowed.iter().any(|s|s.eq_ignore_ascii_case(&actual.binary)) ||
            self.with_binary(actual.binary.clone())!=*actual {
            return Err("Service configuration changed outside this transaction".into());
        }
        Ok(())
    }
}
pub fn command(root:&Path)->Result<String>{
    if !root.is_absolute() || root.to_string_lossy().contains(['"','\r','\n']) {return Err("Invalid service installation path".into());}
    let text=root.to_string_lossy();
    let normal=Path::new(text.trim_start_matches(r"\\?\"));
    Ok(format!("\"{}\" --network-service",normal.join("Atlas.Service.exe").display()))
}
pub struct Service(Handle);
impl Service {
    pub fn exists()->Result<bool>{unsafe{
        let manager=Handle(OpenSCManagerW(ptr::null(),ptr::null(),SC_MANAGER_CONNECT));
        if manager.0.is_null(){return Err(error());}
        let handle=OpenServiceW(manager.0,wide("AtlasNetworkService").as_ptr(),SERVICE_QUERY_STATUS);
        if !handle.is_null(){drop(Handle(handle));return Ok(true);}
        let code=std::io::Error::last_os_error();
        if code.raw_os_error()==Some(1060){Ok(false)}else{Err(format!("Cannot inspect Atlas service: {code}"))}
    }}
    pub fn open(writable:bool)->Result<Self>{unsafe{
        let manager=Handle(OpenSCManagerW(ptr::null(),ptr::null(),SC_MANAGER_CONNECT));
        if manager.0.is_null(){return Err(error());}
        let rights=SERVICE_QUERY_CONFIG|SERVICE_QUERY_STATUS|if writable{SERVICE_CHANGE_CONFIG|SERVICE_START|SERVICE_STOP}else{0};
        let handle=OpenServiceW(manager.0,wide("AtlasNetworkService").as_ptr(),rights);
        if handle.is_null(){return Err(error());}Ok(Self(Handle(handle)))
    }}
    pub fn configuration(&self)->Result<Configuration>{unsafe{
        let mut bytes=0;QueryServiceConfigW(self.0.0,ptr::null_mut(),0,&mut bytes);
        if bytes==0 || bytes>1024*1024{return Err("Invalid SCM configuration size".into());}
        let mut storage=vec![0usize;(bytes as usize).div_ceil(std::mem::size_of::<usize>())];
        let p=storage.as_mut_ptr().cast::<QUERY_SERVICE_CONFIGW>();
        if QueryServiceConfigW(self.0.0,p,bytes,&mut bytes)==0{return Err(error());}
        let lower=storage.as_ptr() as usize;let upper=lower+storage.len()*std::mem::size_of::<usize>();
        let string=|p:*const u16|->Result<(String,usize)>{
            if p.is_null(){return Ok((String::new(),0));}
            let address=p as usize;
            if address<lower || address>=upper || address%2!=0{return Err("SCM string outside returned buffer".into());}
            let slice=std::slice::from_raw_parts(p,(upper-address)/2);
            let end=slice.iter().position(|v|*v==0).ok_or("Unterminated SCM string")?;
            Ok((String::from_utf16(slice.get(..end).unwrap()).map_err(|_|"Invalid SCM Unicode")?,end+1))
        };
        let mut dependencies=Vec::new();let mut dep=(*p).lpDependencies;
        if !dep.is_null(){loop{let(value,count)=string(dep)?;if value.is_empty(){break;}dependencies.push(value);dep=dep.add(count);}}
        Ok(Configuration{binary:string((*p).lpBinaryPathName)?.0,service_type:(*p).dwServiceType,start_type:(*p).dwStartType,error_control:(*p).dwErrorControl,
            account:string((*p).lpServiceStartName)?.0,display:string((*p).lpDisplayName)?.0,group:string((*p).lpLoadOrderGroup)?.0,dependencies,tag:(*p).dwTagId})
    }}
    pub fn status(&self)->Result<SERVICE_STATUS_PROCESS>{unsafe{
        let mut status:SERVICE_STATUS_PROCESS=std::mem::zeroed();let mut size=0;
        if QueryServiceStatusEx(self.0.0,SC_STATUS_PROCESS_INFO,(&mut status as *mut SERVICE_STATUS_PROCESS).cast(),std::mem::size_of_val(&status) as u32,&mut size)==0{return Err(error());}Ok(status)
    }}
    pub fn stop(&self, timeout:std::time::Duration)->Result<()> {
        let deadline=std::time::Instant::now()+timeout;
        loop {
            let status=self.status()?;
            match status.dwCurrentState {
                SERVICE_STOPPED=>return Ok(()),
                SERVICE_STOP_PENDING|SERVICE_START_PENDING=>{},
                SERVICE_RUNNING|SERVICE_PAUSED=>unsafe {
                    let mut reply:SERVICE_STATUS=std::mem::zeroed();
                    if ControlService(self.0.0,SERVICE_CONTROL_STOP,&mut reply)==0 {
                        let code=std::io::Error::last_os_error();
                        if code.raw_os_error()!=Some(1062){return Err(format!("Cannot stop Atlas service: {code}"));}
                    }
                },
                state=>return Err(format!("Cannot stop Atlas service in SCM state {state}")),
            }
            if std::time::Instant::now()>=deadline{return Err("Atlas service stop timed out; binaries were not switched".into());}
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    pub fn remove()->Result<()>{unsafe{
        let manager=Handle(OpenSCManagerW(ptr::null(),ptr::null(),SC_MANAGER_CONNECT));
        if manager.0.is_null(){return Err(error());}
        let handle=OpenServiceW(manager.0,wide("AtlasNetworkService").as_ptr(),SERVICE_QUERY_STATUS|0x00010000);
        if handle.is_null(){
            let code=std::io::Error::last_os_error();
            return if code.raw_os_error()==Some(1060){Ok(())}else{Err(format!("Cannot open Atlas service for removal: {code}"))};
        }
        let service=Self(Handle(handle));
        if service.status()?.dwCurrentState!=SERVICE_STOPPED{return Err("Atlas service must be stopped before removal".into());}
        if DeleteService(service.0.0)==0{return Err(error());}Ok(())
    }}
    pub fn start(&self, timeout:std::time::Duration)->Result<u32> {
        let deadline=std::time::Instant::now()+timeout;
        let mut requested=false;
        loop {
            let status=self.status()?;
            match status.dwCurrentState {
                SERVICE_RUNNING if status.dwProcessId!=0=>return Ok(status.dwProcessId),
                SERVICE_STOPPED if !requested=>unsafe {
                    if StartServiceW(self.0.0,0,ptr::null())==0 {
                        let code=std::io::Error::last_os_error();
                        if code.raw_os_error()!=Some(1056){return Err(format!("Cannot start Atlas service: {code}"));}
                    }
                    requested=true;
                },
                SERVICE_STOPPED=>return Err(format!("Atlas service exited during startup: win32={}, service={}",status.dwWin32ExitCode,status.dwServiceSpecificExitCode)),
                SERVICE_START_PENDING|SERVICE_STOP_PENDING=>{},
                state=>return Err(format!("Cannot start Atlas service in SCM state {state}")),
            }
            if std::time::Instant::now()>=deadline{return Err("Atlas service start timed out".into());}
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    pub fn switch(&self,snapshot:&Configuration,allowed:&[String],target:&str)->Result<()> {
        if !allowed.iter().any(|s|s==target){return Err("Service target outside transaction".into());}
        snapshot.require_switch(&self.configuration()?,allowed)?;
        if self.status()?.dwCurrentState!=SERVICE_STOPPED{return Err("Service must be stopped before ImagePath switch".into());}
        unsafe{if ChangeServiceConfigW(self.0.0,SERVICE_NO_CHANGE,SERVICE_NO_CHANGE,SERVICE_NO_CHANGE,wide(target).as_ptr(),ptr::null(),ptr::null_mut(),ptr::null(),ptr::null(),ptr::null(),ptr::null())==0{return Err(error());}}
        let actual=self.configuration()?;
        if actual!=snapshot.with_binary(target.into()){return Err("SCM switch verification failed".into());}Ok(())
    }
}
#[cfg(test)]mod tests{
    use super::*;
    #[test]fn switch_contract_preserves_every_non_path_field(){
        let old=Configuration{binary:"old".into(),service_type:16,start_type:3,error_control:1,account:"LocalSystem".into(),display:"Atlas".into(),group:String::new(),dependencies:vec!["Tcpip".into()],tag:0};
        let allowed=vec!["old".into(),"new".into()];
        assert!(old.require_switch(&old.with_binary("new".into()),&allowed).is_ok());
        assert!(old.require_switch(&old.with_binary("foreign".into()),&allowed).is_err());
        let mut changed=old.clone();changed.start_type=2;assert!(old.require_switch(&changed,&allowed).is_err());
        let mut changed=old.clone();changed.account="other".into();assert!(old.require_switch(&changed,&allowed).is_err());
        let mut changed=old.clone();changed.dependencies.clear();assert!(old.require_switch(&changed,&allowed).is_err());
        assert!(command(Path::new("relative")).is_err());assert!(command(Path::new("C:\\Atlas\"bad")).is_err());
    }
}

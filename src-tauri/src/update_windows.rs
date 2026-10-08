//! Windows effects for the durable update supervisor. No SCM or user-data
//! operation is inferred from a journal stage without inspecting Windows.
use crate::{update_candidate::Candidate, update_service::{Service,Configuration},
    update_supervisor::{Platform,Point}, update_transaction::{Transaction,Version}};
use std::{path::PathBuf, time::Duration};
use serde::{Deserialize,Serialize};

#[derive(Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
struct State { service:Option<Configuration>, database:PathBuf, user_sid:String, snapshot_complete:bool }

pub fn protect_installation(directory:&std::path::Path)->Result<(),String>{protect_directory(directory,true)}
fn protect_snapshot(directory:&std::path::Path)->Result<(),String>{protect_directory(directory,false)}
fn protect_directory(directory:&std::path::Path,readable:bool)->Result<(),String>{
    use std::os::windows::{ffi::OsStrExt,fs::MetadataExt};
    use windows_sys::Win32::{Foundation::LocalFree,Security::{Authorization::*,*}};
    let metadata=std::fs::symlink_metadata(directory).map_err(|e|e.to_string())?;
    if !metadata.is_dir() || metadata.file_attributes()&0x400!=0 {return Err("Update snapshot directory is a reparse point or non-directory".into());}
    for ancestor in directory.ancestors(){
        let metadata=std::fs::symlink_metadata(ancestor).map_err(|e|e.to_string())?;
        if metadata.file_attributes()&0x400!=0{return Err("Reparse point in installation ancestry".into());}
    }
    let permissions=if readable{"O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;GRGX;;;AU)"}else{"O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)"};
    let sddl:Vec<u16>=permissions.encode_utf16().chain(Some(0)).collect();
    let name:Vec<u16>=directory.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        let mut descriptor=std::ptr::null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(),1,&mut descriptor,std::ptr::null_mut())==0 {
            return Err("Cannot create private update snapshot permissions".into());
        }
        let mut present=0;let mut defaulted=0;let mut acl=std::ptr::null_mut();let mut owner=std::ptr::null_mut();
        let result=if GetSecurityDescriptorDacl(descriptor,&mut present,&mut acl,&mut defaulted)==0 || present==0 ||
            GetSecurityDescriptorOwner(descriptor,&mut owner,&mut defaulted)==0 || owner.is_null() {
            Err("Cannot inspect private update snapshot permissions".into())
        }else{
            let code=SetNamedSecurityInfoW(name.as_ptr(),SE_FILE_OBJECT,OWNER_SECURITY_INFORMATION|DACL_SECURITY_INFORMATION|PROTECTED_DACL_SECURITY_INFORMATION,
                owner,std::ptr::null_mut(),acl,std::ptr::null());
            if code==0{Ok(())}else{Err(format!("Cannot protect update snapshot: Windows {code}"))}
        };
        LocalFree(descriptor);result
    }
}

pub struct Windows {
    root:PathBuf,
    directory:PathBuf,
    state:State,
    previous:Version,
    candidate:Version,
    desktop:Option<Candidate>,
    service_pid:Option<u32>,
}
impl Windows {
    pub fn prepare(tx:&Transaction,database:PathBuf)->Result<Self,String> {
        let user_sid=crate::update_user::current_sid()?;
        if database!=crate::update_user::database()? {return Err("Settings snapshot must belong to the initiating Windows user".into());}
        let directory=tx.root().join(format!("state-{}",tx.journal.transaction_id));
        std::fs::create_dir_all(&directory).map_err(|e|format!("Cannot create update snapshot directory: {e}"))?;
        protect_snapshot(&directory)?;
        let record=directory.join("windows.json");
        let state=if record.exists() {
            let state:State=serde_json::from_slice(&crate::update_integrity::read_bounded(&record,65536)?)
                .map_err(|_|"Invalid Windows update snapshot")?;
            if state.database!=database || state.user_sid!=user_sid{return Err("Update user-data identity changed".into());}
            state
        } else {
            if tx.journal.stage!=crate::update_transaction::Stage::Prepared {
                return Err("Missing Windows snapshot for interrupted update".into());
            }
            let service=if Service::exists()?{Some(Service::open(false)?.configuration()?)}else{None};
            if service.as_ref().map(|s|s.binary.as_str()).unwrap_or("")!=tx.journal.previous_service_path {
                return Err("Installed service changed before update preparation".into());
            }
            if service.is_none()!=tx.journal.previous.absent{return Err("Service presence differs from previous installation state".into());}
            let state=State{service,database,user_sid,snapshot_complete:false};
            crate::update_transaction::durable_write(&record,&serde_json::to_vec(&state).map_err(|e|e.to_string())?)?;
            state
        };
        Ok(Self{root:tx.root().into(),directory,state,previous:tx.journal.previous.clone(),
            candidate:tx.journal.candidate.clone(),desktop:None,service_pid:None})
    }
    fn path(&self,version:&Version)->PathBuf {self.root.join("versions").join(&version.id)}
    fn service_directory(&self,version:&Version)->Result<PathBuf,String>{
        // A first migration can fail before the legacy entry was replaced.
        // In that case its original flat desktop and service remain a pair.
        if version.id==self.previous.id && self.state.service.as_ref().is_some_and(|s|crate::update_service::command(&self.root).is_ok_and(|c|s.binary.eq_ignore_ascii_case(&c))) {
            if let Some(desktop)=version.files.iter().find(|f|f.path=="Atlas.exe") {
                if crate::update_transaction::digest(&self.root.join("Atlas.exe")).is_ok_and(|(size,hash)|size==desktop.size&&hash==desktop.sha256) {
                    return Ok(self.root.clone());
                }
            }
        }
        Ok(self.path(version))
    }
    // Validate all durable SCM fields before allowing a recovery start. Never
    // repair a foreign service by changing its ImagePath or account here.
    fn configured_service(&self,version:&Version,writable:bool)->Result<Option<Service>,String>{
        if version.absent{return if Service::exists()?{Err("Rollback left a newly created Atlas service".into())}else{Ok(None)};}
        let service=Service::open(writable)?;
        let configuration=service.configuration()?;
        let expected=crate::update_service::command(&self.service_directory(version)?)?;
        if self.state.service.as_ref().is_some_and(|snapshot|configuration!=snapshot.with_binary(expected.clone())) || configuration.binary!=expected {
            return Err("Active service configuration differs from the transaction".into());
        }
        if self.state.service.is_none() && (configuration.service_type!=16 || configuration.start_type!=3 || !configuration.account.eq_ignore_ascii_case("LocalSystem")) {
            return Err("New Atlas service has unexpected account or start configuration".into());
        }
        Ok(Some(service))
    }
    fn roots(&self)->Vec<PathBuf>{vec![self.root.clone(),self.path(&self.previous),self.path(&self.candidate)]}
    fn allowed(&self)->Result<Vec<String>,String>{self.roots().iter().map(|p|crate::update_service::command(p)).collect()}
}
impl Platform for Windows {
    fn checkpoint(&mut self,point:Point)->Result<(),String>{
        if point==Point::BeforeSwitch {
            let source=self.path(&self.candidate).join("AtlasUpdater.exe");
            let bytes=crate::update_integrity::read_bounded(&source,64*1024*1024)?;
            // Replace the legacy desktop entry first: it must become subject to
            // the durable admission fence before service activation can begin.
            for name in ["Atlas.exe","AtlasUpdater.exe"] {
                let destination=self.root.join(name);
                if destination.exists() && crate::update_transaction::digest(&destination)?==crate::update_transaction::digest(&source)?{continue;}
                crate::update_transaction::durable_write(&destination,&bytes)?;
            }
        }
        Ok(())
    }
    fn snapshot_data(&mut self,_:&Transaction)->Result<(),String>{
        crate::update_data::save(&self.state.database,&self.directory)?;
        self.state.snapshot_complete=true;
        crate::update_transaction::durable_write(&self.directory.join("windows.json"),
            &serde_json::to_vec(&self.state).map_err(|e|e.to_string())?)
    }
    fn quiesce(&mut self,_:&Version)->Result<(),String>{
        self.desktop.take();
        self.service_pid=None;
        if let Some(service)=&self.state.service {service.require_switch(&Service::open(false)?.configuration()?,&self.allowed()?)?;}
        crate::maintenance::prepare_transaction(&self.roots())
    }
    fn inspect_network_baseline(&mut self)->Result<(),String>{
        crate::network_guard::wait_for_clean_baseline(Duration::from_secs(5))
    }
    fn activate_service(&mut self,version:&Version)->Result<(),String>{
        if version.absent{return Service::remove();}
        if self.state.service.is_none() && !Service::exists()? {
            crate::update_process::status(&self.path(version).join("Atlas.exe"),&["--install-service"],Duration::from_secs(30))?;
        }
        let service=Service::open(true)?;
        service.stop(Duration::from_secs(30))?;
        let command=crate::update_service::command(&self.service_directory(version)?)?;
        if let Some(snapshot)=&self.state.service {service.switch(snapshot,&self.allowed()?,&command)?;}
        else if service.configuration()?.binary!=command{return Err("Fresh service image differs from candidate".into());}
        self.service_pid=Some(service.start(Duration::from_secs(30))?);
        Ok(())
    }
    fn inspect_service(&mut self,version:&Version)->Result<(),String>{
        let Some(service)=self.configured_service(version,false)? else {return Ok(());};
        let status=service.status()?;
        if status.dwCurrentState!=windows_sys::Win32::System::Services::SERVICE_RUNNING || status.dwProcessId==0 {
            return Err(format!("Active Atlas service is not running: state={}, pid={}, win32={}, service={}",status.dwCurrentState,status.dwProcessId,status.dwWin32ExitCode,status.dwServiceSpecificExitCode));
        }
        if self.desktop.as_ref().is_some_and(|desktop|desktop.report.service_pid!=status.dwProcessId) {
            return Err("Atlas service restarted during candidate health verification".into());
        }
        if self.service_pid.is_some_and(|pid|pid!=status.dwProcessId) {
            return Err("Atlas service process changed during activation".into());
        }
        Ok(())
    }
    fn resume_service(&mut self,version:&Version)->Result<(),String>{
        let Some(service)=self.configured_service(version,true)? else {return Ok(());};
        // start() preserves a running PID and waits through an idle STOP_PENDING.
        // Activation/health keep their stricter uninterrupted-PID checks.
        self.service_pid=Some(service.start(Duration::from_secs(30))?);
        self.inspect_service(version)
    }
    fn health(&mut self,tx:&Transaction)->Result<(),String>{
        let version=&tx.journal.candidate;
        // An on-demand service may legitimately reach its idle timeout while
        // CEF is still opening. Bind the stability check to the authenticated
        // UI handshake's PID, not a pre-client startup PID. The handshake itself
        // verifies that reconnect preserves this process and network epoch.
        self.service_pid=None;
        self.desktop=Some(Candidate::launch(&self.path(version).join("Atlas.exe"),
            &tx.journal.transaction_id,&uuid::Uuid::new_v4().to_string(),&version.version,&version.build)?);
        self.service_pid=self.desktop.as_ref().map(|desktop|desktop.report.service_pid);
        self.inspect_service(version)
    }
    fn restore_data(&mut self,_:&Transaction)->Result<(),String>{
        // Before snapshot publication the candidate has never been admitted.
        // A failed or interrupted VACUUM therefore leaves the original DB valid.
        if self.state.snapshot_complete || self.directory.join("settings-snapshot.json").exists() {
            crate::update_data::restore(&self.state.database,&self.directory)?;
        }
        Ok(())
    }
    fn launch_previous(&mut self,version:&Version)->Result<(),String>{
        if version.absent{return Ok(());}
        use std::process::{Command,Stdio};
        let directory=self.service_directory(version)?;
        if let Some((child,pipe))=crate::update_user::limited(&directory.join("Atlas.exe"),&[])? {
            drop(pipe);drop(child);return Ok(());
        }
        Command::new(directory.join("Atlas.exe")).current_dir(directory)
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
            .map_err(|e|format!("Cannot restart restored Atlas: {e}"))?;
        Ok(())
    }
    fn committed(&mut self){if let Some(desktop)=&mut self.desktop{desktop.commit();}}
}

//! Machine-wide synchronization shared by update, repair and session admission.
use windows_sys::Win32::{Foundation::{CloseHandle, LocalFree, HANDLE, WAIT_OBJECT_0, WAIT_ABANDONED}, Security::{Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW, SECURITY_ATTRIBUTES}, System::Threading::{CreateMutexExW, ReleaseMutex, WaitForSingleObject}};
pub struct UpdateLock(HANDLE);
impl Drop for UpdateLock {fn drop(&mut self){unsafe{ReleaseMutex(self.0);CloseHandle(self.0);}}}
impl UpdateLock {
    pub fn acquire(timeout_ms:u32)->Result<Self,String> {Self::named("Global\\Atlas.Update.Transaction.v1",timeout_ms)}
    /// An abandoned mutex is permission to recover, never permission to start a
    /// VPN session. Inspect persistent authority while retaining the mutex.
    pub fn admit_session()->Result<Self,String>{
        // The stable launcher retains this lease through CreateProcess. Allow
        // its short hand-off to finish, then inspect durable authority again.
        let lock=Self::acquire(5000)?;
        let executable=std::env::current_exe().map_err(|_|"Cannot identify session executable")?;
        let directory=executable.parent().ok_or("Missing session installation")?;
        let root=if directory.parent().and_then(|p|p.file_name()).is_some_and(|n|n=="versions") {
            directory.parent().and_then(|p|p.parent()).ok_or("Invalid version layout")?
        }else{directory};
        let path=root.join("current.json");
        match std::fs::File::open(&path){
            Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},
            Err(_)=>return Err("Не удалось проверить состояние обновления Atlas".into()),
            Ok(file)=>{
                use std::io::Read;let mut bytes=Vec::new();file.take(4*1024*1024+1).read_to_end(&mut bytes).map_err(|_|"Cannot read update authority")?;
                if bytes.len()>4*1024*1024{return Err("Update authority exceeds limit".into());}
                let journal:serde_json::Value=serde_json::from_slice(&bytes).map_err(|_|"Повреждён журнал обновления Atlas; требуется восстановление")?;
                session_stage(&journal,directory.file_name().and_then(|n|n.to_str()).unwrap_or(""))?;
            }
        }
        Ok(lock)
    }
    pub(crate) fn named(name:&str,timeout_ms:u32)->Result<Self,String> {
        let name:Vec<u16>=name.encode_utf16().chain(Some(0)).collect();
        // All authenticated sessions can synchronize; only administrators/SYSTEM
        // can alter the object ACL. Releasing another thread's mutex is impossible.
        let sddl:Vec<u16>="D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x00100001;;;AU)".encode_utf16().chain(Some(0)).collect();
        let mut descriptor=std::ptr::null_mut();
        unsafe {
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(),1,&mut descriptor,std::ptr::null_mut())==0 {return Err("Update lock security initialization failed".into());}
            let attributes=SECURITY_ATTRIBUTES{nLength:std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,lpSecurityDescriptor:descriptor,bInheritHandle:0};
            let handle=CreateMutexExW(&attributes,name.as_ptr(),0,0x00100001);LocalFree(descriptor);
            if handle.is_null(){return Err("Update lock unavailable".into());}
            let wait=WaitForSingleObject(handle,timeout_ms);
            if wait!=WAIT_OBJECT_0 && wait!=WAIT_ABANDONED {CloseHandle(handle);return Err("Обновление или восстановление Atlas уже выполняется".into());}
            Ok(Self(handle))
        }
    }
}
fn session_stage(journal:&serde_json::Value,directory:&str)->Result<(),String>{
    let stage=journal["stage"].as_str().ok_or("Missing update stage")?;
    if journal["schema"]!=1 || !matches!(stage,"Created"|"Downloading"|"Downloaded"|"Verified"|"Prepared"|"Failed"|"RolledBack"|"Committed") {
        return Err("Сначала завершите восстановление обновления Atlas".into());
    }
    let expected=if stage=="Committed"{&journal["candidate"]}else{&journal["previous"]};
    if journal["active"]!=*expected || expected["id"].as_str()!=Some(directory){
        return Err("Эта версия Atlas не является активной; запустите Atlas через launcher".into());
    }
    Ok(())
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn concurrent_thread_cannot_enter_and_drop_releases_lock() {
        let name=format!("Local\\Atlas.Update.Test.{}",uuid::Uuid::new_v4());
        let lock=UpdateLock::named(&name,0).unwrap();let other=name.clone();
        assert!(std::thread::spawn(move||UpdateLock::named(&other,0).is_err()).join().unwrap());
        drop(lock);assert!(std::thread::spawn(move||UpdateLock::named(&name,0).is_ok()).join().unwrap());
    }
    #[test] fn abandoned_owner_is_recoverable() {
        let name=format!("Local\\Atlas.Update.Test.{}",uuid::Uuid::new_v4());let other=name.clone();
        std::thread::spawn(move||{let lock=UpdateLock::named(&other,0).unwrap();std::mem::forget(lock);}).join().unwrap();
        assert!(UpdateLock::named(&name,0).is_ok());
    }
    #[test]fn durable_fence_survives_owner_exit_and_rejects_old_desktop_after_commit(){
        use serde_json::json;
        let mut j=json!({"schema":1,"stage":"Prepared","previous":{"id":"old"},"candidate":{"id":"new"},"active":{"id":"old"}});
        assert!(session_stage(&j,"old").is_ok());assert!(session_stage(&j,"new").is_err());
        for stage in ["Quiescing","Quiesced","SwitchIntent","Activated","HealthPending","RollbackPending","unknown"] {j["stage"]=json!(stage);assert!(session_stage(&j,"old").is_err());assert!(session_stage(&j,"new").is_err());}
        j["stage"]=json!("Committed");j["active"]=j["candidate"].clone();assert!(session_stage(&j,"new").is_ok());assert!(session_stage(&j,"old").is_err());
        j["stage"]=json!("RolledBack");assert!(session_stage(&j,"old").is_err());j["active"]=j["previous"].clone();assert!(session_stage(&j,"old").is_ok());
    }
}

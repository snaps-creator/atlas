//! Durable authority for side-by-side updates. No installed payload is overwritten.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fs::{self, File, OpenOptions}, io::{Read, Write}, path::{Component, Path, PathBuf}, time::{SystemTime, UNIX_EPOCH}};
type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Stage { Created, Downloading, Downloaded, Verified, Prepared, Quiescing, Quiesced, SwitchIntent, Activated, HealthPending, Committed, RollbackPending, RolledBack, Failed }
impl Stage {
    pub fn permits(self, to: Self) -> bool {
        use Stage::*;
        self == to || matches!((self,to),
            (Created,Downloading)|(Downloading,Downloaded)|(Downloaded,Verified)|(Verified,Prepared)|
            (Prepared,Quiescing)|(Quiescing,Quiesced)|(Quiesced,SwitchIntent)|(SwitchIntent,Activated)|
            (Activated,HealthPending)|(HealthPending,Committed)|(RollbackPending,RolledBack)) ||
            (to == RollbackPending && matches!(self,Quiescing|Quiesced|SwitchIntent|Activated|HealthPending)) ||
            (to == Failed && matches!(self,Created|Downloading|Downloaded|Verified|Prepared))
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Payload { pub path: String, pub size: u64, pub sha256: String }
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Version {
    pub id: String, pub version: String, pub build: String, pub files: Vec<Payload>,
    /// Explicit pre-install state, with no binaries and no service to restore.
    #[serde(default)] pub absent:bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Journal {
    pub schema: u32, pub transaction_id: String, pub sequence: u64, pub stage: Stage,
    pub started_at: u64, pub updated_at: u64,
    pub active: Version, pub previous: Version, pub candidate: Version,
    pub expected_artifact_hash: String, pub previous_service_path: String,
    pub migration_state: String, pub network_state: String, pub health_state: String,
}
fn now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() }
fn component(value: &str) -> Result<()> {
    if value.is_empty() || value.len()>100 || !value.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)) || value=="." || value==".." || value.ends_with('.') { return Err("Invalid update identifier".into()); }
    let head=value.split('.').next().unwrap().to_ascii_uppercase();
    if matches!(head.as_str(),"CON"|"PRN"|"AUX"|"NUL") || (head.len()==4 && (head.starts_with("COM")||head.starts_with("LPT")) && head.as_bytes()[3].is_ascii_digit()) { return Err("Reserved update identifier".into()); }
    Ok(())
}
fn relative(value: &str) -> Result<PathBuf> {
    if value.contains(['\\', ':']) {return Err("Invalid payload path".into());}
    let path=Path::new(value);
    if path.components().count()==0 || !path.components().all(|c|matches!(c,Component::Normal(_))) {return Err("Payload path escapes version".into());}
    for part in value.split('/') {component(part)?;}
    Ok(path.into())
}
fn plain(path: &Path) -> Result<()> {
    let meta=fs::symlink_metadata(path).map_err(|e|e.to_string())?;
    #[cfg(windows)] {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 { return Err("Reparse point in update path".into()); }
    }
    if meta.file_type().is_symlink() {return Err("Symlink in update path".into());}
    Ok(())
}
fn checked(root: &Path, rel: &str) -> Result<PathBuf> {
    let rel=relative(rel)?; let mut path=root.to_path_buf(); plain(&path)?;
    for part in rel.components() {path.push(part);plain(&path)?;}
    Ok(path)
}
pub fn digest(path: &Path) -> Result<(u64,String)> {
    let mut options=OpenOptions::new(); options.read(true);
    #[cfg(windows)] {use std::os::windows::fs::OpenOptionsExt; options.share_mode(1);}
    let mut file=options.open(path).map_err(|e|e.to_string())?;
    let mut hash=Sha256::new(); let mut size=0; let mut buffer=[0u8;65536];
    loop { let count=file.read(&mut buffer).map_err(|e|e.to_string())?; if count==0 {break;} hash.update(&buffer[..count]);size+=count as u64; }
    Ok((size,format!("{:x}",hash.finalize())))
}
fn inventory(root:&Path, current:&Path, output:&mut Vec<String>) -> Result<()> {
    plain(current)?;
    for entry in fs::read_dir(current).map_err(|e|e.to_string())? {
        let path=entry.map_err(|e|e.to_string())?.path();plain(&path)?;
        if path.is_dir() {inventory(root,&path,output)?;} else if path.is_file() {
            output.push(path.strip_prefix(root).map_err(|e|e.to_string())?.to_string_lossy().replace('\\',"/"));
        } else {return Err("Non-file in payload".into());}
    }
    Ok(())
}
pub fn validate_tree(root:&Path)->Result<()> {let mut names=Vec::new();inventory(root,root,&mut names)}
impl Version {
    pub fn absent()->Self {Self{id:"uninstalled".into(),version:"0.0.0".into(),build:"none".into(),files:Vec::new(),absent:true}}
    pub fn snapshot_legacy(root:&Path,version:String)->Result<Self> {
        plain(root)?;
        let id=format!("legacy-{}",uuid::Uuid::new_v4());
        let mut names=Vec::new();
        for entry in fs::read_dir(root).map_err(|e|e.to_string())? {
            let path=entry.map_err(|e|e.to_string())?.path();
            let name=path.file_name().and_then(|s|s.to_str()).ok_or("Invalid legacy installation filename")?;
            if name=="versions" || name=="current.json" || name.starts_with("state-") || name.starts_with("completed-") || name.starts_with("transaction-") {continue;}
            plain(&path)?;
            if path.is_dir(){inventory(root,&path,&mut names)?;}else if path.is_file(){names.push(name.to_owned());}
        }
        names.sort();
        let mut files=Vec::new();
        for path in names {let(size,sha256)=digest(&checked(root,&path)?)?;files.push(Payload{path,size,sha256});}
        let value=Self{id,version,build:"legacy".into(),files,absent:false};value.validate()?;
        if !value.files.iter().any(|f|f.path=="Atlas.exe") || !value.files.iter().any(|f|f.path=="Atlas.Service.exe") {
            return Err("Legacy installation is missing its desktop or service".into());
        }
        if free_space(root)?<space_required(0,value.size()?,0)?{return Err("Insufficient space for previous installation backup".into());}
        let versions=root.join("versions");fs::create_dir_all(&versions).map_err(|e|e.to_string())?;plain(&versions)?;
        let staging=versions.join(format!(".staging-{}",value.id));
        fs::create_dir(&staging).map_err(|e|e.to_string())?;
        for file in &value.files {
            let source=checked(root,&file.path)?;let target=staging.join(relative(&file.path)?);
            fs::create_dir_all(target.parent().unwrap()).map_err(|e|e.to_string())?;
            let mut input=File::open(source).map_err(|e|e.to_string())?;
            let mut output=OpenOptions::new().write(true).create_new(true).open(target).map_err(|e|e.to_string())?;
            std::io::copy(&mut input,&mut output).map_err(|e|e.to_string())?;output.sync_all().map_err(|e|e.to_string())?;
        }
        value.verify(&staging)?;
        let destination=versions.join(&value.id);
        use std::os::windows::ffi::OsStrExt;
        let from:Vec<u16>=staging.as_os_str().encode_wide().chain(Some(0)).collect();
        let to:Vec<u16>=destination.as_os_str().encode_wide().chain(Some(0)).collect();
        if unsafe{windows_sys::Win32::Storage::FileSystem::MoveFileExW(from.as_ptr(),to.as_ptr(),8)}==0 {return Err("Cannot publish previous installation backup".into());}
        value.verify(&destination)?;Ok(value)
    }
    pub fn size(&self)->Result<u64> {self.files.iter().try_fold(0u64,|n,f|n.checked_add(f.size).ok_or("Payload size overflow".into()))}
    pub fn validate(&self) -> Result<()> {
        if self.absent {
            return if *self==Self::absent(){Ok(())}else{Err("Invalid absent installation state".into())};
        }
        component(&self.id)?;component(&self.version)?;component(&self.build)?;
        if self.files.is_empty() {return Err("Empty version manifest".into());}
        let mut names=std::collections::HashSet::new();
        for file in &self.files {
            relative(&file.path)?;
            if !names.insert(file.path.to_ascii_lowercase()) || file.sha256.len()!=64 || !file.sha256.bytes().all(|b|b.is_ascii_hexdigit()) {return Err("Invalid payload manifest".into());}
        }
        Ok(())
    }
    pub fn verify(&self, root:&Path) -> Result<()> {
        self.validate()?;
        if self.absent{return if !root.exists(){Ok(())}else{Err("Absent installation unexpectedly contains files".into())};}
        let mut actual=Vec::new();inventory(root,root,&mut actual)?;actual.sort();
        let mut expected:Vec<_>=self.files.iter().map(|f|f.path.clone()).collect();expected.sort();
        if actual!=expected {return Err("Payload inventory mismatch".into());}
        self.verify_files(root)
    }
    pub fn verify_files(&self,root:&Path)->Result<()> {
        self.validate()?;
        for file in &self.files {
            let (size,hash)=digest(&checked(root,&file.path)?)?;
            if size!=file.size || !hash.eq_ignore_ascii_case(&file.sha256) {return Err("Payload hash mismatch".into());}
        }
        Ok(())
    }
}
/// All authority is in one atomic record: a crash cannot split stage and pointer.
pub fn durable_write(path:&Path, bytes:&[u8]) -> Result<()> {
    let parent=path.parent().ok_or("Missing journal directory")?;plain(parent)?;
    if path.exists() {plain(path)?;}
    let temp=parent.join(format!(".update-{}.tmp",uuid::Uuid::new_v4()));
    let result=(|| {
        let mut file=OpenOptions::new().write(true).create_new(true).open(&temp).map_err(|e|e.to_string())?;
        file.write_all(bytes).map_err(|e|e.to_string())?;file.sync_all().map_err(|e|e.to_string())?;drop(file);
        #[cfg(windows)] {
            use std::os::windows::ffi::OsStrExt;
            use windows_sys::Win32::Storage::FileSystem::{MoveFileExW,MOVEFILE_REPLACE_EXISTING,MOVEFILE_WRITE_THROUGH};
            let from:Vec<_>=temp.as_os_str().encode_wide().chain(Some(0)).collect();
            let to:Vec<_>=path.as_os_str().encode_wide().chain(Some(0)).collect();
            if unsafe{MoveFileExW(from.as_ptr(),to.as_ptr(),MOVEFILE_REPLACE_EXISTING|MOVEFILE_WRITE_THROUGH)}==0 {return Err(std::io::Error::last_os_error().to_string());}
        }
        #[cfg(not(windows))] {fs::rename(&temp,path).map_err(|e|e.to_string())?;File::open(parent).and_then(|f|f.sync_all()).map_err(|e|e.to_string())?;}
        Ok(())
    })();
    if result.is_err() {let _=fs::remove_file(&temp);} result
}
pub struct Transaction { root:PathBuf, pub journal:Journal, _lock:crate::update_lock::UpdateLock, log:crate::update_log::Log }
pub fn space_required(download:u64,staging:u64,backup:u64)->Result<u64> {
    download.checked_add(staging).and_then(|n|n.checked_add(backup)).and_then(|n|n.checked_add(16*1024*1024)).ok_or("Update space overflow".into())
}
fn free_space(path:&Path)->Result<u64> {
    #[cfg(windows)] {
        use std::os::windows::ffi::OsStrExt;
        let path:Vec<u16>=path.as_os_str().encode_wide().chain(Some(0)).collect();let mut free=0u64;
        if unsafe{windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(path.as_ptr(),&mut free,std::ptr::null_mut(),std::ptr::null_mut())}==0 {return Err("Cannot inspect update disk space".into());}Ok(free)
    }
    #[cfg(not(windows))] {let _=path;Err("Windows update storage required".into())}
}
impl Transaction {
    pub fn root(&self)->&Path{&self.root}
    fn lock(root:&Path)->Result<crate::update_lock::UpdateLock> {
        #[cfg(test)] {crate::update_lock::UpdateLock::named(&format!("Local\\Atlas.Tx.Test.{:x}",Sha256::digest(root.as_os_str().to_string_lossy().as_bytes())),0)}
        #[cfg(not(test))] {let _=root;crate::update_lock::UpdateLock::acquire(0)}
    }
    pub fn create(root:&Path, previous:Version, candidate:Version, artifact_hash:String, service_path:String) -> Result<Self> {
        let lock=Self::lock(root)?;
        previous.validate()?;candidate.validate()?;plain(root)?;
        if candidate.absent || previous.id==candidate.id || artifact_hash.len()!=64 || !artifact_hash.bytes().all(|b|b.is_ascii_hexdigit()) {return Err("Invalid transaction identity".into());}
        let path=root.join("current.json"); if path.exists() {return Err("Existing transaction requires recovery".into());}
        let stamp=now();
        let tx=Self{root:root.into(),_lock:lock,log:crate::update_log::Log::new(),journal:Journal{schema:1,transaction_id:uuid::Uuid::new_v4().to_string(),sequence:0,stage:Stage::Created,started_at:stamp,updated_at:stamp,
            active:previous.clone(),previous,candidate,expected_artifact_hash:artifact_hash,previous_service_path:service_path,
            migration_state:"unchanged".into(),network_state:"unchanged".into(),health_state:"unconfirmed".into()}};
        tx.persist()?;Ok(tx)
    }
    /// Start a subsequent update without removing current.json. A crash between
    /// history persistence and authority replacement still leaves the completed
    /// prior transaction authoritative.
    pub fn begin_next(mut self,candidate:Version,artifact_hash:String,service_path:String)->Result<Self>{
        if !matches!(self.journal.stage,Stage::Committed|Stage::RolledBack|Stage::Failed){return Err("Previous update requires recovery".into());}
        candidate.validate()?;
        if candidate.absent || candidate.id==self.journal.active.id || artifact_hash.len()!=64 || !artifact_hash.bytes().all(|b|b.is_ascii_hexdigit()){return Err("Invalid next update identity".into());}
        self.journal.active.verify(&self.version_path(&self.journal.active))?;
        let history=self.root.join(format!("completed-{}.json",self.journal.transaction_id));
        durable_write(&history,&serde_json::to_vec(&self.journal).map_err(|e|e.to_string())?)?;
        let previous=self.journal.active.clone();let stamp=now();let old=self.journal.clone();
        self.journal=Journal{schema:1,transaction_id:uuid::Uuid::new_v4().to_string(),sequence:0,stage:Stage::Created,started_at:stamp,updated_at:stamp,
            active:previous.clone(),previous,candidate,expected_artifact_hash:artifact_hash,previous_service_path:service_path,
            migration_state:"unchanged".into(),network_state:"unchanged".into(),health_state:"unconfirmed".into()};
        if let Err(e)=self.persist(){self.journal=old;return Err(e);}Ok(self)
    }
    pub fn open(root:&Path)->Result<Self> {
        let lock=Self::lock(root)?;
        plain(root)?;let path=root.join("current.json");plain(&path)?;
        let mut bytes=Vec::new();
        File::open(path).map_err(|e|e.to_string())?.take(4*1024*1024+1).read_to_end(&mut bytes).map_err(|e|e.to_string())?;
        if bytes.len()>4*1024*1024{return Err("Update authority exceeds limit".into());}
        let journal:Journal=serde_json::from_slice(&bytes).map_err(|e|e.to_string())?;
        if journal.schema!=1 || uuid::Uuid::parse_str(&journal.transaction_id).is_err() {return Err("Unsupported transaction journal".into());}
        journal.previous.validate()?;journal.candidate.validate()?;journal.active.validate()?;
        if journal.candidate.absent{return Err("Update candidate cannot be an absent installation".into());}
        if journal.active!=journal.previous && journal.active!=journal.candidate {return Err("Unknown active version".into());}
        let candidate_active=matches!(journal.stage,Stage::Activated|Stage::HealthPending|Stage::Committed);
        if candidate_active && journal.active!=journal.candidate || (!candidate_active && journal.stage!=Stage::RollbackPending && journal.active!=journal.previous) {return Err("Journal stage and active pointer disagree".into());}
        Ok(Self{root:root.into(),journal,_lock:lock,log:crate::update_log::Log::new()})
    }
    fn persist(&self)->Result<()> {durable_write(&self.root.join("current.json"),&serde_json::to_vec(&self.journal).map_err(|e|e.to_string())?)}
    pub fn record(&self,operation:crate::update_log::Operation,error:Option<&str>) {
        let path=self.root.join(format!("transaction-{}.ndjson",self.journal.transaction_id));
        if self.log.record(&path,&self.journal,operation,error).is_err(){eprintln!("Update audit log unavailable; durable transaction journal remains authoritative");}
    }
    pub fn advance(&mut self, stage:Stage)->Result<()> {
        if !self.journal.stage.permits(stage) {return Err("Forbidden update transition".into());}
        if self.journal.stage==stage {return Ok(());}
        let old=self.journal.clone();self.journal.stage=stage;self.journal.sequence+=1;self.journal.updated_at=now();
        if stage==Stage::Activated {self.journal.active=self.journal.candidate.clone();}
        if stage==Stage::RolledBack {self.journal.active=self.journal.previous.clone();}
        if let Err(error)=self.persist() {self.journal=old;return Err(error);}
        use crate::update_log::Operation;
        self.record(match stage {Stage::Created|Stage::Downloading|Stage::Downloaded=>Operation::Download,Stage::Verified=>Operation::Verify,Stage::Prepared=>Operation::Stage,Stage::Quiescing|Stage::Quiesced=>Operation::Quiesce,Stage::SwitchIntent|Stage::Activated=>Operation::Activate,Stage::HealthPending=>Operation::Health,Stage::Committed=>Operation::Commit,Stage::RollbackPending|Stage::RolledBack=>Operation::Rollback,Stage::Failed=>Operation::Recovery},None);
        Ok(())
    }
    pub fn version_path(&self, version:&Version)->PathBuf {self.root.join("versions").join(&version.id)}
    /// Conservative restart decision. External effects (SCM/network) must be
    /// reconciled before acknowledging RolledBack, never inferred from the pointer.
    pub fn recovery_version(&self)->&Version {
        if self.journal.stage==Stage::Committed {&self.journal.candidate} else {&self.journal.previous}
    }
    pub fn stage_payload(&mut self, source:&Path)->Result<()> {
        let free=free_space(&self.root)?;
        self.stage_with(source,free,|_|Ok(()))
    }
    fn stage_with(&mut self, source:&Path, available:u64, mut before_copy:impl FnMut(usize)->Result<()>)->Result<()> {
        if self.journal.stage!=Stage::Verified {return Err("Package must be verified before staging".into());}
        self.journal.candidate.verify(source)?;
        if available<space_required(0,self.journal.candidate.size()?,0)? {return Err("Insufficient update disk space".into());}
        let versions=self.root.join("versions");fs::create_dir_all(&versions).map_err(|e|e.to_string())?;plain(&versions)?;
        let dest=self.version_path(&self.journal.candidate);
        if dest.exists() {self.journal.candidate.verify(&dest)?;return self.advance(Stage::Prepared);}
        let staging=versions.join(format!(".staging-{}",self.journal.transaction_id));
        if staging.exists() {
            let mut names=Vec::new();inventory(&staging,&staging,&mut names)?;
            // A crashed unpublished stage is owned solely by this transaction.
            // Reject all reparses before recursive deletion; never touch a version.
            fs::remove_dir_all(&staging).map_err(|e|e.to_string())?;
        }
        fs::create_dir(&staging).map_err(|e|e.to_string())?;
        for (index,entry) in self.journal.candidate.files.iter().enumerate() {
            before_copy(index)?;
            let input=checked(source,&entry.path)?; let output=staging.join(relative(&entry.path)?);
            fs::create_dir_all(output.parent().unwrap()).map_err(|e|e.to_string())?;
            let mut reader=File::open(input).map_err(|e|e.to_string())?;
            let mut writer=OpenOptions::new().write(true).create_new(true).open(output).map_err(|e|e.to_string())?;
            std::io::copy(&mut reader,&mut writer).map_err(|e|e.to_string())?;writer.sync_all().map_err(|e|e.to_string())?;
        }
        self.journal.candidate.verify(&staging)?;
        #[cfg(windows)] {
            use std::os::windows::ffi::OsStrExt;
            let from:Vec<u16>=staging.as_os_str().encode_wide().chain(Some(0)).collect();let to:Vec<u16>=dest.as_os_str().encode_wide().chain(Some(0)).collect();
            if unsafe{windows_sys::Win32::Storage::FileSystem::MoveFileExW(from.as_ptr(),to.as_ptr(),windows_sys::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH)}==0 {return Err("Publish staged version failed".into());}
        }
        #[cfg(not(windows))] {fs::rename(&staging,&dest).map_err(|e|e.to_string())?;}
        self.journal.candidate.verify(&dest)?;self.advance(Stage::Prepared)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Temp(PathBuf);impl Temp {fn new()->Self {let p=std::env::temp_dir().join(format!("atlas-tx-test-{}",uuid::Uuid::new_v4()));fs::create_dir(&p).unwrap();Self(p)}}
    impl Drop for Temp {fn drop(&mut self){let _=fs::remove_dir_all(&self.0);}}
    fn version(id:&str,bytes:&[u8])->Version {Version{absent:false,id:id.into(),version:"2.4.1".into(),build:id.into(),files:vec![Payload{path:"Atlas.exe".into(),size:bytes.len() as u64,sha256:format!("{:x}",Sha256::digest(bytes))}]}}
    fn setup()->(Temp,Temp,Transaction) {let root=Temp::new();let source=Temp::new();fs::write(source.0.join("Atlas.exe"),b"new").unwrap();let old=version("old",b"old");let candidate=version("new",b"new");fs::create_dir_all(root.0.join("versions/old")).unwrap();fs::write(root.0.join("versions/old/Atlas.exe"),b"old").unwrap();let tx=Transaction::create(&root.0,old,candidate,"a".repeat(64),"old service".into()).unwrap();(root,source,tx)}
    #[test] fn forbidden_transitions_are_rejected() {let (_r,_s,mut tx)=setup();assert!(tx.advance(Stage::Committed).is_err());assert_eq!(tx.journal.sequence,0);assert!(!Stage::Verified.permits(Stage::Activated));assert!(Stage::HealthPending.permits(Stage::RollbackPending));}
    #[test]fn next_update_preserves_previous_authority_and_history(){
        let(root,source,mut tx)=setup();for s in [Stage::Downloading,Stage::Downloaded,Stage::Verified]{tx.advance(s).unwrap();}tx.stage_payload(&source.0).unwrap();
        for s in [Stage::Quiescing,Stage::Quiesced,Stage::SwitchIntent,Stage::Activated,Stage::HealthPending,Stage::Committed]{tx.advance(s).unwrap();}
        let id=tx.journal.transaction_id.clone();let tx=tx.begin_next(version("third",b"third"),"b".repeat(64),"new service".into()).unwrap();
        assert_eq!(tx.journal.previous.id,"new");assert_eq!(tx.recovery_version().id,"new");assert_ne!(tx.journal.transaction_id,id);
        let old:Journal=serde_json::from_slice(&fs::read(root.0.join(format!("completed-{id}.json"))).unwrap()).unwrap();assert_eq!(old.stage,Stage::Committed);
        let reopened=Transaction::open(&root.0).unwrap();assert_eq!(reopened.journal.stage,Stage::Created);reopened.recovery_version().verify(&reopened.version_path(reopened.recovery_version())).unwrap();
    }
    #[test] fn every_critical_restart_selects_complete_previous_until_commit() {let (root,source,mut tx)=setup();for stage in [Stage::Downloading,Stage::Downloaded,Stage::Verified]{tx.advance(stage).unwrap();}tx.stage_payload(&source.0).unwrap();for stage in [Stage::Prepared,Stage::Quiescing,Stage::Quiesced,Stage::SwitchIntent,Stage::Activated,Stage::HealthPending]{tx.advance(stage).unwrap();let reopened=Transaction::open(&root.0).unwrap();assert_eq!(reopened.recovery_version().id,"old");reopened.recovery_version().verify(&reopened.version_path(reopened.recovery_version())).unwrap();}tx.advance(Stage::Committed).unwrap();assert_eq!(Transaction::open(&root.0).unwrap().recovery_version().id,"new");assert_eq!(fs::read(root.0.join("versions/old/Atlas.exe")).unwrap(),b"old");}
    #[test] fn corrupt_staging_never_changes_previous() {let (root,source,mut tx)=setup();for s in [Stage::Downloading,Stage::Downloaded,Stage::Verified]{tx.advance(s).unwrap();}fs::write(source.0.join("Atlas.exe"),b"bad").unwrap();assert!(tx.stage_payload(&source.0).is_err());assert_eq!(tx.journal.stage,Stage::Verified);assert_eq!(fs::read(root.0.join("versions/old/Atlas.exe")).unwrap(),b"old");}
    #[test] fn interrupted_rollback_is_idempotent() {let(root,source,mut tx)=setup();for s in [Stage::Downloading,Stage::Downloaded,Stage::Verified]{tx.advance(s).unwrap();}tx.stage_payload(&source.0).unwrap();for s in [Stage::Quiescing,Stage::Quiesced,Stage::SwitchIntent,Stage::Activated,Stage::RollbackPending]{tx.advance(s).unwrap();}let mut tx=Transaction::open(&root.0).unwrap();assert_eq!(tx.recovery_version().id,"old");tx.advance(Stage::RolledBack).unwrap();let seq=tx.journal.sequence;tx.advance(Stage::RolledBack).unwrap();assert_eq!(tx.journal.sequence,seq);assert_eq!(Transaction::open(&root.0).unwrap().journal.active.id,"old");}
    #[test] fn path_traversal_case_duplicates_and_device_names_fail() {for value in ["../x","/x","C:/x","x:stream","x/../y","NUL","x/COM1.txt","x."]{assert!(relative(value).is_err(),"{value}");}let mut v=version("new",b"new");v.files.push(Payload{path:"atlas.exe".into(),..v.files[0].clone()});assert!(v.validate().is_err());}
    #[test] fn extra_payload_and_truncated_journal_fail_closed() {let(root,source,tx)=setup();fs::write(source.0.join("extra.dll"),b"x").unwrap();assert!(tx.journal.candidate.verify(&source.0).is_err());fs::write(root.0.join("current.json"),b"{").unwrap();assert!(Transaction::open(&root.0).is_err());}
    #[test] fn insufficient_disk_and_access_denied_never_publish_partial_version() {let(root,source,mut tx)=setup();for s in [Stage::Downloading,Stage::Downloaded,Stage::Verified]{tx.advance(s).unwrap();}assert!(tx.stage_with(&source.0,0,|_|Ok(())).is_err());assert!(!root.0.join("versions/new").exists());assert!(tx.stage_with(&source.0,u64::MAX,|_|Err("injected access denied".into())).is_err());assert!(!root.0.join("versions/new").exists());tx.stage_payload(&source.0).unwrap();assert_eq!(fs::read(root.0.join("versions/old/Atlas.exe")).unwrap(),b"old");assert!(space_required(u64::MAX,1,0).is_err());}
    #[test] fn real_locked_authority_file_preserves_last_durable_record() {use std::os::windows::fs::OpenOptionsExt;let(root,_source,mut tx)=setup();let _locked=OpenOptions::new().read(true).share_mode(1).open(root.0.join("current.json")).unwrap();assert!(tx.advance(Stage::Downloading).is_err());assert_eq!(tx.journal.stage,Stage::Created);assert_eq!(Transaction::open(&root.0).unwrap().journal.stage,Stage::Created);}
    #[test] fn crash_child_driver() {
        let Ok(root)=std::env::var("ATLAS_TEST_CRASH_ROOT") else {return;};
        let stage:Stage=serde_json::from_str(&std::env::var("ATLAS_TEST_CRASH_STAGE").unwrap()).unwrap();
        let source=std::env::var("ATLAS_TEST_CRASH_SOURCE").unwrap();let mut tx=Transaction::open(Path::new(&root)).unwrap();
        for s in [Stage::Downloading,Stage::Downloaded,Stage::Verified]{tx.advance(s).unwrap();}
        tx.stage_payload(Path::new(&source)).unwrap();
        for s in [Stage::Prepared,Stage::Quiescing,Stage::Quiesced,Stage::SwitchIntent,Stage::Activated,Stage::HealthPending] {tx.advance(s).unwrap();if stage==s {break;}}
        if stage==Stage::RollbackPending {tx.advance(Stage::RollbackPending).unwrap();}
        if stage==Stage::Committed {tx.advance(Stage::Committed).unwrap();}
        let event:Vec<u16>=std::env::var("ATLAS_TEST_CRASH_EVENT").unwrap().encode_utf16().chain(Some(0)).collect();
        unsafe {
            use windows_sys::Win32::System::Threading::*;
            let handle=OpenEventW(EVENT_MODIFY_STATE,0,event.as_ptr());assert!(!handle.is_null());assert_ne!(SetEvent(handle),0);windows_sys::Win32::Foundation::CloseHandle(handle);
        }
        std::thread::park();panic!("Crash test child was unexpectedly resumed");
    }
    #[test] fn real_process_termination_preserves_recoverable_complete_version() {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::{System::Threading::*,Foundation::{CloseHandle,WAIT_OBJECT_0}};
        for stage in [Stage::Prepared,Stage::Quiescing,Stage::Quiesced,Stage::SwitchIntent,Stage::Activated,Stage::HealthPending,Stage::RollbackPending,Stage::Committed] {
            let(root,source,tx)=setup();drop(tx);
            let name=format!("Local\\Atlas.Crash.Event.{}",uuid::Uuid::new_v4());let wide:Vec<u16>=name.encode_utf16().chain(Some(0)).collect();
            let event=unsafe{CreateEventW(std::ptr::null(),1,0,wide.as_ptr())};assert!(!event.is_null());
            let mut child=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","update_transaction::tests::crash_child_driver","--nocapture"])
                .env("ATLAS_TEST_CRASH_ROOT",&root.0).env("ATLAS_TEST_CRASH_SOURCE",&source.0).env("ATLAS_TEST_CRASH_STAGE",serde_json::to_string(&stage).unwrap()).env("ATLAS_TEST_CRASH_EVENT",name)
                .creation_flags(0x08000000).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
            let ready=unsafe{WaitForSingleObject(event,10000)};unsafe{CloseHandle(event);}
            child.kill().unwrap();child.wait().unwrap();assert_eq!(ready,WAIT_OBJECT_0,"child failed at {stage:?}");
            let tx=Transaction::open(&root.0).unwrap();assert_eq!(tx.journal.stage,stage);
            let selected=tx.recovery_version();selected.verify(&tx.version_path(selected)).unwrap();
            assert_eq!(selected.id,if stage==Stage::Committed {"new"}else{"old"});
            tx.journal.previous.verify(&tx.version_path(&tx.journal.previous)).unwrap();
        }
    }
}

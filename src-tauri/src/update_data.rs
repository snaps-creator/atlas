//! SQLite rollback preserves the encrypted database bytes, including committed
//! WAL data. Callers must hold the update lease and quiesce desktop processes.
use std::{fs,path::{Path,PathBuf}};
use rusqlite::{Connection,OpenFlags};
use serde::{Serialize,Deserialize};
type Result<T>=std::result::Result<T,String>;
#[derive(Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot{pub existed:bool,pub sha256:Option<String>,pub size:u64,pub schema:i64}
fn check(path:&Path)->Result<i64>{
    let db=Connection::open_with_flags(path,OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|_|"Cannot open settings snapshot")?;
    let integrity:String=db.query_row("PRAGMA integrity_check",[],|r|r.get(0)).map_err(|_|"Cannot inspect settings integrity")?;
    if integrity!="ok"{return Err("Settings database integrity failed".into());}
    db.query_row("PRAGMA user_version",[],|r|r.get(0)).map_err(|_|"Cannot inspect settings schema".into())
}
fn ordinary(path:&Path)->Result<()>{
    use std::os::windows::fs::MetadataExt;
    let metadata=fs::symlink_metadata(path).map_err(|_|"Cannot inspect settings path")?;
    if !metadata.is_file() || metadata.file_attributes()&0x400!=0{return Err("Settings path is not an ordinary file".into());}Ok(())
}
pub fn save(database:&Path,directory:&Path)->Result<Snapshot>{
    let authority=directory.join("settings-snapshot.json");
    if authority.exists(){let value:Snapshot=serde_json::from_slice(&crate::update_integrity::read_bounded(&authority,4096)?).map_err(|_|"Invalid settings snapshot record")?;value.verify(directory)?;return Ok(value);}
    let snapshot=directory.join("settings.sqlite");
    // No candidate is started until the snapshot authority is durable. A crashed
    // incomplete snapshot can be removed and recreated from the unchanged DB.
    if snapshot.exists(){ordinary(&snapshot)?;fs::remove_file(&snapshot).map_err(|_|"Cannot remove incomplete settings snapshot")?;}
    let value=if database.try_exists().map_err(|_|"Cannot inspect settings database")?{
        ordinary(database)?;
        let db=Connection::open_with_flags(database,OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|_|"Cannot snapshot settings database")?;
        db.execute("VACUUM INTO ?1",[snapshot.to_str().ok_or("Invalid snapshot path")?]).map_err(|_|"Settings snapshot failed (space, lock or access)")?;
        drop(db);fs::OpenOptions::new().read(true).write(true).open(&snapshot).and_then(|f|f.sync_all()).map_err(|_|"Cannot flush settings snapshot")?;
        let schema=check(&snapshot)?;let(size,sha256)=crate::update_transaction::digest(&snapshot)?;
        Snapshot{existed:true,sha256:Some(sha256),size,schema}
    }else{Snapshot{existed:false,sha256:None,size:0,schema:0}};
    crate::update_transaction::durable_write(&authority,&serde_json::to_vec(&value).map_err(|_|"Cannot encode settings snapshot")?)?;Ok(value)
}
impl Snapshot{
    pub fn verify(&self,directory:&Path)->Result<()>{
        if self.existed{let p=directory.join("settings.sqlite");ordinary(&p)?;let(size,hash)=crate::update_transaction::digest(&p)?;
            if size!=self.size||Some(&hash)!=self.sha256.as_ref()||check(&p)?!=self.schema{return Err("Settings snapshot changed".into());}
        }else if self.sha256.is_some()||self.size!=0{return Err("Invalid empty settings snapshot".into());}Ok(())
    }
}
pub fn restore(database:&Path,directory:&Path)->Result<()>{
    let snapshot:Snapshot=serde_json::from_slice(&crate::update_integrity::read_bounded(&directory.join("settings-snapshot.json"),4096)?).map_err(|_|"Invalid settings snapshot authority")?;
    snapshot.verify(directory)?;
    // RollbackPending remains durable throughout removal/replacement, so a
    // second recovery always repeats from the immutable SQLite snapshot.
    for suffix in ["-wal","-shm"]{
        let mut name=database.as_os_str().to_os_string();name.push(suffix);let path=PathBuf::from(name);
        if path.try_exists().map_err(|_|"Cannot inspect SQLite sidecar")?{ordinary(&path)?;fs::remove_file(path).map_err(|_|"Cannot remove candidate SQLite sidecar")?;}
    }
    if !snapshot.existed{if database.exists(){ordinary(database)?;fs::remove_file(database).map_err(|_|"Cannot restore absent settings database")?;}return Ok(());}
    let temp=database.with_file_name(format!(".atlas-restore-{}.sqlite",uuid::Uuid::new_v4()));
    let result=(||{
        let mut source=fs::File::open(directory.join("settings.sqlite")).map_err(|_|"Cannot open immutable settings snapshot")?;
        let mut out=fs::OpenOptions::new().create_new(true).write(true).open(&temp).map_err(|_|"Cannot create settings restore")?;
        std::io::copy(&mut source,&mut out).map_err(|_|"Cannot write settings restore")?;out.sync_all().map_err(|_|"Cannot flush settings restore")?;drop(out);
        if database.exists(){ordinary(database)?;}
        use std::os::windows::ffi::OsStrExt;
        let from:Vec<u16>=temp.as_os_str().encode_wide().chain(Some(0)).collect();let to:Vec<u16>=database.as_os_str().encode_wide().chain(Some(0)).collect();
        if unsafe{windows_sys::Win32::Storage::FileSystem::MoveFileExW(from.as_ptr(),to.as_ptr(),0x1|0x8)}==0{return Err("Cannot atomically restore settings database".into());}
        let(size,hash)=crate::update_transaction::digest(database)?;if size!=snapshot.size||Some(hash)!=snapshot.sha256{return Err("Restored settings bytes differ".into());}check(database)?;Ok(())
    })();if result.is_err(){let _=fs::remove_file(temp);}result
}
#[cfg(test)]mod tests{
    use super::*;
    #[test]fn wal_snapshot_and_repeated_rollback_restore_exact_schema_and_settings(){
        let root=std::env::temp_dir().join(format!("atlas-data-test-{}",uuid::Uuid::new_v4()));fs::create_dir(&root).unwrap();let path=root.join("atlas.db");
        let db=Connection::open(&path).unwrap();db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA user_version=2; CREATE TABLE settings(value TEXT); INSERT INTO settings VALUES('old selected node');").unwrap();
        let snapshot=save(&path,&root).unwrap();assert_eq!(snapshot.schema,2);drop(db);
        let db=Connection::open(&path).unwrap();db.execute_batch("UPDATE settings SET value='candidate'; PRAGMA user_version=3;").unwrap();drop(db);
        restore(&path,&root).unwrap();restore(&path,&root).unwrap();let db=Connection::open(&path).unwrap();let value:String=db.query_row("SELECT value FROM settings",[],|r|r.get(0)).unwrap();assert_eq!(value,"old selected node");assert_eq!(check(&path).unwrap(),2);drop(db);fs::remove_dir_all(root).unwrap();
    }
    #[test]fn corrupted_snapshot_cannot_replace_candidate_data(){
        let root=std::env::temp_dir().join(format!("atlas-data-corrupt-{}",uuid::Uuid::new_v4()));fs::create_dir(&root).unwrap();let path=root.join("atlas.db");let db=Connection::open(&path).unwrap();db.execute_batch("CREATE TABLE settings(value TEXT)").unwrap();drop(db);save(&path,&root).unwrap();let before=fs::read(&path).unwrap();fs::write(root.join("settings.sqlite"),b"corrupt").unwrap();assert!(restore(&path,&root).is_err());assert_eq!(fs::read(path).unwrap(),before);fs::remove_dir_all(root).unwrap();
    }
}

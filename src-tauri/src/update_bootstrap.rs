//! Migration of an existing flat installation into the same versioned update
//! transaction used by later updates. No old uninstaller is executed.
use std::{path::Path,os::windows::ffi::OsStrExt};
use crate::update_transaction::{Version,Transaction,Stage};

pub fn file_version(path:&Path)->Result<String,String>{unsafe{
    use windows_sys::Win32::Storage::FileSystem::*;
    let name:Vec<u16>=path.as_os_str().encode_wide().chain(Some(0)).collect();
    let size=GetFileVersionInfoSizeW(name.as_ptr(),std::ptr::null_mut());
    if size==0||size>2*1024*1024{return Err("Executable has no bounded Windows version resource".into());}
    let mut bytes=vec![0usize;(size as usize).div_ceil(std::mem::size_of::<usize>())];
    if GetFileVersionInfoW(name.as_ptr(),0,size,bytes.as_mut_ptr().cast())==0{return Err("Cannot read executable version resource".into());}
    let query=|key:&str|->Result<(*mut core::ffi::c_void,u32),String>{
        let key:Vec<u16>=key.encode_utf16().chain(Some(0)).collect();let mut value=std::ptr::null_mut();let mut count=0;
        if VerQueryValueW(bytes.as_ptr().cast(),key.as_ptr(),&mut value,&mut count)==0||value.is_null(){return Err("Executable version field is missing".into());}
        Ok((value,count))
    };
    let translation=query(r"\VarFileInfo\Translation")?;
    if translation.1<4{return Err("Executable version translation is missing".into());}
    let words=std::slice::from_raw_parts(translation.0.cast::<u16>(),2);
    let (value,count)=query(&format!(r"\StringFileInfo\{:04x}{:04x}\ProductVersion",words[0],words[1]))?;
    if count==0||count>256{return Err("Invalid executable product version length".into());}
    let text=String::from_utf16(std::slice::from_raw_parts(value.cast::<u16>(),count as usize)).map_err(|_|"Invalid executable version Unicode")?;
    Ok(text.trim_end_matches('\0').to_owned())
}}

pub fn upgrade(root:&Path,source:&Path,installer:&Path)->Result<(),String>{
    // The embedded receipt authenticates every candidate byte before touching
    // the installation. A user-writable external JSON is not authority here.
    let candidate=crate::update_bundle::payload(source)?;
    let (_,artifact_hash)=crate::update_transaction::digest(installer)?;
    let _lease=crate::update_lock::UpdateLock::acquire(0)?;
    let expected=crate::update_service::command(root)?;
    let service=if crate::update_service::Service::exists()?{Some(crate::update_service::Service::open(false)?.configuration()?)}else{None};
    if !root.join("current.json").exists() && service.as_ref().is_some_and(|s|!s.binary.eq_ignore_ascii_case(&expected)){return Err("Existing Atlas service belongs to another installation".into());}
    std::fs::create_dir_all(root).map_err(|e|e.to_string())?;
    crate::update_windows::protect_installation(root)?;
    let previous_service=service.as_ref().map(|s|s.binary.clone()).unwrap_or_default();
    let mut transaction=if root.join("current.json").exists() {
        let mut transaction=Transaction::open(root)?;
        if matches!(transaction.journal.stage,Stage::Created|Stage::Downloading|Stage::Downloaded|Stage::Verified|Stage::Prepared) {
            transaction.advance(Stage::Failed)?;
        }else if !matches!(transaction.journal.stage,Stage::Committed|Stage::RolledBack|Stage::Failed) {
            let mut host=crate::update_windows::Windows::prepare(&transaction,crate::update_user::database()?)?;
            crate::update_supervisor::recover(&mut transaction,&mut host)?;
        }
        if transaction.journal.stage==Stage::Committed && transaction.journal.active==candidate {
            candidate.verify(&transaction.version_path(&candidate))?;
            drop(transaction);
            return crate::update_launcher::launch(root,false);
        }
        let current_service=if crate::update_service::Service::exists()?{crate::update_service::Service::open(false)?.configuration()?.binary}else{String::new()};
        transaction.begin_next(candidate,artifact_hash,current_service)?
    }else{
        let previous=if service.is_some(){Version::snapshot_legacy(root,file_version(&root.join("Atlas.exe"))?)?}else{
            if root.join("Atlas.exe").exists() || root.join("Atlas.Service.exe").exists(){return Err("Unregistered Atlas files require recovery before a clean installation".into());}
            Version::absent()
        };
        Transaction::create(root,previous,candidate,artifact_hash,previous_service)?
    };
    // Manual installation has already obtained and verified the whole payload.
    // These durable acknowledgements precede any old-process shutdown.
    for stage in [Stage::Downloading,Stage::Downloaded,Stage::Verified]{transaction.advance(stage)?;}
    transaction.stage_payload(source)?;
    // NSIS intentionally denies execution to limited users in $PLUGINSDIR.
    // Stage and verify first, then probe the identical helper from the protected
    // installation tree, whose users have read/execute but no write access.
    // Failure here still precedes all desktop, service and network shutdown.
    crate::update_user::check_executable(&transaction.version_path(&transaction.journal.candidate).join("AtlasUpdater.exe"))?;
    let mut host=crate::update_windows::Windows::prepare(&transaction,crate::update_user::database()?)?;
    crate::update_supervisor::activate(&mut transaction,&mut host)
}

pub fn prepare_uninstall(root:&Path)->Result<(),String>{
    let _lease=crate::update_lock::UpdateLock::acquire(0)?;
    let mut roots=vec![root.to_path_buf()];
    if root.join("current.json").exists(){
        let transaction=Transaction::open(root)?;
        roots.push(transaction.version_path(&transaction.journal.previous));
        roots.push(transaction.version_path(&transaction.journal.candidate));
    }
    crate::maintenance::prepare_transaction(&roots)?;
    crate::update_service::Service::remove()?;
    crate::network_guard::wait_for_clean_baseline(std::time::Duration::from_secs(5))
}

pub fn uninstall(root:&Path)->Result<(),String>{
    let root=root.canonicalize().map_err(|e|e.to_string())?;
    let executable=std::env::current_exe().map_err(|e|e.to_string())?.canonicalize().map_err(|e|e.to_string())?;
    if executable.starts_with(&root){return Err("Uninstaller helper must run from its private temporary directory".into());}
    let _lease=crate::update_lock::UpdateLock::acquire(0)?;
    let transaction=Transaction::open(&root)?;
    crate::update_transaction::validate_tree(&root)?;
    let mut files=std::collections::BTreeSet::new();
    for version in [&transaction.journal.previous,&transaction.journal.candidate] {
        version.validate()?;for file in &version.files{files.insert(file.path.clone());}
    }
    let mut records=Vec::new();let mut states=Vec::new();
    for entry in std::fs::read_dir(&root).map_err(|e|e.to_string())? {
        let path=entry.map_err(|e|e.to_string())?.path();let name=path.file_name().and_then(|n|n.to_str()).ok_or("Invalid installation record name")?;
        if name.starts_with("completed-") && name.ends_with(".json") {
            let journal:crate::update_transaction::Journal=serde_json::from_slice(&crate::update_integrity::read_bounded(&path,4*1024*1024)?).map_err(|_|"Invalid update history during uninstall")?;
            for version in [&journal.previous,&journal.candidate]{version.validate()?;for file in &version.files{files.insert(file.path.clone());}}
            records.push(path);
        }else if name.starts_with("transaction-") && name.ends_with(".ndjson") {records.push(path);}
        else if name.strip_prefix("state-").is_some_and(|id|uuid::Uuid::parse_str(id).is_ok()) {states.push(path);}
    }
    let roots=vec![root.clone(),transaction.version_path(&transaction.journal.previous),transaction.version_path(&transaction.journal.candidate)];
    crate::maintenance::prepare_transaction(&roots)?;
    crate::update_service::Service::remove()?;
    crate::network_guard::wait_for_clean_baseline(std::time::Duration::from_secs(5))?;
    for name in ["Atlas.exe","Atlas.Service.exe","AtlasUpdater.exe","AtlasMaintenance.exe"]{files.insert(name.into());}
    for relative in files {
        let path=root.join(relative);
        if path.try_exists().map_err(|e|e.to_string())? {
            std::fs::remove_file(&path).map_err(|e|format!("Cannot remove Atlas component {}: {e}",path.display()))?;
            let mut parent=path.parent();
            while let Some(directory)=parent {
                if directory==root{break;}
                if std::fs::remove_dir(directory).is_err(){break;}
                parent=directory.parent();
            }
        }
    }
    for directory in states.into_iter().chain([root.join("versions")]) {
        if directory.exists(){std::fs::remove_dir_all(&directory).map_err(|e|format!("Cannot remove Atlas installation state: {e}"))?;}
    }
    for path in records {std::fs::remove_file(path).map_err(|e|e.to_string())?;}
    std::fs::remove_file(root.join("current.json")).map_err(|e|e.to_string())?;
    Ok(())
}

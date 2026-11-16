//! Download and authenticate a complete transactional installer before UAC.
//! Runs in the independent native helper so the installer can stop the desktop.
use std::{path::Path,io::Write,sync::{Arc,atomic::{AtomicBool,AtomicU64,Ordering}},time::Duration};

fn stable(value:&str)->Result<[u32;3],String>{
    let parts=value.split('.').map(str::parse::<u32>).collect::<Result<Vec<_>,_>>().map_err(|_|"Invalid stable update version")?;
    let parts:[u32;3]=parts.try_into().map_err(|_|"Invalid stable update version")?;
    if format!("{}.{}.{}",parts[0],parts[1],parts[2])!=value{return Err("Noncanonical update version".into());}
    Ok(parts)
}
fn newer(version:&str,current:&str)->Result<(),String>{
    if stable(version)?<=stable(current.split('-').next().unwrap_or(current))? {return Err("Update must be newer than the installed version".into());}
    Ok(())
}
fn event(stage:&str,message:Option<&str>){
    let _=writeln!(std::io::stdout(),"{}",serde_json::json!({"stage":stage,"message":message}));
    let _=std::io::stdout().flush();
}
async fn metadata(client:&reqwest::Client,url:&str,limit:usize)->Result<Vec<u8>,String>{
    let mut response=client.get(url).timeout(Duration::from_secs(30)).send().await.map_err(|_|"Update metadata is unavailable")?;
    if !response.status().is_success(){return Err(format!("Update metadata HTTP {}",response.status().as_u16()));}
    let mut bytes=Vec::new();
    while let Some(chunk)=response.chunk().await.map_err(|_|"Update metadata download failed")? {
        if bytes.len()+chunk.len()>limit{return Err("Update metadata exceeds size limit".into());}
        bytes.extend(chunk);
    }
    Ok(bytes)
}
async fn cancellable_metadata(client:&reqwest::Client,url:&str,limit:usize,cancel:&AtomicBool)->Result<Vec<u8>,String>{
    if cancel.load(Ordering::SeqCst){return Err("Download cancelled".into());}
    tokio::select! {
        result=metadata(client,url,limit)=>result,
        _=async {while !cancel.load(Ordering::SeqCst){tokio::time::sleep(Duration::from_millis(50)).await;}}=>Err("Download cancelled".into()),
    }
}
pub fn elevated(executable:&Path,arguments:&str)->Result<(),String>{
    use std::os::windows::{ffi::OsStrExt,io::{FromRawHandle,OwnedHandle}};
    use windows_sys::Win32::{UI::Shell::*,System::Threading::*,Foundation::*};
    let wide=|s:&std::ffi::OsStr|s.encode_wide().chain(Some(0)).collect::<Vec<_>>();
    let file=wide(executable.as_os_str());let args=wide(std::ffi::OsStr::new(arguments));let verb=wide(std::ffi::OsStr::new("runas"));
    let directory=wide(executable.parent().ok_or("Installer directory missing")?.as_os_str());
    unsafe {
        let mut info:SHELLEXECUTEINFOW=std::mem::zeroed();info.cbSize=std::mem::size_of_val(&info) as u32;
        info.fMask=SEE_MASK_NOCLOSEPROCESS|SEE_MASK_NOASYNC;info.lpVerb=verb.as_ptr();info.lpFile=file.as_ptr();info.lpParameters=args.as_ptr();info.lpDirectory=directory.as_ptr();info.nShow=1;
        if ShellExecuteExW(&mut info)==0 {return Err(format!("Windows did not start the installer (UAC may have been cancelled): {}",std::io::Error::last_os_error()));}
        if info.hProcess.is_null(){return Err("Installer process handle missing".into());}
        let _handle=OwnedHandle::from_raw_handle(info.hProcess);
        // Never terminate an installer mid-transaction. The package stays pinned
        // until the installer finishes, even after its old desktop has exited.
        if WaitForSingleObject(info.hProcess,INFINITE)!=WAIT_OBJECT_0{return Err("Installer wait failed".into());}
        let mut code=0;if GetExitCodeProcess(info.hProcess,&mut code)==0 || code!=0{return Err(format!("Installer returned failure ({code}); inspect its recovery log"));}
    }
    Ok(())
}
pub fn run(version:&str)->Result<(),String>{
    newer(version,env!("CARGO_PKG_VERSION"))?;
    let root=std::env::temp_dir().join(format!("atlas-verified-update-{}",uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).map_err(|_|"Cannot create private update directory")?;
    let result:Result<(),String>=(||{
        event("downloading",None);
        let config:serde_json::Value=serde_json::from_str(include_str!("../tauri.conf.json")).map_err(|_|"Invalid updater trust")?;
        let key=config["plugins"]["updater"]["pubkey"].as_str().ok_or("Missing updater trust")?;
        let rt=tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|_|"Update runtime failed")?;
        let client=crate::update_download::client()?;
        let base=format!("https://github.com/snaps-creator/atlas/releases/download/v{version}");
        let bytes=rt.block_on(metadata(&client,&format!("{base}/update-manifest.json"),1024*1024))?;
        let signature=String::from_utf8(rt.block_on(metadata(&client,&format!("{base}/update-manifest.json.sig"),4096))?).map_err(|_|"Invalid manifest signature encoding")?;
        let verified=crate::update_integrity::manifest(&bytes,&signature,key)?;
        let manifest=verified.data();
        if manifest.version!=version || manifest.schema!=2 || manifest.installer_kind.as_deref()!=Some("transactional-v1") {
            return Err("The signed release does not contain a compatible transactional installer".into());
        }
        let installer=root.join("Atlas-Setup.exe");
        let asset=crate::update_download::Asset{url:manifest.url.clone(),size:manifest.size,sha256:manifest.sha256.clone()};
        rt.block_on(crate::update_download::download(&asset,&installer,&AtomicBool::new(false),&AtomicU64::new(0)))?;
        let _pinned=crate::update_integrity::VerifiedPackage::open(&installer,verified,key)?;
        event("installing",None);
        elevated(&installer,"")?;
        event("complete",None);Ok(())
    })();
    if let Err(error)=&result {event("error",Some(error));}
    // Only this invocation's random private download directory is removed.
    let _=std::fs::remove_dir_all(&root);
    result
}
// The repository-root installer uses the same public key, signed manifest,
// streaming verifier and transactional installer as an in-app update.
pub fn setup(cancel_file:&Path)->Result<(),String>{
    let cancel=Arc::new(AtomicBool::new(false));let done=Arc::new(AtomicBool::new(false));
    let (watch_cancel,watch_done,path)=(cancel.clone(),done.clone(),cancel_file.to_path_buf());
    let watcher=std::thread::spawn(move||{
        while !watch_done.load(Ordering::SeqCst) {
            if path.exists(){watch_cancel.store(true,Ordering::SeqCst);break;}
            std::thread::sleep(Duration::from_millis(100));
        }
    });
    let result=setup_inner(&cancel);
    done.store(true,Ordering::SeqCst);let _=watcher.join();
    if let Err(error)=&result {event("error",Some(error));}
    result
}
fn setup_inner(cancel:&AtomicBool)->Result<(),String>{
    let root=std::env::temp_dir().join(format!("atlas-verified-setup-{}",uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).map_err(|_|"Cannot create private setup directory")?;
    let result=(||{
        event("checking",None);
        let config:serde_json::Value=serde_json::from_str(include_str!("../tauri.conf.json")).map_err(|_|"Invalid updater trust")?;
        let key=config["plugins"]["updater"]["pubkey"].as_str().ok_or("Missing updater trust")?;
        let rt=tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|_|"Update runtime failed")?;
        let client=crate::update_download::client()?;
        let base="https://github.com/snaps-creator/atlas/releases/latest/download";
        let bytes=rt.block_on(cancellable_metadata(&client,&format!("{base}/update-manifest.json"),1024*1024,cancel))?;
        let signature=String::from_utf8(rt.block_on(cancellable_metadata(&client,&format!("{base}/update-manifest.json.sig"),4096,cancel))?).map_err(|_|"Invalid manifest signature encoding")?;
        let verified=crate::update_integrity::manifest(&bytes,&signature,key)?;
        let manifest=verified.data();
        if stable(&manifest.version)?<stable(env!("CARGO_PKG_VERSION"))? || manifest.schema!=2 || manifest.installer_kind.as_deref()!=Some("transactional-v1") {
            return Err("The fixed Atlas release is not published yet. Download the current full installer from GitHub Releases after publication.".into());
        }
        if cancel.load(Ordering::SeqCst){return Err("Download cancelled".into());}
        event("downloading",Some(&manifest.version));
        let installer=root.join("Atlas-Setup.exe");
        let asset=crate::update_download::Asset{url:manifest.url.clone(),size:manifest.size,sha256:manifest.sha256.clone()};
        rt.block_on(crate::update_download::download(&asset,&installer,cancel,&AtomicU64::new(0)))?;
        let _pinned=crate::update_integrity::VerifiedPackage::open(&installer,verified,key)?;
        if cancel.load(Ordering::SeqCst){return Err("Download cancelled".into());}
        event("installing",None);
        if cancel.load(Ordering::SeqCst){return Err("Download cancelled".into());}
        elevated(&installer,"")?;
        event("complete",None);Ok(())
    })();
    let _=std::fs::remove_dir_all(&root);
    result
}
#[cfg(test)]mod tests{
    use super::*;
    #[test]fn cancelling_metadata_does_not_wait_for_a_stalled_server(){
        use std::{io::Read,net::TcpListener};
        let listener=TcpListener::bind("127.0.0.1:0").unwrap();let address=listener.local_addr().unwrap();
        let server=std::thread::spawn(move||{
            let (mut stream,_)=listener.accept().unwrap();stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut buffer=[0;2048];let _=stream.read(&mut buffer);
            // Never send HTTP headers; the downloader must cancel its request.
            while stream.read(&mut buffer).is_ok_and(|n|n>0){}
        });
        let cancel=AtomicBool::new(false);
        let rt=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let start=std::time::Instant::now();
        let result=rt.block_on(async {
            let client=reqwest::Client::builder().no_proxy().build().unwrap();let url=format!("http://{address}/manifest");
            let request=cancellable_metadata(&client,&url,1024,&cancel);
            let trigger=async {tokio::time::sleep(Duration::from_millis(150)).await;cancel.store(true,Ordering::SeqCst);};
            tokio::join!(request,trigger).0
        });
        assert_eq!(result.unwrap_err(),"Download cancelled");assert!(start.elapsed()<Duration::from_secs(2));
        server.join().unwrap();
    }
    #[test]fn only_canonical_newer_stable_releases_are_downloaded(){
        assert!(newer("2.4.3","2.4.2").is_ok());
        for value in ["2.4.2","2.4.1","2.04.3","2.4.3/evil","2.4.3-alpha.1","https://other","2.4","2.4.3.4"]{assert!(newer(value,"2.4.2").is_err());}
    }
}

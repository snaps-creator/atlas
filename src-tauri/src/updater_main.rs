//! Native updater entrypoint; no CEF, UI runtime or VPN backend initialization.
#![allow(dead_code)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod update_lock;
mod update_transaction;
mod update_integrity;
mod update_download;
mod update_supervisor;
mod update_health;
mod update_process;
mod update_log;
mod update_candidate;
mod update_service;
mod update_data;
mod update_launcher;
mod maintenance;
mod network_guard;
mod windows;
mod lan_policy;
mod update_windows;
mod update_user;
mod update_bundle;
mod update_bootstrap;
mod update_install;
fn trust_key()->Result<String,String>{
    let config:serde_json::Value=serde_json::from_str(include_str!("../tauri.conf.json")).map_err(|_|"Embedded updater trust is invalid")?;
    config["plugins"]["updater"]["pubkey"].as_str().map(str::to_owned).ok_or("Embedded updater trust missing".into())
}
fn signed_manifest(manifest:&std::path::Path,signature:&std::path::Path)->Result<update_integrity::VerifiedManifest,String>{
    let bytes=update_integrity::read_bounded(manifest,1024*1024)?;
    let signature=String::from_utf8(update_integrity::read_bounded(signature,4096)?).map_err(|_|"Invalid manifest signature text")?;
    update_integrity::manifest(&bytes,&signature,&trust_key()?)
}
fn run()->Result<(),String> {
    let args:Vec<String>=std::env::args().collect();
    if args.len()==3 && args[1]=="--download-install" {return update_install::run(&args[2]);}
    if args.len()==1 || args.len()==2 && args[1]=="--autostart" {
        let executable=std::env::current_exe().map_err(|e|e.to_string())?;
        if executable.file_name().is_some_and(|name|name.to_string_lossy().eq_ignore_ascii_case("Atlas.exe")) {
            return update_launcher::launch(executable.parent().ok_or("Missing Atlas installation")?,args.len()==2);
        }
    }
    if args.len()==2 && args[1]=="--user-context-report" {println!("{}",update_user::report()?);return Ok(());}
    if args.len()==2 && args[1]=="--check-user-context" {println!("{}",update_user::check()?);return Ok(());}
    if args.len()==3 && args[1]=="--check-user-context" {println!("{}",update_user::check_executable(std::path::Path::new(&args[2]))?);return Ok(());}
    if args.len()==3 && args[1]=="--verify-local-payload" {
        let payload=update_bundle::payload(std::path::Path::new(&args[2]))?;
        println!("{}",serde_json::json!({"verified":true,"version":payload.version,"build":payload.build,"files":payload.files.len()}));
        return Ok(());
    }
    if args.len()==5 && args[1]=="--install-local" {
        return update_bootstrap::upgrade(std::path::Path::new(&args[2]),std::path::Path::new(&args[3]),std::path::Path::new(&args[4]));
    }
    if args.len()==3 && args[1]=="--prepare-uninstall" {return update_bootstrap::prepare_uninstall(std::path::Path::new(&args[2]));}
    if args.len()==3 && args[1]=="--uninstall-local" {return update_bootstrap::uninstall(std::path::Path::new(&args[2]));}
    if args.len()==4 && args[1]=="--activate" {
        let executable=std::env::current_exe().map_err(|e|e.to_string())?;
        let root=executable.parent().ok_or("Missing updater installation")?;
        let manifest=signed_manifest(std::path::Path::new(&args[2]),std::path::Path::new(&args[3]))?;
        let mut transaction=update_transaction::Transaction::open(root)?;
        if manifest.data().payload()?!=transaction.journal.candidate || manifest.data().sha256!=transaction.journal.expected_artifact_hash {
            return Err("Prepared transaction differs from signed update metadata".into());
        }
        // Fail before quiescence if the desktop cannot run in its original
        // unelevated account or the installer was elevated as another user.
        update_user::check()?;
        let mut host=update_windows::Windows::prepare(&transaction,update_user::database()?)?;
        update_supervisor::activate(&mut transaction,&mut host)?;
        println!("{}",serde_json::json!({"committed":true,"version":transaction.journal.active.version,"transaction":transaction.journal.transaction_id}));
        return Ok(());
    }
    if args.len()==2 && args[1]=="--recover" {
        let executable=std::env::current_exe().map_err(|e|e.to_string())?;
        let root=executable.parent().ok_or("Missing updater installation")?;
        let mut transaction=update_transaction::Transaction::open(root)?;
        let mut host=update_windows::Windows::prepare(&transaction,update_user::database()?)?;
        update_supervisor::recover(&mut transaction,&mut host)?;
        println!("{}",serde_json::json!({"recovered":true,"version":transaction.journal.active.version,"stage":transaction.journal.stage}));
        return Ok(());
    }
    if args.len() == 2 && args[1] == "--launch" || args.len() == 3 && args[1] == "--launch" && args[2] == "--autostart" {
        let executable = std::env::current_exe().map_err(|e| format!("Cannot locate Atlas launcher: {e}"))?;
        let root = executable.parent().ok_or("Missing Atlas installation directory")?;
        return update_launcher::launch(root, args.len() == 3);
    }
    if args.len()==2 && args[1]=="--protocol" {
        println!("{}",serde_json::json!({"protocol":1,"version":env!("CARGO_PKG_VERSION"),"build":env!("ATLAS_BUILD_ID")}));return Ok(());
    }
    if args.len()==5 && args[1]=="--verify-package" {
        let manifest=signed_manifest(std::path::Path::new(&args[2]),std::path::Path::new(&args[3]))?;
        let package=update_integrity::VerifiedPackage::open(std::path::Path::new(&args[4]),manifest,&trust_key()?)?;
        let value=package.manifest.data();
        println!("{}",serde_json::json!({"verified":true,"version":value.version,"build":value.build,"sha256":value.sha256,"size":value.size}));return Ok(());
    }
    Err("Expected --protocol, --launch [--autostart], --activate <manifest> <manifest.sig>, --recover or --verify-package <manifest> <manifest.sig> <package>".into())
}
fn main(){if let Err(error)=run(){
    if std::env::args().any(|a|a=="--launch") || std::env::current_exe().ok().and_then(|p|p.file_name().map(|n|n.to_string_lossy().eq_ignore_ascii_case("Atlas.exe"))).unwrap_or(false) {update_launcher::record_failure(&error);}
    eprintln!("{error}");std::process::exit(1);
}}

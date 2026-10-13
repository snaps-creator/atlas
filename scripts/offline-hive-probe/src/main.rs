#[path="../../../src-tauri/src/uninstall_hive.rs"] mod uninstall_hive;
#[path="../../../src-tauri/src/uninstall_startup.rs"] mod uninstall_startup;
fn main() {
    assert_eq!(std::env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
    assert_eq!(std::env::var("RUNNER_OS").as_deref(), Ok("Windows"));
    use winreg::{RegKey,enums::*};
    let profiles=RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList").unwrap();
    for sid in profiles.enum_keys().map(Result::unwrap) {
        if !sid.starts_with("S-1-5-21-") {continue;}
        if RegKey::predef(HKEY_USERS).open_subkey(&sid).is_ok() {println!("PROFILE {sid} loaded; no offline access");continue;}
        let raw:String=profiles.open_subkey(&sid).unwrap().get_value("ProfileImagePath").unwrap();
        let path=std::path::PathBuf::from(raw.replace("%SystemDrive%", &std::env::var("SystemDrive").unwrap())).join("NTUSER.DAT");
        if !path.exists() {println!("PROFILE {sid} absent hive");continue;}
        println!("PROFILE {sid} hive={path:?} length={}", std::fs::metadata(&path).unwrap().len());
        match RegKey::load_app_key_with_flags(&path, KEY_READ|KEY_WRITE, REG_PROCESS_APPKEY) {
            Ok(key)=>{println!("APPKEY OK");drop(key);}
            Err(error)=>println!("APPKEY ERROR {error}"),
        }
        println!("STANDARD {:?}",uninstall_hive::with_hive(&path, |_| Ok(())));
    }
    if let Some(root)=std::env::args().nth(1) {
        println!("PRODUCTION CLEANUP {:?}",uninstall_startup::cleanup(std::path::Path::new(&root)));
    }
}

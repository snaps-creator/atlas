//! Documented Shell properties; never reads or rewrites the user's taskbar pins.
use std::path::{Path, PathBuf};
use windows_api::{core::{Interface, PCWSTR}, Win32::{
    Foundation::HWND,
    Storage::EnhancedStorage::{PKEY_AppUserModel_ID, PKEY_AppUserModel_RelaunchCommand,
        PKEY_AppUserModel_RelaunchDisplayNameResource, PKEY_AppUserModel_RelaunchIconResource},
    System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED, STGM_READWRITE, StructuredStorage::PROPVARIANT},
    UI::Shell::{PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow},
        IShellLinkW, ShellLink, SetCurrentProcessExplicitAppUserModelID},
}};

pub const APP_ID: &str = "net.atlasvpn.desktop";
fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    value.encode_wide().chain(Some(0)).collect()
}
pub fn launcher(executable: &Path) -> Result<PathBuf, String> {
    let directory = executable.parent().ok_or("Missing executable directory")?;
    let root = if directory.parent().and_then(Path::file_name).is_some_and(|n| n == "versions") {
        directory.parent().and_then(Path::parent).ok_or("Invalid version layout")?
    } else { directory };
    let target = root.join("Atlas.exe");
    if !target.is_absolute() || target.to_string_lossy().contains(['"','\r','\n']) {
        return Err("Invalid canonical Atlas launcher".into());
    }
    Ok(target)
}
pub fn process() -> Result<(), String> {
    unsafe { SetCurrentProcessExplicitAppUserModelID(PCWSTR(wide(APP_ID.as_ref()).as_ptr())) }.map_err(|e|e.to_string())
}
pub fn window(hwnd: isize, executable: &Path) -> Result<(), String> {
    window_with_id(hwnd, executable, APP_ID)
}
pub(crate) fn window_with_id(hwnd: isize, executable: &Path, id: &str) -> Result<(), String> {
    let target = launcher(executable)?;
    let command = format!("\"{}\"", target.display());
    let icon = format!("{},0", target.display());
    unsafe {
        let store: IPropertyStore = SHGetPropertyStoreForWindow(HWND(hwnd as _)).map_err(|e|e.to_string())?;
        // Set relaunch properties before the ID, as required by the Shell.
        for (key, value) in [(PKEY_AppUserModel_RelaunchCommand, command.as_str()),
            (PKEY_AppUserModel_RelaunchDisplayNameResource, "Atlas"),
            (PKEY_AppUserModel_RelaunchIconResource, icon.as_str()), (PKEY_AppUserModel_ID, id)] {
            store.SetValue(&key, &PROPVARIANT::from(value)).map_err(|e|e.to_string())?;
        }
        // Window property stores apply immediately (Commit is not required).
    }
    Ok(())
}
pub fn stamp_shortcut(path: &Path, executable: &Path) -> Result<(), String> {
    if path.extension().is_none_or(|v|!v.eq_ignore_ascii_case("lnk")) { return Err("Expected Atlas shortcut".into()); }
    let target = launcher(executable)?;
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().map_err(|e|e.to_string())?;
        let result = (|| -> windows_api::core::Result<()> {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            let file: IPersistFile = link.cast()?;
            file.Load(PCWSTR(wide(path.as_os_str()).as_ptr()), STGM_READWRITE)?;
            let mut current = [0u16;32768];
            link.GetPath(&mut current, std::ptr::null_mut(), 0)?;
            let end = current.iter().position(|v|*v==0).unwrap_or(current.len());
            if !String::from_utf16_lossy(&current[..end]).eq_ignore_ascii_case(&target.to_string_lossy()) {
                return Err(windows_api::core::Error::from_hresult(windows_api::core::HRESULT(0x80070057u32 as i32)));
            }
            let store: IPropertyStore = link.cast()?;
            store.SetValue(&PKEY_AppUserModel_ID, &PROPVARIANT::from(APP_ID))?;
            store.Commit()?;
            file.Save(PCWSTR(wide(path.as_os_str()).as_ptr()), true)?;
            Ok(())
        })();
        CoUninitialize();
        result.map_err(|e|e.to_string())
    }
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn versions_and_flat_layout_share_existing_identity_and_launcher() {
        let target=Path::new(r"C:\Program Files\Atlas\Atlas.exe");
        for version in ["3.0.1-old", "3.0.2-next", "3.0.3-current"] {
            assert_eq!(launcher(&PathBuf::from(format!(r"C:\Program Files\Atlas\versions\{version}\Atlas.exe"))).unwrap(),target);
        }
        assert_eq!(launcher(target).unwrap(),target);
        assert!(launcher(Path::new("relative/Atlas.exe")).is_err());
        let config:serde_json::Value=serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(config["identifier"],APP_ID);
    }
    #[test] fn real_shortcut_stamp_is_idempotent_and_rejects_foreign_targets() {
        let root=std::env::temp_dir().join(format!("atlas-shell-{}",uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let outcome=std::panic::catch_unwind(||unsafe{
            CoInitializeEx(None,COINIT_APARTMENTTHREADED).ok().unwrap();
            let path=root.join("Atlas.lnk");
            let create=|target:&Path| {
                let link:IShellLinkW=CoCreateInstance(&ShellLink,None,CLSCTX_INPROC_SERVER).unwrap();
                link.SetPath(PCWSTR(wide(target.as_os_str()).as_ptr())).unwrap();
                let file:IPersistFile=link.cast().unwrap();
                file.Save(PCWSTR(wide(path.as_os_str()).as_ptr()),true).unwrap();
            };
            create(&root.join("Atlas.exe"));
            for _ in 0..2 {stamp_shortcut(&path,&root.join("AtlasUpdater.exe")).unwrap();}
            let link:IShellLinkW=CoCreateInstance(&ShellLink,None,CLSCTX_INPROC_SERVER).unwrap();
            let file:IPersistFile=link.cast().unwrap();
            file.Load(PCWSTR(wide(path.as_os_str()).as_ptr()),STGM_READWRITE).unwrap();
            let store:IPropertyStore=link.cast().unwrap();
            assert_eq!(store.GetValue(&PKEY_AppUserModel_ID).unwrap().to_string(),APP_ID);
            drop(store);drop(file);drop(link);
            create(&root.join("Other.exe"));
            let before=std::fs::read(&path).unwrap();
            assert!(stamp_shortcut(&path,&root.join("AtlasUpdater.exe")).is_err());
            assert_eq!(std::fs::read(&path).unwrap(),before);
            CoUninitialize();
        });
        std::fs::remove_dir_all(&root).unwrap();
        if let Err(error)=outcome {std::panic::resume_unwind(error);}
    }
}

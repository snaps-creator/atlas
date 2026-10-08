//! Keep the Windows startup entry on the stable launcher across version swaps.
use std::path::Path;
use winreg::{RegKey,RegValue,enums::*};
const RUN:&str=r"Software\Microsoft\Windows\CurrentVersion\Run";
const APPROVED:&str=r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
fn command(executable:&Path)->Result<String,String>{
    if !executable.is_absolute()||executable.to_string_lossy().contains(['"','\r','\n','\0']) {
        return Err("Invalid startup executable".into());
    }
    let directory=executable.parent().ok_or("Missing application directory")?;
    let launcher=if directory.parent().and_then(|p|p.file_name()).is_some_and(|n|n.to_string_lossy().eq_ignore_ascii_case("versions")) {
        let root=directory.parent().and_then(|p|p.parent()).ok_or("Invalid version installation")?;
        Some(root.join("AtlasUpdater.exe"))
    }else{None};
    let target=launcher.as_deref().unwrap_or(executable);
    let needs_launch=launcher.is_some()||target.file_name().is_some_and(|n|n.to_string_lossy().eq_ignore_ascii_case("AtlasUpdater.exe"));
    Ok(format!("\"{}\" {}--autostart",target.display(),if needs_launch{"--launch "}else{""}))
}
pub fn configure(enabled:bool,explicit:bool)->Result<(),String>{
    let user=RegKey::predef(HKEY_CURRENT_USER);
    let (run,_)=user.create_subkey(RUN).map_err(|e|e.to_string())?;
    if enabled {
        let executable=std::env::current_exe().map_err(|e|e.to_string())?;
        let value=command(&executable)?;
        if run.get_value::<String,_>("Atlas").ok().as_deref()!=Some(&value) {
            run.set_value("Atlas",&value).map_err(|e|e.to_string())?;
        }
        // A user action enables startup; automatic path migration preserves a
        // disable action made in Windows Task Manager.
        if explicit {
            if let Ok(approved)=user.open_subkey_with_flags(APPROVED,KEY_SET_VALUE) {
                let mut bytes=vec![0;12];bytes[0]=2;
                approved.set_raw_value("Atlas",&RegValue{vtype:REG_BINARY,bytes}).map_err(|e|e.to_string())?;
            }
        }
    } else if let Err(error)=run.delete_value("Atlas") {
        if error.kind()!=std::io::ErrorKind::NotFound{return Err(error.to_string());}
    }
    Ok(())
}
#[cfg(test)]mod tests{
    use super::*;
    #[test]fn startup_uses_quoted_stable_launcher_for_every_version(){
        for id in ["2.4.1-first","2.4.2-next"] {
            let path=format!(r"C:\Program Files\Atlas\versions\{id}\Atlas.exe");
            assert_eq!(command(Path::new(&path)).unwrap(),r#""C:\Program Files\Atlas\AtlasUpdater.exe" --launch --autostart"#);
        }
        assert_eq!(command(Path::new(r"C:\Program Files\Atlas\Atlas.exe")).unwrap(),r#""C:\Program Files\Atlas\Atlas.exe" --autostart"#);
        assert!(command(Path::new("relative/Atlas.exe")).is_err());
    }
    #[test]fn startup_recognizes_windows_case_and_direct_updater(){
        assert_eq!(command(Path::new(r"C:\Atlas\Versions\2.4.2\Atlas.exe")).unwrap(),r#""C:\Atlas\AtlasUpdater.exe" --launch --autostart"#);
        assert_eq!(command(Path::new(r"C:\Atlas\ATLASUPDATER.EXE")).unwrap(),r#""C:\Atlas\ATLASUPDATER.EXE" --launch --autostart"#);
    }
    #[test]fn startup_preserves_unc_unicode_and_non_versioned_paths(){
        assert_eq!(command(Path::new(r"\\server\Apps\Atlas\versions\2.4.2\Atlas.exe")).unwrap(),r#""\\server\Apps\Atlas\AtlasUpdater.exe" --launch --autostart"#);
        assert_eq!(command(Path::new(r"C:\Приложения\Atlas\versions\2.4.2\Atlas.exe")).unwrap(),r#""C:\Приложения\Atlas\AtlasUpdater.exe" --launch --autostart"#);
        assert_eq!(command(Path::new(r"C:\Atlas\notversions\2.4.2\Atlas.exe")).unwrap(),r#""C:\Atlas\notversions\2.4.2\Atlas.exe" --autostart"#);
    }
    #[test]fn startup_rejects_unrooted_paths_and_command_injection(){
        for path in [r"C:Atlas.exe",r"\Atlas.exe",r"Atlas.exe",r#"C:\Atlas\bad"name\Atlas.exe"#,"C:\\Atlas\\bad\nname\\Atlas.exe","C:\\Atlas\\bad\rname\\Atlas.exe","C:\\Atlas\\bad\0name\\Atlas.exe",r#"C:\Atlas\versions\bad"version\Atlas.exe"#] {
            assert!(command(Path::new(path)).is_err(),"{path:?}");
        }
    }
}

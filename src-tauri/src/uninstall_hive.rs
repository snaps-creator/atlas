//! Load offline user profiles through the normal registry hive API. Valid
//! NTUSER.DAT files can be rejected by the application-hive loader (ERROR_BADDB).
use std::{
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::Path,
    ptr,
};
use windows_sys::Win32::{
    Foundation::{GetLastError, SetLastError},
    Security::*,
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};
use winreg::{enums::HKEY_USERS, RegKey};

#[link(name = "advapi32")]
extern "system" {
    fn RegLoadKeyW(key: winreg::HKEY, name: *const u16, file: *const u16) -> i32;
    fn RegUnLoadKeyW(key: winreg::HKEY, name: *const u16) -> i32;
}

fn error(action: &str, code: i32) -> String {
    format!("{action}: {}", std::io::Error::from_raw_os_error(code))
}

struct Privilege {
    token: OwnedHandle,
    previous: TOKEN_PRIVILEGES,
}
impl Privilege {
    fn enable(name: &str) -> Result<Self, String> {
        unsafe {
            let mut raw = ptr::null_mut();
            if OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_QUERY | TOKEN_ADJUST_PRIVILEGES,
                &mut raw,
            ) == 0
            {
                return Err(error(
                    "Cannot prepare offline registry privileges",
                    GetLastError() as i32,
                ));
            }
            let token = OwnedHandle::from_raw_handle(raw);
            let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
            let mut luid = std::mem::zeroed();
            if LookupPrivilegeValueW(ptr::null(), name.as_ptr(), &mut luid) == 0 {
                return Err(error(
                    "Cannot resolve offline registry privilege",
                    GetLastError() as i32,
                ));
            }
            let requested = TOKEN_PRIVILEGES {
                PrivilegeCount: 1,
                Privileges: [LUID_AND_ATTRIBUTES {
                    Luid: luid,
                    Attributes: SE_PRIVILEGE_ENABLED,
                }],
            };
            let mut lease = Self {
                token,
                previous: std::mem::zeroed(),
            };
            let mut size = 0;
            SetLastError(0);
            let success = AdjustTokenPrivileges(
                lease.token.as_raw_handle(),
                0,
                &requested,
                std::mem::size_of::<TOKEN_PRIVILEGES>() as u32,
                &mut lease.previous,
                &mut size,
            );
            let code = GetLastError();
            if success == 0 || code != 0 {
                return Err(error(
                    "Cannot enable offline registry privilege",
                    code as i32,
                ));
            }
            Ok(lease)
        }
    }
}
impl Drop for Privilege {
    fn drop(&mut self) {
        unsafe {
            AdjustTokenPrivileges(
                self.token.as_raw_handle(),
                0,
                &self.previous,
                0,
                ptr::null_mut(),
                ptr::null_mut(),
            );
        }
    }
}

struct MountedHive {
    name: Vec<u16>,
    key: Option<RegKey>,
    mounted: bool,
}
impl MountedHive {
    fn open(path: &Path) -> Result<Self, String> {
        let name = format!("AtlasUninstall-{}", uuid::Uuid::new_v4());
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let file: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let code = unsafe { RegLoadKeyW(HKEY_USERS, wide.as_ptr(), file.as_ptr()) };
        if code != 0 {
            return Err(error("Cannot load offline user registry hive", code));
        }
        let mut mounted = Self {
            name: wide,
            key: None,
            mounted: true,
        };
        mounted.key = Some(
            RegKey::predef(HKEY_USERS)
                .open_subkey(&name)
                .map_err(|e| format!("Cannot open mounted offline user hive: {e}"))?,
        );
        Ok(mounted)
    }
    fn close(&mut self) -> Result<(), String> {
        self.key.take(); // Every application handle must close before unload.
        if self.mounted {
            let code = unsafe { RegUnLoadKeyW(HKEY_USERS, self.name.as_ptr()) };
            if code != 0 {
                return Err(error("Cannot unload offline user registry hive", code));
            }
            self.mounted = false;
        }
        Ok(())
    }
}
impl Drop for MountedHive {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

pub fn with_hive<T>(
    path: &Path,
    action: impl FnOnce(&RegKey) -> Result<T, String>,
) -> Result<T, String> {
    // These already-held administrator privileges are enabled only while the
    // unique mount exists. Restore their previous state after closing it.
    let _backup = Privilege::enable("SeBackupPrivilege")?;
    let _restore = Privilege::enable("SeRestorePrivilege")?;
    let mut mounted = MountedHive::open(path)?;
    let result = action(mounted.key.as_ref().ok_or("Offline hive handle missing")?);
    let closed = mounted.close();
    match (result, closed) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(primary), Ok(())) => Err(primary),
        (Ok(_), Err(cleanup)) => Err(cleanup),
        (Err(primary), Err(cleanup)) => Err(format!("{primary}; {cleanup}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::{Foundation::LocalFree, Security::Authorization::*};
    use winreg::enums::*;

    #[link(name = "advapi32")]
    extern "system" {
        fn RegSaveKeyW(
            key: winreg::HKEY,
            file: *const u16,
            attributes: *const SECURITY_ATTRIBUTES,
        ) -> i32;
    }

    fn permissions(key: &RegKey, sddl: &str) {
        unsafe {
            let text: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
            let mut descriptor = ptr::null_mut();
            assert_ne!(
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    text.as_ptr(),
                    1,
                    &mut descriptor,
                    ptr::null_mut()
                ),
                0
            );
            let mut acl = ptr::null_mut();
            let mut present = 0;
            let mut defaulted = 0;
            assert_ne!(
                GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted),
                0
            );
            let result = SetSecurityInfo(
                key.raw_handle(),
                SE_REGISTRY_KEY,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                acl,
                ptr::null_mut(),
            );
            LocalFree(descriptor);
            assert_eq!(result, 0);
        }
    }

    fn mounts() -> std::collections::BTreeSet<String> {
        RegKey::predef(HKEY_USERS)
            .enum_keys()
            .map(Result::unwrap)
            .filter(|name| name.starts_with("AtlasUninstall-"))
            .collect()
    }

    #[test]
    #[ignore = "Requires an elevated disposable Windows runner; creates only its own temporary hive"]
    fn ordinary_hive_with_distinct_acls_roundtrips_and_unloads_after_errors() {
        assert_eq!(std::env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
        assert_eq!(std::env::var("RUNNER_OS").as_deref(), Ok("Windows"));
        let nonce = uuid::Uuid::new_v4();
        let folder = std::env::temp_dir().join(format!("atlas-native-hive-{nonce}"));
        std::fs::create_dir(&folder).unwrap();
        let source = format!(r"Software\AtlasTests\offline-hive-{nonce}");
        struct Fixture {
            source: String,
            folder: std::path::PathBuf,
        }
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(&self.source);
                let _ = std::fs::remove_dir_all(&self.folder);
            }
        }
        let _fixture = Fixture {
            source: source.clone(),
            folder: folder.clone(),
        };
        let hive = folder.join("NTUSER.DAT");
        {
            let (root, _) = RegKey::predef(HKEY_CURRENT_USER)
                .create_subkey(&source)
                .unwrap();
            permissions(&root, "D:P(A;;KA;;;SY)(A;;KA;;;BA)(A;;KR;;;WD)");
            let (child, _) = root.create_subkey("Child").unwrap();
            permissions(&child, "D:P(A;;KA;;;SY)(A;;KA;;;BA)");
            child.set_value("Atlas", &"owned-fixture").unwrap();
            child.set_value("Unrelated", &"preserved").unwrap();
            let file: Vec<u16> = hive.as_os_str().encode_wide().chain(Some(0)).collect();
            let _backup = Privilege::enable("SeBackupPrivilege").unwrap();
            assert_eq!(
                unsafe { RegSaveKeyW(root.raw_handle(), file.as_ptr(), ptr::null()) },
                0
            );
        }
        let before = mounts();
        with_hive(&hive, |root| {
            let child = root
                .open_subkey_with_flags("Child", KEY_READ | KEY_SET_VALUE)
                .map_err(|e| e.to_string())?;
            assert_eq!(
                child.get_value::<String, _>("Atlas").unwrap(),
                "owned-fixture"
            );
            child.delete_value("Atlas").map_err(|e| e.to_string())?;
            Ok(())
        })
        .unwrap();
        with_hive(&hive, |root| {
            let child = root.open_subkey("Child").map_err(|e| e.to_string())?;
            assert!(child.get_raw_value("Atlas").is_err());
            assert_eq!(
                child.get_value::<String, _>("Unrelated").unwrap(),
                "preserved"
            );
            Ok(())
        })
        .unwrap();
        let failure =
            with_hive::<()>(&hive, |_| Err("intentional action failure".into())).unwrap_err();
        assert_eq!(failure, "intentional action failure");
        assert_eq!(
            mounts(),
            before,
            "No temporary user-hive mount may survive either result"
        );
        let _exclusive = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .open(&hive)
            .expect("Hive remains locked after cleanup");
    }
}

use serde::Serialize;
use std::{collections::BTreeMap, path::Path};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::{Diagnostics::ToolHelp::*, Threading::*},
    UI::{Shell::*, WindowsAndMessaging::*},
};
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Application {
    pub name: String,
    pub process: String,
    pub path: String,
    pub running: bool,
    pub icon: Option<String>,
}
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
fn add(map: &mut BTreeMap<String, Application>, path: String, name: Option<String>, running: bool) {
    let path = path.trim_matches('"').to_owned();
    let p = Path::new(&path);
    if !p.is_file() || !p.extension().is_some_and(|s| s.eq_ignore_ascii_case("exe")) {
        return;
    }
    let process = p.file_name().unwrap().to_string_lossy().to_string();
    let key = path.to_lowercase();
    if let Some(old) = map.get_mut(&key) {
        old.running |= running;
        return;
    }
    let name = name.unwrap_or_else(|| p.file_stem().unwrap().to_string_lossy().to_string());
    map.insert(
        key,
        Application {
            name,
            process,
            path,
            running,
            icon: None,
        },
    );
}
pub fn list() -> Vec<Application> {
    let mut map = BTreeMap::new();
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot != INVALID_HANDLE_VALUE {
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of_val(&entry) as u32;
            let mut ok = Process32FirstW(snapshot, &mut entry);
            while ok != 0 {
                let process =
                    OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, entry.th32ProcessID);
                if !process.is_null() {
                    let mut buf = vec![0u16; 32768];
                    let mut len = buf.len() as u32;
                    if QueryFullProcessImageNameW(process, 0, buf.as_mut_ptr(), &mut len) != 0 {
                        add(
                            &mut map,
                            String::from_utf16_lossy(&buf[..len as usize]),
                            None,
                            true,
                        );
                    }
                    CloseHandle(process);
                }
                ok = Process32NextW(snapshot, &mut entry);
            }
            CloseHandle(snapshot);
        }
    }
    for root in [
        winreg::enums::HKEY_CURRENT_USER,
        winreg::enums::HKEY_LOCAL_MACHINE,
    ] {
        for view in [
            winreg::enums::KEY_WOW64_64KEY,
            winreg::enums::KEY_WOW64_32KEY,
        ] {
            let root = winreg::RegKey::predef(root);
            if let Ok(paths) = root.open_subkey_with_flags(
                "Software\\Microsoft\\Windows\\CurrentVersion\\App Paths",
                winreg::enums::KEY_READ | view,
            ) {
                for key in paths.enum_keys().flatten() {
                    if let Ok(app) = paths.open_subkey(key) {
                        if let Ok(path) = app.get_value::<String, _>("") {
                            add(&mut map, path, None, false);
                        }
                    }
                }
            }
            if let Ok(paths) = root.open_subkey_with_flags(
                "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
                winreg::enums::KEY_READ | view,
            ) {
                for key in paths.enum_keys().flatten() {
                    if let Ok(app) = paths.open_subkey(key) {
                        if let Ok(icon) = app.get_value::<String, _>("DisplayIcon") {
                            let path = icon
                                .rsplit_once(',')
                                .map(|(p, _)| p)
                                .unwrap_or(&icon)
                                .trim_matches('"')
                                .to_owned();
                            add(
                                &mut map,
                                path,
                                app.get_value::<String, _>("DisplayName").ok(),
                                false,
                            );
                        }
                    }
                }
            }
        }
    }
    let mut apps: Vec<_> = map.into_values().collect();
    apps.sort_by_key(|a| (!a.running, a.name.to_lowercase()));
    for app in &mut apps {
        app.icon = icon(&app.path);
    }
    apps
}
fn icon(path: &str) -> Option<String> {
    unsafe {
        let mut info: SHFILEINFOW = std::mem::zeroed();
        if SHGetFileInfoW(
            wide(path).as_ptr(),
            0,
            &mut info,
            std::mem::size_of_val(&info) as u32,
            SHGFI_ICON | SHGFI_SMALLICON,
        ) == 0
        {
            return None;
        }
        let screen = GetDC(std::ptr::null_mut());
        let dc = CreateCompatibleDC(screen);
        let mut bitmap: BITMAPINFO = std::mem::zeroed();
        bitmap.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bitmap.bmiHeader.biWidth = 32;
        bitmap.bmiHeader.biHeight = -32;
        bitmap.bmiHeader.biPlanes = 1;
        bitmap.bmiHeader.biBitCount = 32;
        bitmap.bmiHeader.biCompression = BI_RGB;
        let mut bits = std::ptr::null_mut();
        let dib = CreateDIBSection(
            screen,
            &bitmap,
            DIB_RGB_COLORS,
            &mut bits,
            std::ptr::null_mut(),
            0,
        );
        let mut png_data = None;
        if !dib.is_null() && !dc.is_null() {
            let old = SelectObject(dc, dib);
            std::ptr::write_bytes(bits as *mut u8, 0, 4096);
            if DrawIconEx(
                dc,
                0,
                0,
                info.hIcon,
                32,
                32,
                0,
                std::ptr::null_mut(),
                DI_NORMAL,
            ) != 0
            {
                let mut pixels = std::slice::from_raw_parts(bits as *const u8, 4096).to_vec();
                for p in pixels.chunks_exact_mut(4) {
                    p.swap(0, 2);
                }
                let mut bytes = vec![];
                {
                    let mut encoder = png::Encoder::new(&mut bytes, 32, 32);
                    encoder.set_color(png::ColorType::Rgba);
                    encoder.set_depth(png::BitDepth::Eight);
                    if let Ok(mut writer) = encoder.write_header() {
                        let _ = writer.write_image_data(&pixels);
                    }
                }
                if !bytes.is_empty() {
                    use base64::Engine;
                    png_data = Some(format!(
                        "data:image/png;base64,{}",
                        base64::engine::general_purpose::STANDARD.encode(bytes)
                    ));
                }
            }
            SelectObject(dc, old);
        }
        if !dib.is_null() {
            DeleteObject(dib);
        }
        if !dc.is_null() {
            DeleteDC(dc);
        }
        ReleaseDC(std::ptr::null_mut(), screen);
        DestroyIcon(info.hIcon);
        png_data
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn finds_current_process_with_icon() {
        let current = std::env::current_exe().unwrap();
        let apps = super::list();
        let app = apps
            .iter()
            .find(|a| std::path::Path::new(&a.path) == current)
            .expect("current executable must be listed");
        assert!(app.running);
        assert!(app
            .icon
            .as_ref()
            .is_some_and(|i| i.starts_with("data:image/png;base64,")));
    }
}

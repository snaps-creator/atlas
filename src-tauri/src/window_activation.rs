//! Forward to Tauri's existing IPC BEFORE CEF claims the cache singleton.
//! The startup barrier covers the gap before the plugin creates its HWND.
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::Manager;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

#[derive(Default)]
pub struct Activation { ready: AtomicBool, pending: AtomicBool }

pub fn request(app: &tauri::AppHandle) {
    let state = app.state::<Activation>();
    state.pending.store(true, Ordering::SeqCst);
    if state.ready.load(Ordering::SeqCst) {
        let handle = app.clone();
        if let Err(error) = app.run_on_main_thread(move || {
            if !handle.state::<Activation>().pending.swap(false, Ordering::SeqCst) { return; }
            if handle.try_state::<crate::ShuttingDown>().is_some_and(|s| s.load(Ordering::SeqCst)) { return; }
            if let Some(window) = handle.get_webview_window("main") {
                let result = window.show().and_then(|_| window.unminimize()).and_then(|_| window.set_focus());
                if result.is_ok() && handle.try_state::<crate::Shared>().is_some() { crate::persist_window_hidden(&handle,false); }
                if let Err(error) = result { eprintln!("Atlas window activation failed: {error}"); }
            }
        }) { eprintln!("Atlas activation dispatch failed: {error}"); }
    }
}
pub fn ready(app: &tauri::AppHandle) {
    app.state::<Activation>().ready.store(true, Ordering::SeqCst);
    if app.state::<Activation>().pending.load(Ordering::SeqCst) { request(app); }
}
fn wide(text: &str) -> Vec<u16> { text.encode_utf16().chain(Some(0)).collect() }
#[repr(C)]
struct CopyData { tag: usize, size: u32, data: *const u8 }

fn forward(id: &str) -> Result<bool, String> {
    unsafe {
        let hwnd = FindWindowW(wide(&format!("{id}-sic")).as_ptr(), wide(&format!("{id}-siw")).as_ptr());
        if hwnd.is_null() { return Ok(false); }
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 { return Err("Atlas activation endpoint has no owner".into()); }
        // Same documented foreground transfer used by the pinned Tauri plugin.
        AllowSetForegroundWindow(pid);
        // No user-controlled commands or network actions cross this endpoint.
        let args: Vec<String> = std::env::args().collect();
        let flag = match crate::startup_coordinator::LaunchReason::from_args(&args) {
            crate::startup_coordinator::LaunchReason::ManualLaunch => "",
            _ => "|--autostart",
        };
        let bytes = format!("|Atlas{flag}\0").into_bytes();
        let message = CopyData { tag:1542, size:bytes.len() as u32, data:bytes.as_ptr() };
        let mut result = 0;
        if SendMessageTimeoutW(hwnd, WM_COPYDATA, 0, &message as *const _ as isize,
            SMTO_ABORTIFHUNG | SMTO_BLOCK, 5000, &mut result) == 0 || result != 1 {
            return Err(format!("Atlas activation was not acknowledged: {}", std::io::Error::last_os_error()));
        }
        Ok(true)
    }
}
pub fn admit(id: &str) -> Result<Option<crate::update_lock::UpdateLock>, String> {
    let gate = crate::update_lock::UpdateLock::named(&format!("Local\\{id}.Startup"), 30_000)?;
    // Failure is fail-closed: never enter application setup and its VPN cleanup
    // when an existing desktop could not acknowledge activation.
    if forward(id)? { Ok(None) } else { Ok(Some(gate)) }
}

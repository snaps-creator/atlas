//! Desktop-session Windows balloon; the privileged service never creates UI.
use std::sync::atomic::{AtomicBool,Ordering};
static VISIBLE: AtomicBool=AtomicBool::new(false);
pub(crate) fn message(updated: bool) -> &'static str {
    if updated { "Не удалось найти доступный сервер, соответствующий вашим настройкам. Подписки обновлены, повторная проверка завершена." }
    else { "Не удалось найти доступный сервер. Обновление подписок не завершено или недоступно; повторная проверка не помогла." }
}
pub(crate) fn show(updated: bool) {
    if VISIBLE.swap(true,Ordering::SeqCst) { return; }
    std::thread::spawn(move || {
        #[cfg(windows)] unsafe {
            use windows_sys::Win32::UI::{Shell::*,WindowsAndMessaging::*};
            let class:Vec<u16>="STATIC\0".encode_utf16().collect();
            let hwnd=CreateWindowExW(0,class.as_ptr(),std::ptr::null(),0,0,0,0,0,
                HWND_MESSAGE,std::ptr::null_mut(),std::ptr::null_mut(),std::ptr::null());
            if !hwnd.is_null() {
                let mut data:NOTIFYICONDATAW=std::mem::zeroed();
                data.cbSize=std::mem::size_of::<NOTIFYICONDATAW>() as u32;
                data.hWnd=hwnd; data.uID=1; data.uFlags=NIF_INFO|NIF_ICON|NIF_TIP;
                data.hIcon=LoadIconW(std::ptr::null_mut(),IDI_INFORMATION);
                for (slot,c) in data.szTip.iter_mut().zip("Atlas VPN".encode_utf16()) { *slot=c; }
                for (slot,c) in data.szInfoTitle.iter_mut().zip("Atlas — VPN-серверы недоступны".encode_utf16()) { *slot=c; }
                for (slot,c) in data.szInfo.iter_mut().zip(message(updated).encode_utf16()) { *slot=c; }
                data.dwInfoFlags=NIIF_WARNING;
                let accepted=Shell_NotifyIconW(NIM_ADD,&data)!=0;
                crate::incident_history::record("RecoveryNotificationDelivered",serde_json::json!({"acceptedByWindows":accepted}),&[]);
                if accepted {
                    let deadline=std::time::Instant::now()+std::time::Duration::from_secs(10);
                    while std::time::Instant::now()<deadline {
                        let mut msg:MSG=std::mem::zeroed();
                        while PeekMessageW(&mut msg,hwnd,0,0,PM_REMOVE)!=0 { TranslateMessage(&msg); DispatchMessageW(&msg); }
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                    Shell_NotifyIconW(NIM_DELETE,&data);
                }
                DestroyWindow(hwnd);
            }
        }
        VISIBLE.store(false,Ordering::SeqCst);
    });
}
#[cfg(test)] mod tests {
    #[test] fn failed_refresh_does_not_claim_success() {
        assert!(!super::message(false).contains("Подписки обновлены"));
        assert!(super::message(true).contains("Подписки обновлены"));
    }
}

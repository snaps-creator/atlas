use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem},
    Icon, TrayIconBuilder,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;
fn main() {
    let evidence = std::path::PathBuf::from(
        std::env::args_os()
            .nth(1)
            .expect("private evidence directory"),
    );
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let signal = done.clone();
    let output = evidence.join("menu.txt");
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        std::fs::write(&output, format!("{}", e.id.0)).unwrap();
        signal.store(true, std::sync::atomic::Ordering::SeqCst);
    }));
    let menu = Menu::new();
    let restart = MenuItem::with_id("restart", "Перезагрузить", true, None);
    menu.append(&restart).unwrap();
    let icon = Icon::from_rgba(vec![64, 128, 255, 255].repeat(256), 16, 16).unwrap();
    let tray = TrayIconBuilder::new()
        .with_id("atlas")
        .with_icon(icon)
        .with_tooltip("Atlas isolated tray fixture")
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()
        .unwrap();
    std::fs::write(
        evidence.join("ready.txt"),
        format!(
            "pid={} hwnd={:?} rect={:?}",
            std::process::id(),
            tray.window_handle(),
            tray.rect()
        ),
    )
    .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        unsafe {
            let mut msg = std::mem::zeroed();
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, 1) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        if done.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
        if std::time::Instant::now() > deadline {
            std::fs::write(
                evidence.join("timeout.txt"),
                format!("rect={:?}", tray.rect()),
            )
            .unwrap();
            drop(tray);
            std::process::exit(2)
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

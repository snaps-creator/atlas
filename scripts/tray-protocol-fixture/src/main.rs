use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem},
    Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;
unsafe extern "system" fn input_window_proc(
    window: windows_sys::Win32::Foundation::HWND,
    message: u32,
    w: windows_sys::Win32::Foundation::WPARAM,
    l: windows_sys::Win32::Foundation::LPARAM,
) -> windows_sys::Win32::Foundation::LRESULT {
    DefWindowProcW(window, message, w, l)
}
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
    let tray_evidence = evidence.join("tray-click.txt");
    TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
        if matches!(
            event,
            TrayIconEvent::Click {
                button: MouseButton::Right,
                button_state: MouseButtonState::Up,
                ..
            }
        ) {
            std::fs::write(&tray_evidence, format!("{event:?}")).unwrap();
        }
    }));
    let menu = Menu::new();
    let restart = MenuItem::with_id("restart", "Перезагрузить", true, None);
    menu.append(&restart).unwrap();
    let disabled = MenuItem::with_id("disabled", "Disabled", false, None);
    menu.append(&disabled).unwrap();
    let icon = Icon::from_rgba(vec![64, 128, 255, 255].repeat(256), 16, 16).unwrap();
    let tray = TrayIconBuilder::new()
        .with_id("atlas")
        .with_icon(icon)
        .with_tooltip("Atlas isolated tray fixture")
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()
        .unwrap();
    // Model the real installed startup-A desktop: a window belonging to this
    // known child can receive genuine input and its normal foreground grant.
    // A hidden tray-only process has no such grant when the test posts the
    // shell callback, so its menu's first click can be eaten as activation.
    let class: Vec<u16> = "AtlasTrayFixtureWindow\0".encode_utf16().collect();
    let title: Vec<u16> = "Atlas isolated tray fixture\0".encode_utf16().collect();
    let input_window = unsafe {
        let instance =
            windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null());
        let definition = WNDCLASSW {
            lpfnWndProc: Some(input_window_proc),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        assert_ne!(RegisterClassW(&definition), 0);
        let window = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            20,
            20,
            300,
            140,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        );
        assert!(!window.is_null());
        assert_ne!(
            SetWindowPos(
                window,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
            ),
            0
        );
        window
    };
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
    unsafe {
        DestroyWindow(input_window);
    }
}

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#[tauri_runtime_cef::cef_entry_point]
fn main() {
    // A short, side-effect-free UI launch for process-name acceptance. It
    // deliberately bypasses Atlas setup, stored preferences, service and VPN.
    if std::env::var_os("ATLAS_UI_PROCESS_SMOKE").as_deref() == Some(std::ffi::OsStr::new("1")) {
        use tauri::Manager;

        let smoke_cache = std::env::temp_dir().join(format!("atlas-cef-smoke-{}", uuid::Uuid::new_v4()));
        let result = tauri::Builder::default()
            .runtime(tauri_runtime_cef::Cef::default().root_cache_path(smoke_cache))
            .invoke_handler(tauri::generate_handler![ui_smoke_ready])
            .on_page_load(|webview, _| {
                let _ = webview.eval(r#"(() => { const timer = setInterval(() => { if (document.querySelector('main') && document.querySelectorAll('button').length >= 5) { clearInterval(timer); window.__TAURI_INTERNALS__.invoke('ui_smoke_ready', { title: document.title, buttons: document.querySelectorAll('button').length }); } }, 100); })()"#);
            })
            .setup(|app| {
                if let Some(window) = app.get_webview_window("main") { let _ = window.hide(); }
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(12));
                    handle.exit(1);
                });
                Ok(())
            })
            .run(tauri::generate_context!());
        std::process::exit(if result.is_ok() { 0 } else { 1 });
    }
    let args: Vec<String> = std::env::args().collect();
    // Packaged service/desktop identity acceptance, with no SCM, VPN or settings.
    if std::env::var_os("ATLAS_SERVICE_IDENTITY_SMOKE").as_deref() == Some(std::ffi::OsStr::new("1")) {
        if args.get(1).is_some_and(|s| s == "--identity-owner") {
            let _ = std::io::stdin().read_line(&mut String::new());
            return;
        }
        if args.get(1).is_some_and(|s| s == "--identity-check") {
            let result = args.get(2).and_then(|pid| pid.parse::<u32>().ok())
                .ok_or("Нет PID владельца".to_owned())
                .and_then(|pid| std::env::current_exe().map_err(|e| e.to_string())
                    .and_then(|path| atlas::verify_desktop_owner(pid, &path)));
            if let Err(error) = &result { eprintln!("{error}"); }
            std::process::exit(if result.is_ok() { 0 } else { 1 });
        }
        std::process::exit(2);
    }
    if args.get(1).is_some_and(|s| s == "--watch-network-session") {
        let result = args
            .get(2)
            .and_then(|s| s.parse::<u32>().ok())
            .filter(|pid| *pid != 0)
            .ok_or_else(|| "Нет PID службы".to_owned())
            .and_then(|pid| args.get(3).and_then(|s| s.parse::<u32>().ok()).filter(|pid| *pid != 0)
                .ok_or("Нет PID владельца сессии".to_owned())
                .and_then(|desktop_pid| args.get(4).ok_or("Нет каталога сессии".to_owned())
                    .and_then(|directory| atlas::watch_network_session(pid, desktop_pid, std::path::Path::new(directory)))));
        if let Err(error) = &result { eprintln!("{error}"); }
        std::process::exit(if result.is_ok() { 0 } else { 1 });
    }
    if args.get(1).is_some_and(|s| s == "--check-network-service") {
        let result = atlas::check_network_service();
        if let Err(ref error) = result {
            eprintln!("{error}");
        }
        std::process::exit(if result.is_ok() { 0 } else { 1 });
    }
    if args.get(1).is_some_and(|s| s == "--network-service") {
        std::process::exit(if atlas::network_service().is_ok() {
            0
        } else {
            1
        });
    }
    if args.get(1).is_some_and(|s| s == "--install-service") {
        std::process::exit(if atlas::install_network_service().is_ok() {
            0
        } else {
            1
        });
    }
    if args.get(1).is_some_and(|s| s == "--uninstall-service") {
        std::process::exit(if atlas::uninstall_network_service().is_ok() {
            0
        } else {
            1
        });
    }
    if std::env::args().any(|arg| arg == "--cleanup") {
        std::process::exit(if atlas::cleanup().is_ok() { 0 } else { 1 });
    }
    atlas::run();
}

// Only registered in the isolated UI acceptance mode above. A window/process
// existing is not success: React must render and CEF IPC must reach this command.
#[tauri::command]
fn ui_smoke_ready(app: tauri::AppHandle, title: String, buttons: usize) {
    if buttons < 5 { return; }
    if let Some(path) = std::env::var_os("ATLAS_UI_SMOKE_REPORT") {
        if std::fs::write(path, serde_json::json!({"rendered":true,"title":title,"buttons":buttons,"version":env!("CARGO_PKG_VERSION")}).to_string()).is_err() { app.exit(2); return; }
    }
    std::thread::spawn(move || { std::thread::sleep(std::time::Duration::from_millis(500)); app.exit(0); });
}

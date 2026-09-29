#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
fn main() {
    // Equivalent to CEF's entry-point dispatch, with ownership established
    // before a helper initializes. A late helper exits if its GUI already died.
    if std::env::args().any(|arg| arg.starts_with("--type=")) {
        if atlas::DesktopJob::join().is_err() { std::process::exit(1); }
        tauri_runtime_cef::run_cef_helper_process();
        return;
    }
    // A short, side-effect-free UI launch for process-name acceptance. It
    // deliberately bypasses Atlas setup, stored preferences, service and VPN.
    if std::env::var_os("ATLAS_UI_PROCESS_SMOKE").as_deref() == Some(std::ffi::OsStr::new("1")) {
        use tauri::Manager;
        let _desktop_job = atlas::DesktopJob::new().expect("Desktop process ownership");
        let smoke_cache = std::env::temp_dir().join(format!("atlas-cef-smoke-{}", uuid::Uuid::new_v4()));
        let result = tauri::Builder::default()
            .runtime(tauri_runtime_cef::Cef::default().root_cache_path(smoke_cache))
            .setup(|app| {
                if let Some(window) = app.get_webview_window("main") { let _ = window.hide(); }
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(12));
                    handle.exit(0);
                });
                Ok(())
            })
            .run(tauri::generate_context!());
        std::process::exit(if result.is_ok() { 0 } else { 1 });
    }
    let args: Vec<String> = std::env::args().collect();
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
    let _desktop_job = match atlas::DesktopJob::new() {
        Ok(job) => job,
        Err(error) => { eprintln!("{error}"); std::process::exit(1); }
    };
    atlas::run();
}

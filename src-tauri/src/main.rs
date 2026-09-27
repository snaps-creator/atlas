#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#[tauri_runtime_cef::cef_entry_point]
fn main() {
    // A short, side-effect-free UI launch for process-name acceptance. It
    // deliberately bypasses Atlas setup, stored preferences, service and VPN.
    if std::env::var_os("ATLAS_UI_PROCESS_SMOKE").as_deref() == Some(std::ffi::OsStr::new("1")) {
        let result = tauri::Builder::default()
            .runtime(tauri_runtime_cef::Cef::default())
            .setup(|app| {
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
            .and_then(|pid| args.get(3).ok_or("Нет каталога сессии".to_owned())
                .and_then(|directory| atlas::watch_network_session(pid, std::path::Path::new(directory))));
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

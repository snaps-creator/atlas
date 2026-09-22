#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|s| s == "--watch-network-session") {
        let result = args
            .get(2)
            .and_then(|s| s.parse::<u32>().ok())
            .filter(|pid| *pid != 0)
            .ok_or_else(|| "Нет PID службы".to_owned())
            .and_then(atlas::watch_network_session);
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

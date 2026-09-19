#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|s| s == "--network-service") {
        std::process::exit(if atlas::network_service().is_ok() {
            0
        } else {
            1
        });
    }
    if args.get(1).is_some_and(|s| s == "--network-helper") {
        let result = args
            .get(2)
            .and_then(|s| s.parse::<u32>().ok())
            .zip(args.get(3))
            .ok_or("Некорректные аргументы".to_string())
            .and_then(|(pid, pipe)| atlas::network_helper(pid, pipe));
        std::process::exit(if result.is_ok() { 0 } else { 1 });
    }
    if std::env::args().any(|arg| arg == "--cleanup") {
        std::process::exit(if atlas::cleanup().is_ok() { 0 } else { 1 });
    }
    atlas::run();
}

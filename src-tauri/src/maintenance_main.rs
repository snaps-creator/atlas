//! Installer-owned recovery, deliberately independent of CEF and the OLD Atlas.
#![allow(dead_code)]
mod lan_policy;
mod network_guard;
mod windows;
mod maintenance;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|a| a == "--inspect") {
        println!("{}", network_guard::tun_diagnostic());
        return;
    }
    let result = if args.len() == 3 && args[1] == "--prepare-install" {
        maintenance::prepare(std::path::Path::new(&args[2]))
    } else {
        Err("Expected --inspect or --prepare-install <installation directory>".into())
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

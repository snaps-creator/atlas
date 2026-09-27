fn main() {
    println!("cargo:rustc-env=ATLAS_BUILD_ID={}",
        std::env::var("ATLAS_BUILD_ID").unwrap_or_else(|_| "local-dev".into()));
    println!("cargo:rerun-if-env-changed=ATLAS_BUILD_ID");
    tauri_build::build()
}

fn main() {
    println!("cargo:rustc-env=ATLAS_BUILD_ID={}",
        std::env::var("ATLAS_BUILD_ID").unwrap_or_else(|_| "local-dev".into()));
    println!("cargo:rerun-if-env-changed=ATLAS_BUILD_ID");
    println!("cargo:rerun-if-env-changed=ATLAS_INSTALL_RECEIPT");
    let receipt=match std::env::var_os("ATLAS_INSTALL_RECEIPT") {
        Some(path)=>{
            println!("cargo:rerun-if-changed={}",std::path::Path::new(&path).display());
            let bytes=std::fs::read(path).expect("Installer payload receipt must be readable");
            assert!(bytes.len()<=1024*1024,"Installer payload receipt exceeds limit");
            bytes
        },
        None=>b"null".to_vec(),
    };
    std::fs::write(std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("install-receipt.json"),receipt)
        .expect("Cannot embed installer payload receipt");
    tauri_build::build()
}

fn main() {
    println!("cargo:rustc-env=ATLAS_BUILD_ID=root-installer-{}",env!("CARGO_PKG_VERSION"));
    std::fs::write(std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("install-receipt.json"),b"null").unwrap();
}

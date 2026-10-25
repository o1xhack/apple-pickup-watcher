fn main() {
    println!("cargo:rerun-if-env-changed=APW_REQUIRE_EMBEDDED_ASSETS");
    if std::env::var_os("APW_REQUIRE_EMBEDDED_ASSETS").is_some() {
        assert!(
            !tauri_build::is_dev(),
            "Installed app requires embedded frontend assets. Use scripts/build-local-us.sh, not plain cargo build."
        );
    }
    tauri_build::build()
}

fn main() {
    // The app's own commands get generated `allow-…` permissions, so the ACL applies to them and
    // the capabilities list exactly what the window may call. `dev_say` exists only in debug
    // builds, which grant it at runtime; release builds have no handler and no grant for it.
    let manifest = tauri_build::AppManifest::new().commands(&[
        "connect",
        "command",
        "session_log",
        "setup_status",
        "setup_models",
        "setup_save",
        "setup_key",
        "setup_download",
        "setup_microphone",
        "setup_finish",
        "setup_open_microphone_settings",
        "dev_say",
    ]);
    let attributes = tauri_build::Attributes::new().app_manifest(manifest);
    if let Err(error) = tauri_build::try_build(attributes) {
        println!("cargo::error=tauri build failed: {error:#}");
    }
}

fn main() {
    // Only these commands exist, and the capability grants exactly them.
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(&["ipc_send", "ipc_status", "open_issue", "fetch_presets", "update_status", "update_install"])),
    )
    .expect("tauri build script");
}

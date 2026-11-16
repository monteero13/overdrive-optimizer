fn main() {
    // Only these two commands exist, and the capability grants exactly them.
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(&["ipc_send", "ipc_status"])),
    )
    .expect("tauri build script");
}

/// App commands get `allow-<command>` permissions (with `_` as `-`), so each
/// window's capability lists exactly the commands it may call.
const COMMANDS: &[&str] = &[
    "ping",
    "search",
    "get_status",
    "list_roots",
    "add_root",
    "remove_root",
    "set_root_enabled",
    "pause_indexing",
    "resume_indexing",
    "rescan_all",
    "open_file",
    "reveal_file",
    "get_settings",
    "update_settings",
    "list_errors",
    "retry_errors",
    "features_status",
    "set_feature_enabled",
    "download_feature",
    "cancel_download",
    "remove_download",
    "clear_index",
];

fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("failed to run tauri-build");
}

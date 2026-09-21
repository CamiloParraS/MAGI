use tauri::Manager;

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
fn ping() -> &'static str {
    magi_core::ping()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // The static asset scope is empty on purpose: the thumbnail cache
            // lives under the magi data directory (`directories`, or
            // `MAGI_DATA_DIR`), which the Tauri identifier can't name, so the
            // one allowed directory is granted here at startup.
            let thumbs = magi_core::thumbs::thumbs_dir();
            std::fs::create_dir_all(&thumbs)?;
            app.asset_protocol_scope().allow_directory(&thumbs, true)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![ping])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

mod commands;

use magi_core::host::{Host, HostEvent, HostPaths};
use tauri::{Emitter, Manager, RunEvent};

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
fn ping() -> &'static str {
    magi_core::ping()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // `open_file`/`reveal_file` use it from Rust; the webview gets no link handler.
        .plugin(
            tauri_plugin_opener::Builder::new()
                .open_js_links_on_click(false)
                .build(),
        )
        .setup(|app| {
            // The static asset scope is empty on purpose: the thumbnail cache
            // lives under the magi data directory (`directories`, or
            // `MAGI_DATA_DIR`), which the Tauri identifier can't name, so the
            // one allowed directory is granted here at startup.
            let thumbs = magi_core::thumbs::thumbs_dir();
            std::fs::create_dir_all(&thumbs)?;
            app.asset_protocol_scope().allow_directory(&thumbs, true)?;

            let events = app.handle().clone();
            let host = Host::start(HostPaths::default(), move |event| {
                // A closed window just misses the event; the next one is complete.
                let _ = match event {
                    HostEvent::Status(status) => events.emit("engine://status", status),
                    HostEvent::Features(features) => events.emit("engine://features", features),
                };
            })?;
            app.manage(host);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ping,
            commands::search,
            commands::get_status,
            commands::list_roots,
            commands::add_root,
            commands::remove_root,
            commands::set_root_enabled,
            commands::pause_indexing,
            commands::resume_indexing,
            commands::rescan_all,
            commands::open_file,
            commands::reveal_file,
            commands::get_settings,
            commands::update_settings,
            commands::list_errors,
            commands::retry_errors,
            commands::features_status,
            commands::set_feature_enabled,
            commands::download_feature,
            commands::cancel_download,
            commands::remove_download,
            commands::clear_index,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let RunEvent::Exit = event
                && let Some(host) = app.try_state::<Host>()
            {
                host.shutdown();
            }
        });
}

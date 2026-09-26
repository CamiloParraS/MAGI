//! Thin Tauri wrappers over `magi_core::host::Host` (SPEC.md §5.7). Each
//! runs on a blocking thread, so the UI thread never waits on the database
//! or a model.

use std::path::{Path, PathBuf};

use magi_core::config::Config;
use magi_core::dto::{
    ErrorCode, FeatureStatus, FileError, IndexStatus, RootStatus, SearchRequest, SearchResponse,
};
use magi_core::features::Feature;
use magi_core::host::Host;
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

type Reply<T> = Result<T, ErrorCode>;

async fn run<T: Send + 'static>(
    host: State<'_, Host>,
    f: impl FnOnce(&Host) -> magi_core::Result<T> + Send + 'static,
) -> Reply<T> {
    let host = host.inner().clone();
    tauri::async_runtime::spawn_blocking(move || f(&host))
        .await
        .map_err(|e| ErrorCode::Internal {
            detail: e.to_string(),
        })?
        .map_err(|e| ErrorCode::from(&e))
}

fn internal(e: impl std::fmt::Display) -> ErrorCode {
    ErrorCode::Internal {
        detail: e.to_string(),
    }
}

#[tauri::command]
pub async fn search(host: State<'_, Host>, request: SearchRequest) -> Reply<SearchResponse> {
    run(host, move |h| h.search(&request)).await
}

#[tauri::command]
pub async fn get_status(host: State<'_, Host>) -> Reply<IndexStatus> {
    run(host, |h| h.engine()?.status()).await
}

#[tauri::command]
pub async fn list_roots(host: State<'_, Host>) -> Reply<Vec<RootStatus>> {
    run(host, |h| Ok(h.engine()?.status()?.roots)).await
}

#[tauri::command]
pub async fn add_root(host: State<'_, Host>, path: String) -> Reply<RootStatus> {
    run(host, move |h| h.engine()?.add_root(Path::new(&path))).await
}

#[tauri::command]
pub async fn remove_root(host: State<'_, Host>, id: i64) -> Reply<()> {
    run(host, move |h| h.engine()?.remove_root(id)).await
}

#[tauri::command]
pub async fn set_root_enabled(host: State<'_, Host>, id: i64, enabled: bool) -> Reply<()> {
    run(host, move |h| h.engine()?.set_root_enabled(id, enabled)).await
}

#[tauri::command]
pub async fn pause_indexing(host: State<'_, Host>) -> Reply<()> {
    run(host, |h| h.engine()?.pause()).await
}

#[tauri::command]
pub async fn resume_indexing(host: State<'_, Host>) -> Reply<()> {
    run(host, |h| h.engine()?.resume()).await
}

#[tauri::command]
pub async fn rescan_all(host: State<'_, Host>) -> Reply<()> {
    // The scan runs on; progress arrives as `engine://status`.
    run(host, |h| h.engine().map(|e| drop(e.rescan()))).await
}

async fn path_of(host: State<'_, Host>, file_id: i64) -> Reply<PathBuf> {
    run(host, move |h| h.file_path(file_id)).await
}

#[tauri::command]
pub async fn open_file(app: AppHandle, host: State<'_, Host>, file_id: i64) -> Reply<()> {
    let path = path_of(host, file_id).await?;
    app.opener()
        .open_path(path.to_string_lossy().into_owned(), None::<&str>)
        .map_err(internal)
}

#[tauri::command]
pub async fn reveal_file(app: AppHandle, host: State<'_, Host>, file_id: i64) -> Reply<()> {
    let path = path_of(host, file_id).await?;
    app.opener().reveal_item_in_dir(path).map_err(internal)
}

#[tauri::command]
pub async fn get_settings(host: State<'_, Host>) -> Reply<Config> {
    run(host, |h| h.settings()).await
}

#[tauri::command]
pub async fn update_settings(host: State<'_, Host>, patch: serde_json::Value) -> Reply<Config> {
    run(host, move |h| h.update_settings(&patch)).await
}

#[tauri::command]
pub async fn list_errors(host: State<'_, Host>, limit: u32) -> Reply<Vec<FileError>> {
    run(host, move |h| h.list_errors(limit)).await
}

#[tauri::command]
pub async fn retry_errors(host: State<'_, Host>) -> Reply<usize> {
    run(host, |h| h.engine()?.retry_errors()).await
}

#[tauri::command]
pub async fn features_status(host: State<'_, Host>) -> Reply<Vec<FeatureStatus>> {
    run(host, |h| h.features().status()).await
}

#[tauri::command]
pub async fn set_feature_enabled(
    host: State<'_, Host>,
    feature: Feature,
    enabled: bool,
) -> Reply<()> {
    run(host, move |h| h.set_feature_enabled(feature, enabled)).await
}

#[tauri::command]
pub async fn download_feature(host: State<'_, Host>, feature: Feature) -> Reply<()> {
    run(host, move |h| h.features().download(feature)).await
}

#[tauri::command]
pub async fn cancel_download(host: State<'_, Host>) -> Reply<()> {
    run(host, |h| {
        h.features().cancel_download();
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn remove_download(host: State<'_, Host>, feature: Feature) -> Reply<()> {
    run(host, move |h| h.features().remove_download(feature)).await
}

#[tauri::command]
pub async fn clear_index(host: State<'_, Host>) -> Reply<()> {
    run(host, |h| h.clear_index()).await
}

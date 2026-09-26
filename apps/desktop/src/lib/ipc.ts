// Every IPC call and event, typed by the ts-rs bindings (SPEC.md §5.7).
// Commands reject with an `ErrorCode`.
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { Config } from "../bindings/Config";
import type { Feature } from "../bindings/Feature";
import type { FeatureStatus } from "../bindings/FeatureStatus";
import type { FileError } from "../bindings/FileError";
import type { IndexingConfig } from "../bindings/IndexingConfig";
import type { IndexStatus } from "../bindings/IndexStatus";
import type { ModelsConfig } from "../bindings/ModelsConfig";
import type { RootStatus } from "../bindings/RootStatus";
import type { SearchRequest } from "../bindings/SearchRequest";
import type { SearchResponse } from "../bindings/SearchResponse";
import type { SearchResult } from "../bindings/SearchResult";
import type { UiConfig } from "../bindings/UiConfig";

/** What `update_settings` accepts: any subset of these sections' fields. */
export type SettingsPatch = {
  indexing?: Partial<IndexingConfig>;
  models?: Partial<ModelsConfig>;
  ui?: Partial<UiConfig>;
};

export const ipc = {
  search: (request: SearchRequest) => invoke<SearchResponse>("search", { request }),
  getStatus: () => invoke<IndexStatus>("get_status"),
  listRoots: () => invoke<RootStatus[]>("list_roots"),
  addRoot: (path: string) => invoke<RootStatus>("add_root", { path }),
  removeRoot: (id: number) => invoke<void>("remove_root", { id }),
  setRootEnabled: (id: number, enabled: boolean) =>
    invoke<void>("set_root_enabled", { id, enabled }),
  pauseIndexing: () => invoke<void>("pause_indexing"),
  resumeIndexing: () => invoke<void>("resume_indexing"),
  rescanAll: () => invoke<void>("rescan_all"),
  openFile: (fileId: number) => invoke<void>("open_file", { fileId }),
  revealFile: (fileId: number) => invoke<void>("reveal_file", { fileId }),
  getSettings: () => invoke<Config>("get_settings"),
  updateSettings: (patch: SettingsPatch) => invoke<Config>("update_settings", { patch }),
  listErrors: (limit: number) => invoke<FileError[]>("list_errors", { limit }),
  retryErrors: () => invoke<number>("retry_errors"),
  featuresStatus: () => invoke<FeatureStatus[]>("features_status"),
  setFeatureEnabled: (feature: Feature, enabled: boolean) =>
    invoke<void>("set_feature_enabled", { feature, enabled }),
  downloadFeature: (feature: Feature) => invoke<void>("download_feature", { feature }),
  cancelDownload: () => invoke<void>("cancel_download"),
  removeDownload: (feature: Feature) => invoke<void>("remove_download", { feature }),
  clearIndex: () => invoke<void>("clear_index"),
};

export const onStatus = (f: (status: IndexStatus) => void): Promise<UnlistenFn> =>
  listen<IndexStatus>("engine://status", (e) => f(e.payload));

export const onFeatures = (f: (status: FeatureStatus[]) => void): Promise<UnlistenFn> =>
  listen<FeatureStatus[]>("engine://features", (e) => f(e.payload));

/** A result's thumbnail URL, through the asset protocol (scoped to the thumbnail cache). */
export const thumbUrl = (r: SearchResult): string | null =>
  r.thumb_path === null ? null : convertFileSrc(r.thumb_path);

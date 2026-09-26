import { vi, test, expect } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
  convertFileSrc: vi.fn((p: string) => `asset://localhost/${p}`),
}));

import { invoke } from "@tauri-apps/api/core";
import { ipc, thumbUrl } from "./ipc";

test("commands send the camelCase argument names Tauri expects", async () => {
  await ipc.openFile(3);
  await ipc.setRootEnabled(2, false);
  await ipc.search({ query: "q", limit: null });
  await ipc.updateSettings({ ui: { language: "es" } });
  expect(invoke).toHaveBeenCalledWith("open_file", { fileId: 3 });
  expect(invoke).toHaveBeenCalledWith("set_root_enabled", { id: 2, enabled: false });
  expect(invoke).toHaveBeenCalledWith("search", { request: { query: "q", limit: null } });
  expect(invoke).toHaveBeenCalledWith("update_settings", { patch: { ui: { language: "es" } } });
});

test("thumbnails go through the asset protocol, absent ones stay absent", () => {
  const base = {
    file_id: 1, path: "/r/a.png", file_name: "a.png", kind: "image" as const, score: 1,
    snippet: null, page: null, modified_at: 0, match_sources: [],
  };
  expect(thumbUrl({ ...base, thumb_path: "/cache/t.webp" })).toBe("asset://localhost//cache/t.webp");
  expect(thumbUrl({ ...base, thumb_path: null })).toBeNull();
});

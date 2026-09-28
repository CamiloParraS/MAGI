// PROTOTYPE (M6 Plan 3) — throwaway. Fake data shaped like the real ts-rs DTOs.
import { useEffect, useState } from "react";
import type { SearchResult } from "../bindings/SearchResult";
import type { IndexStatus } from "../bindings/IndexStatus";
import type { FeatureStatus } from "../bindings/FeatureStatus";
import type { FileError } from "../bindings/FileError";
import type { MatchSource } from "../bindings/MatchSource";
import type { Feature } from "../bindings/Feature";
import type { Snippet } from "../bindings/Snippet";

export type Screen = "search" | "settings";
export type SearchState = "results" | "empty" | "none" | "loading" | "error";
export type ProtoProps = { screen: Screen; state: SearchState };

export const QUERY = "rent payment deadline";

function snip(text: string, ...words: string[]): Snippet {
  const lower = text.toLowerCase();
  const highlights: [number, number][] = [];
  for (const w of words) {
    const i = lower.indexOf(w.toLowerCase());
    if (i >= 0) highlights.push([i, i + w.length]);
  }
  return { text, highlights: highlights.sort((a, b) => a[0] - b[0]) };
}

const day = 86_400_000;
const now = Date.parse("2026-09-27T10:00:00");

export const RESULTS: SearchResult[] = [
  {
    file_id: 1, file_name: "Lease agreement - Calle 93.docx", kind: "office", score: 0.94, page: null, thumb_path: null,
    path: "C:\\Users\\ana\\Documents\\Apartment\\Lease agreement - Calle 93.docx",
    snippet: snip("…Rent is due on the 5th day of each month. A late payment after the deadline incurs a 2% monthly charge…", "Rent", "payment", "deadline"),
    modified_at: now - 40 * day, match_sources: ["keyword", "semantic"],
  },
  {
    file_id: 2, file_name: "arrendamiento-2025.pdf", kind: "pdf", score: 0.88, page: 3, thumb_path: null,
    path: "C:\\Users\\ana\\Documents\\Contratos\\arrendamiento-2025.pdf",
    snippet: snip("…el arrendatario pagará el canon dentro de los primeros cinco días de cada mes, sin necesidad de requerimiento…", "pagará el canon", "cinco días"),
    modified_at: now - 380 * day, match_sources: ["semantic"],
  },
  {
    file_id: 3, file_name: "whatsapp-landlord.png", kind: "image", score: 0.81, page: null, thumb_path: "chat",
    path: "D:\\Photos\\Screenshots\\2026\\whatsapp-landlord.png",
    snippet: snip("Hi Ana! Could you send this month's rent before Friday? The payment app changed", "rent", "payment"),
    modified_at: now - 3 * day, match_sources: ["ocr", "semantic"],
  },
  {
    file_id: 4, file_name: "recibo-julio.jpg", kind: "image", score: 0.74, page: null, thumb_path: "receipt",
    path: "D:\\Photos\\Receipts\\recibo-julio.jpg",
    snippet: snip("PAGO ARRIENDO JULIO  $2.450.000  FECHA LÍMITE 05/07", "PAGO ARRIENDO", "FECHA LÍMITE"),
    modified_at: now - 84 * day, match_sources: ["ocr"],
  },
  {
    file_id: 5, file_name: "budget-2026.xlsx", kind: "office", score: 0.69, page: null, thumb_path: null,
    path: "C:\\Users\\ana\\Documents\\Finance\\budget-2026.xlsx",
    snippet: snip("Rent | 2,450,000 | due 5th | auto payment: no", "Rent", "payment"),
    modified_at: now - 12 * day, match_sources: ["keyword"],
  },
  {
    file_id: 6, file_name: "payments.rs", kind: "code", score: 0.61, page: null, thumb_path: null,
    path: "C:\\Users\\ana\\code\\household\\src\\payments.rs",
    snippet: snip("pub fn next_rent_deadline(today: NaiveDate) -> NaiveDate {", "rent_deadline"),
    modified_at: now - 150 * day, match_sources: ["keyword"],
  },
  {
    file_id: 7, file_name: "IMG_4412.heic", kind: "image", score: 0.52, page: null, thumb_path: "door",
    path: "D:\\Photos\\2025\\Moving day\\IMG_4412.heic",
    snippet: null, modified_at: now - 400 * day, match_sources: ["visual"],
  },
  {
    file_id: 8, file_name: "moving-checklist.md", kind: "text", score: 0.47, page: null, thumb_path: null,
    path: "C:\\Users\\ana\\Notes\\moving-checklist.md",
    snippet: snip("- [ ] set up automatic rent payment with the bank", "rent payment"),
    modified_at: now - 395 * day, match_sources: ["keyword", "filename"],
  },
];

export const STATUS: IndexStatus = {
  state: "indexing", queued: 1284, indexed: 48213, skipped: 902, errors: 3,
  current_file: "D:\\Photos\\2026\\Trip\\IMG_8812.heic",
  roots: [
    { id: 1, path: "C:\\Users\\ana\\Documents", enabled: true, status: "ok" },
    { id: 2, path: "D:\\Photos", enabled: true, status: "ok" },
    { id: 3, path: "C:\\Users\\ana\\code", enabled: false, status: "ok" },
    { id: 4, path: "E:\\Backup 2024", enabled: true, status: "missing" },
  ],
};

export const FEATURES: FeatureStatus[] = [
  { feature: "meaning", enabled: true, download_size: 118_000_000, install: { state: "installed", size_bytes: 118_000_000 }, backfill: null },
  { feature: "image_text", enabled: true, download_size: 21_000_000, install: { state: "downloading", bytes: 13_100_000, total: 21_000_000 }, backfill: null },
  { feature: "image_visual", enabled: false, download_size: 812_000_000, install: { state: "not_installed" }, backfill: null },
];

export const ERRORS: FileError[] = [
  { file_id: 90, path: "C:\\Users\\ana\\Documents\\Taxes\\2025-renta.pdf", code: "extract_failed", detail: "pdfium: password required", attempts: 3 },
  { file_id: 91, path: "C:\\Users\\ana\\Documents\\Work\\~$Q3 plan.docx", code: "locked", detail: "sharing violation (os error 32)", attempts: 3 },
  { file_id: 92, path: "D:\\Photos\\2019\\broken.jpg", code: "read_failed", detail: "unexpected EOF", attempts: 3 },
];

export const FEATURE_NAME: Record<Feature, string> = {
  meaning: "Search by meaning",
  image_text: "Read text in images",
  image_visual: "Find images by what they show",
};
export const FEATURE_HELP: Record<Feature, string> = {
  meaning: "Finds files that talk about your query even when they use other words, in English and Spanish.",
  image_text: "Reads screenshots, scans and photos of documents so their words become searchable.",
  image_visual: "Finds photos by describing them, like “red door” or “beach at sunset”.",
};
export const SOURCE_LABEL: Record<MatchSource, string> = {
  keyword: "Words", semantic: "Meaning", ocr: "Text in image", visual: "Looks like", qr: "QR code", filename: "File name",
};
export const ERROR_LABEL: Record<string, string> = {
  extract_failed: "Couldn't read its contents", locked: "Another app has it open", read_failed: "The file is damaged or unreadable",
};

export const FILE_TYPES = ["Text", "Code", "PDF", "Office documents", "Images"];
export const EXCLUDES = ["**/node_modules/**", "**/.git/**", "**/*.tmp"];

export function mb(bytes: number) {
  return bytes >= 1e9 ? `${(bytes / 1e9).toFixed(1)} GB` : `${Math.round(bytes / 1e6)} MB`;
}
export function ago(ms: number) {
  const d = Math.round((now - ms) / day);
  if (d < 1) return "today";
  if (d < 31) return `${d} days ago`;
  return new Date(ms).toLocaleDateString("en", { month: "short", day: "numeric", year: "numeric" });
}
export function folderOf(path: string) {
  return path.slice(0, path.lastIndexOf("\\"));
}
export function ext(name: string) {
  return name.slice(name.lastIndexOf(".") + 1).toUpperCase();
}

export function Highlighted({ snippet, mark }: { snippet: Snippet; mark: string }) {
  const parts: React.ReactNode[] = [];
  let at = 0;
  snippet.highlights.forEach(([s, e], i) => {
    parts.push(snippet.text.slice(at, s), <mark key={i} className={mark}>{snippet.text.slice(s, e)}</mark>);
    at = e;
  });
  parts.push(snippet.text.slice(at));
  return <>{parts}</>;
}

/** Fake thumbnails: tiny SVG scenes standing in for the real thumbnail cache. */
export function Thumb({ kind, className }: { kind: string | null; className?: string }) {
  const scenes: Record<string, React.ReactNode> = {
    chat: (<><rect width="80" height="80" fill="#e9f5ec" /><rect x="8" y="10" width="46" height="14" rx="6" fill="#fff" /><rect x="26" y="30" width="46" height="18" rx="6" fill="#bfe8c7" /><rect x="8" y="54" width="38" height="14" rx="6" fill="#fff" /></>),
    receipt: (<><rect width="80" height="80" fill="#8a7a66" /><path d="M22 6h36v68l-6-4-6 4-6-4-6 4-6-4-6 4z" fill="#f7f4ee" /><g fill="#9a9489"><rect x="28" y="16" width="24" height="3" /><rect x="28" y="26" width="18" height="2" /><rect x="28" y="32" width="22" height="2" /><rect x="28" y="44" width="24" height="4" /></g></>),
    door: (<><rect width="80" height="80" fill="#d9cdb8" /><rect x="26" y="14" width="28" height="60" fill="#b3261e" /><circle cx="48" cy="46" r="2.5" fill="#e8c46a" /><rect x="0" y="72" width="80" height="8" fill="#8f877a" /></>),
  };
  return (
    <svg viewBox="0 0 80 80" preserveAspectRatio="xMidYMid slice" className={className} aria-hidden>
      {kind ? scenes[kind] : null}
    </svg>
  );
}

/** ↑/↓ moves the selection; wraps. Enough to feel the keyboard flow. */
export function useSelection(count: number) {
  const [sel, setSel] = useState(0);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "ArrowDown") { e.preventDefault(); setSel((s) => (s + 1) % count); }
      if (e.key === "ArrowUp") { e.preventDefault(); setSel((s) => (s - 1 + count) % count); }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [count]);
  return [sel, setSel] as const;
}

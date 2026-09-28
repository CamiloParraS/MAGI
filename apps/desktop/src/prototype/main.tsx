// PROTOTYPE (M6 Plan 3) — throwaway. Three visual directions for the search
// and settings windows, switchable via ?variant=A|B|C and the bar at the bottom.
// Run: `pnpm proto`. Lives only on the prototype/m6-visual branch.
import React, { useEffect, useState } from "react";
import ReactDOM from "react-dom/client";
import "../index.css";
import "./proto.css";
import type { Screen, SearchState } from "./fake";
import { VariantA } from "./VariantA";
import { VariantB } from "./VariantB";
import { VariantC } from "./VariantC";

const VARIANTS = [
  { key: "A", name: "Pane: native and quiet", C: VariantA },
  { key: "B", name: "Ledger: dense, two panes", C: VariantB },
  { key: "C", name: "Star: characterful, grouped", C: VariantC },
];
const SCREENS: Screen[] = ["search", "settings"];
const STATES: SearchState[] = ["results", "empty", "none", "loading", "error"];
const STATE_LABEL: Record<SearchState, string> = {
  results: "Results + indexing hint", empty: "No query yet", none: "No results + feature hint", loading: "Loading", error: "Error",
};

function param(name: string, fallback: string) {
  return new URLSearchParams(location.search).get(name) ?? fallback;
}

function Proto() {
  const [variant, setVariant] = useState(param("variant", "A"));
  const [screen, setScreen] = useState(param("screen", "search") as Screen);
  const [state, setState] = useState(param("state", "results") as SearchState);
  const [theme, setTheme] = useState(
    param("theme", matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light"),
  );

  useEffect(() => {
    const p = new URLSearchParams({ variant, screen, state, theme });
    history.replaceState(null, "", `?${p}`);
    document.documentElement.dataset.theme = theme;
  }, [variant, screen, state, theme]);

  const i = Math.max(0, VARIANTS.findIndex((v) => v.key === variant));
  const cycle = (d: number) => setVariant(VARIANTS[(i + d + VARIANTS.length) % VARIANTS.length].key);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement;
      if (t.closest("input, textarea, select, [contenteditable]")) return;
      if (e.key === "ArrowLeft") cycle(-1);
      if (e.key === "ArrowRight") cycle(1);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const V = VARIANTS[i];
  return (
    <>
      <div className="proto-desk">
        <V.C key={V.key + screen + state} screen={screen} state={state} />
      </div>
      <div className="proto-bar" role="toolbar" aria-label="Prototype controls">
        <button onClick={() => cycle(-1)} aria-label="Previous variant">◀</button>
        <strong>{V.key}</strong> <span>{V.name}</span>
        <button onClick={() => cycle(1)} aria-label="Next variant">▶</button>
        <select value={screen} onChange={(e) => setScreen(e.target.value as Screen)} aria-label="Screen">
          {SCREENS.map((s) => <option key={s}>{s}</option>)}
        </select>
        <select value={state} onChange={(e) => setState(e.target.value as SearchState)} disabled={screen !== "search"} aria-label="Search state">
          {STATES.map((s) => <option key={s} value={s}>{STATE_LABEL[s]}</option>)}
        </select>
        <button onClick={() => setTheme(theme === "dark" ? "light" : "dark")}>{theme === "dark" ? "Dark" : "Light"}</button>
      </div>
    </>
  );
}

if (import.meta.env.DEV) {
  ReactDOM.createRoot(document.getElementById("root")!).render(<React.StrictMode><Proto /></React.StrictMode>);
}

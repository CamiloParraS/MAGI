// PROTOTYPE variant B, "Ledger": dense, for power users. Two panes: a tight
// result list on the left and a full preview of the selected file on the right.
// Settings are one filterable page, and each row shows its config key.
import { useState } from "react";
import "./b.css";
import {
  ago, ERROR_LABEL, ERRORS, EXCLUDES, ext, FEATURE_HELP, FEATURE_NAME, FEATURES, FILE_TYPES, folderOf,
  Highlighted, QUERY, RESULTS, SOURCE_LABEL, STATUS, Thumb, useSelection, mb, type ProtoProps,
} from "./fake";

export function VariantB({ screen, state }: ProtoProps) {
  return <div className="vb">{screen === "search" ? <Search state={state} /> : <Settings />}</div>;
}

const TABS = ["All", "Documents", "Images", "Code"];

function Search({ state }: { state: ProtoProps["state"] }) {
  const [sel, setSel] = useSelection(RESULTS.length);
  const r = RESULTS[sel];
  const query = state === "empty" ? "" : state === "none" ? "red umbrella beach" : QUERY;
  const pct = Math.round((STATUS.indexed / (STATUS.indexed + STATUS.queued)) * 100);
  return (
    <section className="vb-search" aria-label="Search">
      <header className="vb-top">
        <input autoFocus defaultValue={query} placeholder="Search files…" aria-label="Search files" />
        <div className="vb-tabs" role="tablist">
          {TABS.map((t, i) => <button key={t} role="tab" aria-selected={i === 0}>{t}</button>)}
        </div>
      </header>

      <div className="vb-body">
        {state === "results" || state === "loading" ? (
          <>
            <ul className={`vb-list ${state === "loading" ? "vb-stale" : ""}`} role="listbox" aria-label="Results">
              {RESULTS.map((x, i) => (
                <li key={x.file_id} role="option" aria-selected={i === sel} onClick={() => setSel(i)}>
                  <span className="vb-ext">{ext(x.file_name)}</span>
                  <span className="vb-name">{x.file_name}</span>
                  <span className="vb-date">{ago(x.modified_at)}</span>
                </li>
              ))}
            </ul>
            <article className="vb-preview" aria-label="Preview">
              {r.thumb_path && <Thumb kind={r.thumb_path} className="vb-thumb" />}
              <h2>{r.file_name}</h2>
              {r.snippet && <blockquote><Highlighted snippet={r.snippet} mark="vb-mark" /></blockquote>}
              <dl>
                <dt>Folder</dt><dd className="vb-mono">{folderOf(r.path)}</dd>
                <dt>Modified</dt><dd>{new Date(r.modified_at).toLocaleDateString("en", { dateStyle: "medium" })}</dd>
                {r.page && <><dt>Page</dt><dd>{r.page}</dd></>}
                <dt>Matched by</dt>
                <dd className="vb-chips">{r.match_sources.map((s) => <span key={s} className={`vb-chip vb-s-${s}`}>{SOURCE_LABEL[s]}</span>)}</dd>
              </dl>
              <div className="vb-actions">
                <button className="vb-btn vb-go">Open <kbd>↵</kbd></button>
                <button className="vb-btn">Show in folder <kbd>Ctrl ↵</kbd></button>
                <button className="vb-btn">Copy path <kbd>Ctrl C</kbd></button>
              </div>
            </article>
          </>
        ) : (
          <div className="vb-msg">
            {state === "empty" && <p>Type to search. Words, file names and meaning are all searched at once.</p>}
            {state === "none" && (
              <>
                <p>Nothing matches “red umbrella beach”.</p>
                <p className="vb-hint">
                  Photos can be found by what they show once <b>{FEATURE_NAME.image_visual}</b> is on.{" "}
                  <button className="vb-btn vb-go">Turn on, {mb(FEATURES[2].download_size)}</button>{" "}
                  <button className="vb-btn">Not now</button>
                </p>
              </>
            )}
            {state === "error" && (<><p>Search failed: the index is restarting.</p><button className="vb-btn">Retry</button></>)}
          </div>
        )}
      </div>

      <footer className="vb-status">
        <span className="vb-dot" /> Indexing {STATUS.queued.toLocaleString("en")} files, {pct}% done
        <span className="vb-bar"><span style={{ width: `${pct}%` }} /></span>
        <span className="vb-mono vb-cur">{STATUS.current_file}</span>
      </footer>
    </section>
  );
}

type Row = { label: string; key: string; help?: string; control: React.ReactNode };

function Settings() {
  const [filter, setFilter] = useState("");
  const sections: [string, Row[]][] = [
    ["Folders", STATUS.roots.map((r): Row => ({
      label: r.path, key: `roots[${r.id - 1}]`,
      help: r.status === "missing" ? "Not found. Reconnect the drive; its index is kept." : r.enabled ? "Watching" : "Paused",
      control: <><input type="checkbox" defaultChecked={r.enabled} aria-label="Enabled" /> <button className="vb-btn">Remove</button></>,
    })).concat([{ label: "Add a folder", key: "roots", control: <button className="vb-btn vb-go">Add…</button> }])],
    ["Search features", FEATURES.map((f) => ({
      label: FEATURE_NAME[f.feature], key: `features.${f.feature}`, help: FEATURE_HELP[f.feature],
      control: (
        <span className="vb-feat">
          {f.install.state === "downloading" && <><span className="vb-bar"><span style={{ width: `${(f.install.bytes / f.install.total) * 100}%` }} /></span> {mb(f.install.bytes)}/{mb(f.install.total)} <button className="vb-btn">Cancel</button></>}
          {f.install.state === "installed" && <>{mb(f.install.size_bytes)} <button className="vb-btn">Remove download</button></>}
          {f.install.state === "not_installed" && <>{mb(f.download_size)}</>}
          <input type="checkbox" defaultChecked={f.enabled} aria-label="Enabled" />
        </span>
      ),
    }))],
    ["Indexing", [
      { label: "File types to read", key: "indexing.file_types", help: "Others are found by name only.", control: <span className="vb-types">{FILE_TYPES.map((t) => <label key={t}><input type="checkbox" defaultChecked /> {t}</label>)}</span> },
      { label: "Largest file to read", key: "indexing.max_file_size_mb", control: <><input className="vb-in" type="number" defaultValue={100} /> MB</> },
      { label: "Skip paths", key: "indexing.exclude_globs", control: <textarea className="vb-in vb-mono" rows={3} defaultValue={EXCLUDES.join("\n")} /> },
      { label: "Pause on battery", key: "indexing.pause_on_battery", control: <input type="checkbox" defaultChecked /> },
    ]],
    ["App", [
      { label: "Search shortcut", key: "ui.hotkey", help: "Click, then press the keys.", control: <button className="vb-btn vb-mono">Ctrl+Shift+Space</button> },
      { label: "Start with your computer", key: "ui.launch_at_login", control: <input type="checkbox" defaultChecked /> },
      { label: "Language", key: "ui.language", control: <select className="vb-in"><option>System (English)</option><option>English</option><option>Español</option></select> },
      { label: "Window background", key: "ui.transparency_mode", control: <select className="vb-in"><option>Match system</option><option>See-through</option><option>Solid</option></select> },
      { label: "See-through amount", key: "ui.transparency_intensity", control: <input type="range" defaultValue={60} aria-label="See-through amount" /> },
    ]],
    ["Index", [
      { label: `${STATUS.indexed.toLocaleString("en")} indexed, ${STATUS.queued.toLocaleString("en")} queued, ${STATUS.skipped} skipped`, key: "status", control: <button className="vb-btn">Pause</button> },
      ...ERRORS.map((e) => ({ label: e.path, key: `error ${e.code}`, help: `${ERROR_LABEL[e.code]} (${e.attempts} attempts)`, control: <button className="vb-btn">Retry</button> })),
      { label: "Clear index", key: "clear_index", help: "Deletes what Magi has learned about your files. Your files are not touched.", control: <button className="vb-btn vb-danger">Clear…</button> },
    ]],
  ];
  const q = filter.toLowerCase();
  return (
    <section className="vb-settings" aria-label="Settings">
      <nav className="vb-toc">
        <input className="vb-in" placeholder="Filter settings" value={filter} onChange={(e) => setFilter(e.target.value)} aria-label="Filter settings" />
        {sections.map(([name]) => <a key={name} href={`#vb-${name}`}>{name}</a>)}
      </nav>
      <div className="vb-page">
        {sections.map(([name, rows]) => {
          const shown = rows.filter((r) => !q || `${r.label} ${r.key} ${r.help ?? ""}`.toLowerCase().includes(q));
          if (!shown.length) return null;
          return (
            <section key={name} id={`vb-${name}`}>
              <h2>{name}</h2>
              {shown.map((r) => (
                <div className="vb-row" key={r.key}>
                  <div><div>{r.label}</div>{r.help && <div className="vb-help">{r.help}</div>}<code className="vb-key">{r.key}</code></div>
                  <div className="vb-ctl">{r.control}</div>
                </div>
              ))}
            </section>
          );
        })}
      </div>
    </section>
  );
}

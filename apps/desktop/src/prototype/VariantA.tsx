// PROTOTYPE variant A, "Pane": native and quiet. Feels like part of the OS.
// One translucent column, the snippet only under the selected row, and
// settings as a sidebar plus grouped rows (the Windows 11 / macOS pattern).
import { useState } from "react";
import "./a.css";
import {
  ago, ERROR_LABEL, ERRORS, EXCLUDES, FEATURE_HELP, FEATURE_NAME, FEATURES, FILE_TYPES, folderOf,
  Highlighted, QUERY, RESULTS, SOURCE_LABEL, STATUS, Thumb, useSelection, mb, type ProtoProps,
} from "./fake";
import type { SearchResult } from "../bindings/SearchResult";

const KIND_GLYPH: Record<string, string> = { pdf: "PDF", office: "DOC", code: "</>", text: "TXT", image: "", other: "•" };

export function VariantA({ screen, state }: ProtoProps) {
  return <div className="va">{screen === "search" ? <Search state={state} /> : <Settings />}</div>;
}

function Icon({ r }: { r: SearchResult }) {
  return r.thumb_path
    ? <Thumb kind={r.thumb_path} className="va-thumb" />
    : <span className={`va-glyph va-k-${r.kind}`}>{KIND_GLYPH[r.kind]}</span>;
}

function Search({ state }: { state: ProtoProps["state"] }) {
  const [sel, setSel] = useSelection(RESULTS.length);
  const query = state === "empty" ? "" : state === "none" ? "red umbrella beach" : QUERY;
  return (
    <section className="va-search" aria-label="Search">
      <div className="va-input">
        <svg viewBox="0 0 20 20" aria-hidden><circle cx="8.5" cy="8.5" r="5.5" /><path d="M13 13l4 4" /></svg>
        <input autoFocus defaultValue={query} placeholder="Search your files" aria-label="Search your files" />
      </div>
      {state === "loading" && <div className="va-loadbar" role="progressbar" aria-label="Searching" />}

      {state === "results" && (
        <>
          <ul className="va-list" role="listbox" aria-label="Results">
            {RESULTS.map((r, i) => (
              <li key={r.file_id} role="option" aria-selected={i === sel} className="va-row" onMouseEnter={() => setSel(i)}>
                <Icon r={r} />
                <div className="va-main">
                  <div className="va-name">{r.file_name}{r.page && <span className="va-page"> · page {r.page}</span>}</div>
                  {i === sel && r.snippet
                    ? <p className="va-snip"><Highlighted snippet={r.snippet} mark="va-mark" /></p>
                    : <div className="va-path">{folderOf(r.path)}</div>}
                </div>
                <div className="va-meta">
                  <span>{ago(r.modified_at)}</span>
                  {i === sel && <span className="va-src">{r.match_sources.map((s) => SOURCE_LABEL[s]).join(", ")}</span>}
                </div>
              </li>
            ))}
          </ul>
          <footer className="va-foot">
            <span>Still indexing, {STATUS.queued.toLocaleString("en")} files to go. Results may be incomplete.</span>
            <span className="va-keys"><kbd>↵</kbd> Open <kbd>Ctrl ↵</kbd> Show in folder <kbd>Ctrl C</kbd> Copy path</span>
          </footer>
        </>
      )}
      {state === "loading" && (
        <ul className="va-list va-dim" aria-hidden>
          {RESULTS.slice(0, 4).map((r) => <li key={r.file_id} className="va-row"><Icon r={r} /><div className="va-main"><div className="va-name">{r.file_name}</div><div className="va-path">{folderOf(r.path)}</div></div></li>)}
        </ul>
      )}
      {state === "empty" && (
        <div className="va-empty">
          <p>Search {STATUS.indexed.toLocaleString("en")} files by name, words, or what they're about.</p>
          <span className="va-keys"><kbd>↑</kbd><kbd>↓</kbd> Move <kbd>↵</kbd> Open <kbd>Esc</kbd> Close</span>
        </div>
      )}
      {state === "none" && (
        <div className="va-empty">
          <p>No files match “red umbrella beach”.</p>
          <div className="va-hint" role="note">
            <div>
              <strong>{FEATURE_NAME.image_visual}</strong> is off. It can find photos by describing them.
            </div>
            <button className="va-btn va-primary">Turn on ({mb(FEATURES[2].download_size)})</button>
            <button className="va-x" aria-label="Dismiss">×</button>
          </div>
        </div>
      )}
      {state === "error" && (
        <div className="va-empty">
          <p>Couldn't search the index. Magi is restarting it.</p>
          <button className="va-btn">Try again</button>
        </div>
      )}
    </section>
  );
}

const SECTIONS = ["Folders", "Search features", "What to index", "Shortcut and startup", "Appearance", "Index"] as const;

function Settings() {
  const [section, setSection] = useState<(typeof SECTIONS)[number]>("Folders");
  return (
    <section className="va-settings" aria-label="Settings">
      <nav className="va-nav">
        <div className="va-title">Settings</div>
        {SECTIONS.map((s) => (
          <button key={s} aria-current={s === section} onClick={() => setSection(s)}>{s}</button>
        ))}
      </nav>
      <div className="va-pane">
        <h1>{section}</h1>
        {section === "Folders" && (
          <>
            <div className="va-group">
              {STATUS.roots.map((r) => (
                <div className="va-set" key={r.id}>
                  <div>
                    <div>{r.path}</div>
                    <div className="va-sub">
                      {r.status === "missing" ? <span className="va-warn">Folder not found. Reconnect the drive; the index is kept.</span> : r.enabled ? "Watching for changes" : "Paused, not searched"}
                    </div>
                  </div>
                  <label className="va-switch"><input type="checkbox" defaultChecked={r.enabled} /><span /></label>
                  <button className="va-btn">Remove</button>
                </div>
              ))}
            </div>
            <button className="va-btn">Add folder</button>
          </>
        )}
        {section === "Search features" && (
          <div className="va-group">
            {FEATURES.map((f) => (
              <div className="va-set" key={f.feature}>
                <div>
                  <div>{FEATURE_NAME[f.feature]}</div>
                  <div className="va-sub">{FEATURE_HELP[f.feature]}</div>
                  {f.install.state === "downloading" && (
                    <div className="va-prog"><progress value={f.install.bytes} max={f.install.total} /> {mb(f.install.bytes)} of {mb(f.install.total)} <button className="va-link">Cancel</button></div>
                  )}
                  {f.install.state === "installed" && <div className="va-sub">Downloaded, {mb(f.install.size_bytes)} <button className="va-link">Remove download</button></div>}
                  {f.install.state === "not_installed" && <div className="va-sub">{mb(f.download_size)} download when turned on</div>}
                </div>
                <label className="va-switch"><input type="checkbox" defaultChecked={f.enabled} /><span /></label>
              </div>
            ))}
          </div>
        )}
        {section === "What to index" && (
          <div className="va-group">
            <div className="va-set"><div>File types<div className="va-sub">Other files are found by name only.</div></div>
              <div className="va-chips">{FILE_TYPES.map((t) => <label key={t}><input type="checkbox" defaultChecked /> {t}</label>)}</div></div>
            <div className="va-set"><div>Largest file to read</div><div><input className="va-num" type="number" defaultValue={100} /> MB</div></div>
            <div className="va-set va-col"><div>Skip these paths<div className="va-sub">One pattern per line.</div></div>
              <textarea className="va-area" defaultValue={EXCLUDES.join("\n")} rows={3} /></div>
          </div>
        )}
        {section === "Shortcut and startup" && (
          <div className="va-group">
            <div className="va-set"><div>Open search<div className="va-sub">Press a new shortcut to change it.</div></div><button className="va-btn va-kbd">Ctrl + Shift + Space</button></div>
            <div className="va-set"><div>Start with your computer</div><label className="va-switch"><input type="checkbox" defaultChecked /><span /></label></div>
            <div className="va-set"><div>Pause indexing on battery</div><label className="va-switch"><input type="checkbox" defaultChecked /><span /></label></div>
          </div>
        )}
        {section === "Appearance" && (
          <div className="va-group">
            <div className="va-set"><div>Language</div><select className="va-select"><option>Same as system (English)</option><option>English</option><option>Español</option></select></div>
            <div className="va-set"><div>Search window background</div><select className="va-select"><option>Match system</option><option>Always see-through</option><option>Always solid</option></select></div>
            <div className="va-set"><div>See-through amount</div><div className="va-range">More solid <input type="range" defaultValue={60} /> More transparent</div></div>
          </div>
        )}
        {section === "Index" && (
          <>
            <div className="va-group">
              <div className="va-set"><div>{STATUS.indexed.toLocaleString("en")} files indexed<div className="va-sub">{STATUS.queued.toLocaleString("en")} waiting, {STATUS.skipped} skipped. Now reading {STATUS.current_file}</div></div><button className="va-btn">Pause</button></div>
            </div>
            <h2>Files that couldn't be indexed</h2>
            <div className="va-group">
              {ERRORS.map((e) => (
                <div className="va-set" key={e.file_id}><div>{e.path}<div className="va-sub">{ERROR_LABEL[e.code]}</div></div><button className="va-btn">Retry</button></div>
              ))}
            </div>
            <button className="va-btn va-danger">Clear index…</button>
          </>
        )}
      </div>
    </section>
  );
}

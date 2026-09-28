// PROTOTYPE variant C, "Star": has its own character (Magi as the wise men
// following a star). The query is set large, and results are grouped by *how*
// Magi found them, with pictures as tiles. Settings open with a plain-language
// status sentence, and every section folds away.
import "./c.css";
import {
  ago, ERROR_LABEL, ERRORS, EXCLUDES, FEATURE_HELP, FEATURE_NAME, FEATURES, FILE_TYPES, folderOf,
  Highlighted, QUERY, RESULTS, SOURCE_LABEL, STATUS, Thumb, useSelection, mb, type ProtoProps,
} from "./fake";
import type { SearchResult } from "../bindings/SearchResult";

export function VariantC({ screen, state }: ProtoProps) {
  return <div className="vc">{screen === "search" ? <Search state={state} /> : <Settings />}</div>;
}

function Star({ className }: { className?: string }) {
  return <svg viewBox="0 0 24 24" className={className} aria-hidden><path d="M12 1l2.6 8.4L23 12l-8.4 2.6L12 23l-2.6-8.4L1 12l8.4-2.6z" /></svg>;
}

const words = (r: SearchResult) => r.kind !== "image" && r.match_sources.some((s) => s === "keyword" || s === "filename");
const GROUPS: [string, SearchResult[]][] = [
  ["Using your words", RESULTS.filter(words)],
  ["About the same thing", RESULTS.filter((r) => r.kind !== "image" && !words(r))],
  ["In your pictures", RESULTS.filter((r) => r.kind === "image")],
];
const ORDER = GROUPS.flatMap(([, rs]) => rs);

function Search({ state }: { state: ProtoProps["state"] }) {
  const [sel, setSel] = useSelection(ORDER.length);
  const current = ORDER[sel];
  const query = state === "empty" ? "" : state === "none" ? "red umbrella beach" : QUERY;
  return (
    <section className="vc-search" aria-label="Search">
      <div className="vc-input">
        <Star className={`vc-star ${state === "loading" ? "vc-pulse" : ""}`} />
        <input autoFocus defaultValue={query} placeholder="What are you looking for?" aria-label="What are you looking for?" />
      </div>

      {(state === "results" || state === "loading") && (
        <div className={state === "loading" ? "vc-stale" : ""}>
          {GROUPS.map(([title, rs]) => (
            <section key={title} className="vc-group">
              <h3>{title}</h3>
              {title === "In your pictures" ? (
                <div className="vc-tiles">
                  {rs.map((r) => (
                    <figure key={r.file_id} aria-selected={r === current} onClick={() => setSel(ORDER.indexOf(r))}>
                      <Thumb kind={r.thumb_path} className="vc-thumb" />
                      <figcaption>
                        <div className="vc-name">{r.file_name}</div>
                        <div className="vc-why">{r.snippet ? <Highlighted snippet={r.snippet} mark="vc-mark" /> : r.match_sources.map((s) => SOURCE_LABEL[s]).join(", ")}</div>
                      </figcaption>
                    </figure>
                  ))}
                </div>
              ) : (
                <ul>
                  {rs.map((r) => (
                    <li key={r.file_id} aria-selected={r === current} onClick={() => setSel(ORDER.indexOf(r))}>
                      <Star className="vc-bullet" />
                      <div>
                        <div className="vc-name">{r.file_name} <span className="vc-where">in {folderOf(r.path).split("\\").pop()}{r.page ? `, page ${r.page}` : ""}, {ago(r.modified_at)}</span></div>
                        {r.snippet && <p className="vc-snip"><Highlighted snippet={r.snippet} mark="vc-mark" /></p>}
                      </div>
                    </li>
                  ))}
                </ul>
              )}
            </section>
          ))}
          <p className="vc-foot">Magi is still reading {STATUS.queued.toLocaleString("en")} files, so more may turn up. <kbd>Enter</kbd> opens, <kbd>Ctrl Enter</kbd> shows it in its folder.</p>
        </div>
      )}
      {state === "empty" && <p className="vc-quiet">Describe it in your own words, in English or Spanish.</p>}
      {state === "none" && (
        <div className="vc-quiet">
          <p>Nothing found for “red umbrella beach”.</p>
          <div className="vc-offer" role="note">
            <p>Magi can't see what's in your photos yet. Turn on <b>{FEATURE_NAME.image_visual}</b> to find them by describing them.</p>
            <button className="vc-btn vc-gold">Turn on ({mb(FEATURES[2].download_size)} download)</button>
            <button className="vc-btn">Dismiss</button>
          </div>
        </div>
      )}
      {state === "error" && (
        <div className="vc-quiet"><p>Magi couldn't search just now; the index is restarting.</p><button className="vc-btn">Search again</button></div>
      )}
    </section>
  );
}

function Settings() {
  const roots = STATUS.roots.filter((r) => r.enabled).length;
  const trouble = STATUS.roots.filter((r) => r.status !== "ok").length;
  return (
    <section className="vc-settings" aria-label="Settings">
      <header className="vc-hero">
        <Star className="vc-star" />
        <p className="vc-big">Magi has read {STATUS.indexed.toLocaleString("en")} files in {roots} folders.</p>
        <p className="vc-sub">{STATUS.queued.toLocaleString("en")} still to go. Reading {STATUS.current_file?.split("\\").pop()} now.</p>
        <button className="vc-btn">Pause</button>
      </header>

      <details open>
        <summary>Folders <span>{trouble ? `${trouble} needs attention` : "All good"}</span></summary>
        {STATUS.roots.map((r) => (
          <div className="vc-row" key={r.id}>
            <div>{r.path}{r.status === "missing" && <div className="vc-warn">Not found. Reconnect the drive and Magi picks up where it left off.</div>}</div>
            <label><input type="checkbox" defaultChecked={r.enabled} /> Search</label>
            <button className="vc-btn">Remove</button>
          </div>
        ))}
        <button className="vc-btn vc-gold">Add a folder</button>
      </details>

      <details>
        <summary>Search features <span>1 on, 1 downloading, 1 off</span></summary>
        {FEATURES.map((f) => (
          <div className="vc-row" key={f.feature}>
            <div>
              <b>{FEATURE_NAME[f.feature]}</b>
              <div className="vc-dim">{FEATURE_HELP[f.feature]}</div>
              {f.install.state === "downloading" && <div className="vc-dim"><progress value={f.install.bytes} max={f.install.total} /> {mb(f.install.bytes)} of {mb(f.install.total)}</div>}
            </div>
            <label><input type="checkbox" defaultChecked={f.enabled} /> {f.install.state === "not_installed" ? `On (${mb(f.download_size)})` : "On"}</label>
            {f.install.state === "installed" && <button className="vc-btn">Remove download</button>}
            {f.install.state === "downloading" && <button className="vc-btn">Cancel</button>}
          </div>
        ))}
      </details>

      <details>
        <summary>What Magi reads <span>{FILE_TYPES.length} kinds, up to 100 MB</span></summary>
        <div className="vc-row"><div>Kinds of files</div><div className="vc-types">{FILE_TYPES.map((t) => <label key={t}><input type="checkbox" defaultChecked /> {t}</label>)}</div></div>
        <div className="vc-row"><div>Largest file</div><label><input className="vc-in" type="number" defaultValue={100} /> MB</label></div>
        <div className="vc-row vc-stack"><div>Skip these paths, one per line</div><textarea className="vc-in" rows={3} defaultValue={EXCLUDES.join("\n")} /></div>
        <div className="vc-row"><div>Pause while on battery</div><input type="checkbox" defaultChecked /></div>
      </details>

      <details>
        <summary>Shortcut, startup and look <span>Ctrl + Shift + Space</span></summary>
        <div className="vc-row"><div>Open Magi with</div><button className="vc-btn">Ctrl + Shift + Space</button></div>
        <div className="vc-row"><div>Start with your computer</div><input type="checkbox" defaultChecked /></div>
        <div className="vc-row"><div>Language</div><select className="vc-in"><option>Same as system (English)</option><option>English</option><option>Español</option></select></div>
        <div className="vc-row"><div>Search window background</div><select className="vc-in"><option>Match system</option><option>See-through</option><option>Solid</option></select></div>
        <div className="vc-row"><div>How see-through</div><label className="vc-dim">More solid <input type="range" defaultValue={60} /> More transparent</label></div>
      </details>

      <details>
        <summary>Problems <span>{ERRORS.length} files couldn't be read</span></summary>
        {ERRORS.map((e) => (
          <div className="vc-row" key={e.file_id}><div>{e.path.split("\\").pop()}<div className="vc-dim">{ERROR_LABEL[e.code]}</div></div><button className="vc-btn">Try again</button></div>
        ))}
        <div className="vc-row"><div>Start over<div className="vc-dim">Forgets everything Magi has read. Your files stay as they are.</div></div><button className="vc-btn vc-danger">Clear index…</button></div>
      </details>
    </section>
  );
}

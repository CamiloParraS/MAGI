//! What a desktop host needs from the core (SPEC.md §5.3): the engine, the
//! search features and search, behind one cloneable handle. One supervisor
//! thread owns the engine's lifecycle. It starts the engine, forwards the
//! engine's status and the features' state as [`HostEvent`]s, and restarts
//! the engine when what it loaded at start is stale: a feature's desire or
//! install state, the settings it reads, or a cleared index.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};
use std::thread::JoinHandle;
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender, select};
use rusqlite::{Connection, OptionalExtension};

use crate::config::{self, Config};
use crate::db::{self, files};
use crate::discovery::Kind;
use crate::dto::{
    FeatureStatus, FileError, IndexStatus, Install, MatchSource, SearchRequest, SearchResponse,
    SearchResult, Snippet,
};
use crate::embed::manager::{ModelManifest, UreqFetcher, models_root};
use crate::engine::{Engine, EngineHandle};
use crate::error::{Error, Result};
use crate::features::{Components, Feature, Features, load_components};
use crate::search::{SearchHit, hybrid_search};

pub enum HostEvent {
    Status(IndexStatus),
    Features(Vec<FeatureStatus>),
}

#[derive(Debug, Clone)]
pub struct HostPaths {
    pub db: PathBuf,
    pub config: PathBuf,
}

impl Default for HostPaths {
    fn default() -> Self {
        Self {
            db: crate::paths::data_dir().join("magi.db"),
            config: config::config_path(),
        }
    }
}

#[derive(Clone)]
pub struct Host {
    inner: Arc<Shared>,
}

struct Shared {
    paths: HostPaths,
    features: Features,
    running: RwLock<Option<Running>>,
    /// Held across every read-modify-write of `config.toml`.
    config: Mutex<()>,
    control: Sender<Control>,
    supervisor: Mutex<Option<JoinHandle<()>>>,
    /// Search reads here, never the engine's connections (`query_only`).
    reader: Mutex<Connection>,
}

#[derive(Clone)]
struct Running {
    engine: EngineHandle,
    components: Components,
}

enum Control {
    Restart,
    Stop,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Host {
    /// Opens the features, then starts the engine
    /// on the supervisor thread; [`engine`](Self::engine) fails with
    /// `EngineStarting` until the first reconciliation scan is done.
    pub fn start(paths: HostPaths, on_event: impl Fn(HostEvent) + Send + 'static) -> Result<Self> {
        let features = Features::start_with(
            &paths.db,
            paths.config.clone(),
            models_root(),
            ModelManifest::load()?,
            Arc::new(UreqFetcher),
        )?;
        let feature_rx = features.subscribe();
        let reader = db::open(&paths.db)?;
        reader.pragma_update(None, "query_only", true)?;
        let (control, control_rx) = crossbeam_channel::unbounded();
        let host = Self {
            inner: Arc::new(Shared {
                paths,
                features,
                running: RwLock::new(None),
                config: Mutex::new(()),
                control,
                supervisor: Mutex::new(None),
                reader: Mutex::new(reader),
            }),
        };
        let shared = host.inner.clone();
        let thread = std::thread::Builder::new()
            .name("magi-host".into())
            .spawn(move || supervise(&shared, &control_rx, &feature_rx, &on_event))
            .map_err(|e| Error::Engine(format!("could not start the host thread: {e}")))?;
        *lock(&host.inner.supervisor) = Some(thread);
        Ok(host)
    }

    /// The running engine, or `EngineStarting` while it starts or restarts.
    pub fn engine(&self) -> Result<EngineHandle> {
        self.inner
            .running()
            .map(|r| r.engine)
            .ok_or(Error::EngineStarting)
    }

    /// The features the running engine has loaded.
    pub fn running(&self) -> Vec<Feature> {
        self.inner
            .running()
            .map(|r| r.components.running())
            .unwrap_or_default()
    }

    pub fn features(&self) -> &Features {
        &self.inner.features
    }

    /// Records the desire; the engine restarts with or without the feature
    /// when the features' state reflects it.
    pub fn set_feature_enabled(&self, feature: Feature, on: bool) -> Result<()> {
        let _config = lock(&self.inner.config);
        self.inner.features.set_enabled(feature, on)
    }

    /// Hybrid search with whatever features the running engine has, or
    /// keywords alone while it (re)starts. Indexing yields to it.
    pub fn search(&self, request: &SearchRequest) -> Result<SearchResponse> {
        let started = Instant::now();
        let limit = match request.limit {
            Some(limit) => limit,
            None => self.inner.load_config()?.ui.max_results,
        };
        let running = self.inner.running();
        let _yield = running.as_ref().map(|r| r.engine.search_pending().guard());
        let components = running.map(|r| r.components).unwrap_or_default();
        let conn = lock(&self.inner.reader);
        let hits = hybrid_search(
            &conn,
            components.text.as_deref(),
            components.image.as_deref(),
            &request.query,
            limit,
        )?;
        let mut results = Vec::with_capacity(hits.len());
        for hit in hits {
            results.extend(result_of(&conn, hit)?);
        }
        Ok(SearchResponse {
            results,
            took_ms: started.elapsed().as_millis() as u64,
        })
    }

    pub fn file_path(&self, file_id: i64) -> Result<PathBuf> {
        files::path_of(&lock(&self.inner.reader), file_id)
    }

    pub fn list_errors(&self, limit: u32) -> Result<Vec<FileError>> {
        files::list_errors(&lock(&self.inner.reader), limit)
    }

    /// Stops a running download (its partial files stay), then the engine
    /// and the supervisor. Idempotent.
    pub fn shutdown(&self) {
        self.inner.features.cancel_download();
        let _ = self.inner.control.send(Control::Stop);
        if let Some(thread) = lock(&self.inner.supervisor).take() {
            let _ = thread.join();
        }
    }
}

impl Shared {
    fn running(&self) -> Option<Running> {
        self.running
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn load_config(&self) -> Result<Config> {
        config::load_from(&self.paths.config)
    }

    fn start_engine(&self) -> Option<Receiver<IndexStatus>> {
        let started = self.load_config().and_then(|config| {
            let components = load_components(&config.features);
            let engine = Engine::start(&config, &self.paths.db, components.clone())?;
            Ok(Running { engine, components })
        });
        match started {
            Ok(running) => {
                let rx = running.engine.subscribe();
                *self.running.write().unwrap_or_else(PoisonError::into_inner) = Some(running);
                Some(rx)
            }
            Err(e) => {
                tracing::error!(error = %e, "the engine did not start; waiting for a change");
                None
            }
        }
    }

    fn stop_engine(&self) {
        let taken = self
            .running
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(running) = taken {
            running.engine.shutdown();
        }
    }
}

/// What the engine's components depend on: each feature's desire and
/// whether it is on disk. Download progress and backfill leave it equal.
/// ponytail: downloading a feature that is off also restarts once; compare
/// the runnable set instead if that restart shows up on large roots.
fn engine_inputs(status: &[FeatureStatus]) -> Vec<(Feature, bool, bool)> {
    status
        .iter()
        .map(|s| {
            (
                s.feature,
                s.enabled,
                matches!(s.install, Install::Installed { .. }),
            )
        })
        .collect()
}

fn supervise(
    shared: &Shared,
    control: &Receiver<Control>,
    features: &Receiver<Vec<FeatureStatus>>,
    on_event: &dyn Fn(HostEvent),
) {
    loop {
        // Read before starting: `start_engine` loads config and components and
        // then blocks in `Engine::start`, so a feature change that lands during
        // that window must still show up as a mismatch below, not be baked into
        // `inputs` as if it were already running.
        let inputs = shared.features.status().ok().map(|s| engine_inputs(&s));
        let mut status_rx = shared
            .start_engine()
            .unwrap_or_else(crossbeam_channel::never);
        if let Some(running) = shared.running()
            && let Ok(status) = running.engine.status()
        {
            on_event(HostEvent::Status(status));
        }
        let next = loop {
            select! {
                recv(status_rx) -> status => match status {
                    Ok(status) => on_event(HostEvent::Status(status)),
                    Err(_) => status_rx = crossbeam_channel::never(),
                },
                recv(features) -> status => match status {
                    Ok(status) => {
                        let changed = inputs.as_ref() != Some(&engine_inputs(&status));
                        on_event(HostEvent::Features(status));
                        if changed {
                            break Control::Restart;
                        }
                    }
                    Err(_) => break Control::Stop,
                },
                recv(control) -> next => break next.unwrap_or(Control::Stop),
            }
        };
        shared.stop_engine();
        match next {
            Control::Restart => {}
            Control::Stop => return,
        }
    }
}

/// Joins in what the UI shows. `None` when the file was deleted after it
/// was ranked: that result is dropped, not the search.
fn result_of(conn: &Connection, hit: SearchHit) -> Result<Option<SearchResult>> {
    let Some((file_name, kind, mtime_ns, thumb_key)) = conn
        .query_row(
            "SELECT file_name, kind, mtime_ns, thumb_key FROM files WHERE id = ?1",
            [hit.file_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .optional()?
    else {
        return Ok(None);
    };
    let visual_only = hit.match_sources == ["visual"];
    Ok(Some(SearchResult {
        file_id: hit.file_id,
        path: hit.path.to_string_lossy().into_owned(),
        file_name,
        kind: serde_json::from_value(serde_json::Value::String(kind)).unwrap_or(Kind::Other),
        score: hit.score,
        snippet: (!visual_only).then(|| Snippet::from_marked(&hit.snippet)),
        page: hit.page,
        thumb_path: thumb_key.map(|key| {
            crate::thumbs::thumb_path(&key)
                .to_string_lossy()
                .into_owned()
        }),
        modified_at: mtime_ns / 1_000_000,
        match_sources: hit
            .match_sources
            .iter()
            .filter_map(|s| MatchSource::from_wire(s))
            .collect(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::{Backfill, Install};

    fn status(install: Install, backfill: Option<Backfill>) -> Vec<FeatureStatus> {
        vec![FeatureStatus {
            feature: Feature::Meaning,
            enabled: true,
            download_size: 100,
            install,
            backfill,
        }]
    }

    #[test]
    fn a_hit_whose_file_is_gone_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open(&dir.path().join("magi.db")).unwrap();
        let hit = crate::search::SearchHit {
            file_id: 42,
            path: PathBuf::from("/gone.txt"),
            score: 1.0,
            snippet: "x".into(),
            match_sources: vec!["keyword"],
            page: None,
        };
        assert!(result_of(&conn, hit).unwrap().is_none());
    }

    #[test]
    fn progress_and_backfill_do_not_change_engine_inputs() {
        let base = engine_inputs(&status(
            Install::Downloading {
                bytes: 1,
                total: 100,
            },
            None,
        ));
        assert_eq!(
            base,
            engine_inputs(&status(
                Install::Downloading {
                    bytes: 99,
                    total: 100
                },
                None
            ))
        );
        let installed = engine_inputs(&status(Install::Installed { size_bytes: 100 }, None));
        assert_ne!(base, installed, "finishing a download restarts");
        assert_eq!(
            installed,
            engine_inputs(&status(
                Install::Installed { size_bytes: 100 },
                Some(Backfill { done: 3, total: 9 })
            ))
        );
    }
}

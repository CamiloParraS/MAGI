//! The desktop host over the real engine with the fake embedder
//! (`MAGI_FAKE_EMBEDDER=1`, SPEC.md §4.5).

use std::path::Path;
use std::sync::Once;
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use magi_core::config::{Config, FeaturesConfig};
use magi_core::dto::{SearchRequest, SearchResult};
use magi_core::features::Feature;
use magi_core::host::{Host, HostEvent, HostPaths};

fn setup() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let dir = std::env::temp_dir().join(format!("magi-host-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: every test calls this first; `Once` finishes before any proceeds.
        unsafe {
            std::env::set_var("MAGI_DATA_DIR", dir);
            std::env::set_var("MAGI_FAKE_EMBEDDER", "1");
        }
    });
}

fn wait_until(deadline: Duration, what: &str, mut cond: impl FnMut() -> bool) {
    let start = Instant::now();
    while !cond() {
        assert!(start.elapsed() < deadline, "timed out waiting for: {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn meaning_only() -> FeaturesConfig {
    FeaturesConfig {
        meaning: true,
        image_text: false,
        image_visual: false,
    }
}

struct Env {
    _dir: tempfile::TempDir,
    root: tempfile::TempDir,
    host: Host,
    events: Receiver<HostEvent>,
}

impl Env {
    fn start(features: FeaturesConfig) -> Self {
        setup();
        let dir = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let paths = HostPaths {
            db: dir.path().join("magi.db"),
            config: dir.path().join("config.toml"),
        };
        let config = Config {
            features,
            ..Config::default()
        };
        std::fs::write(&paths.config, toml::to_string(&config).unwrap()).unwrap();
        let (tx, events) = crossbeam_channel::unbounded();
        let host = Host::start(paths, move |e| {
            let _ = tx.send(e);
        })
        .unwrap();
        wait_until(Duration::from_secs(30), "the engine", || {
            host.engine().is_ok()
        });
        host.engine().unwrap().add_root(root.path()).unwrap();
        Self {
            _dir: dir,
            root,
            host,
            events,
        }
    }

    fn root(&self) -> &Path {
        self.root.path()
    }

    fn write(&self, name: &str, text: &str) {
        std::fs::write(self.root().join(name), text).unwrap();
    }

    fn hits(&self, query: &str) -> Vec<SearchResult> {
        self.host
            .search(&SearchRequest {
                query: query.into(),
                limit: None,
            })
            .unwrap()
            .results
    }
}

#[test]
fn the_host_forwards_engine_status_and_feature_state() {
    let env = Env::start(meaning_only());
    env.write("a.txt", "quarterly invoice");
    let (mut indexed, mut features) = (false, false);
    let start = Instant::now();
    while !(indexed && features) {
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "no status/features events"
        );
        match env.events.recv_timeout(Duration::from_secs(1)) {
            Ok(HostEvent::Status(s)) => indexed |= s.indexed == 1,
            Ok(HostEvent::Features(f)) => features |= f.len() == Feature::ALL.len(),
            Err(_) => {}
        }
    }
}

#[test]
fn disabling_a_feature_restarts_the_engine_without_it() {
    let env = Env::start(meaning_only());
    assert_eq!(env.host.running(), vec![Feature::Meaning]);
    env.host
        .set_feature_enabled(Feature::Meaning, false)
        .unwrap();
    wait_until(Duration::from_secs(30), "a restart without meaning", || {
        env.host.engine().is_ok() && env.host.running().is_empty()
    });
}

#[test]
fn shutdown_stops_the_engine() {
    let env = Env::start(meaning_only());
    env.host.shutdown();
    assert!(matches!(
        env.host.engine(),
        Err(magi_core::Error::EngineStarting)
    ));
}

#[test]
fn search_returns_metadata_highlights_and_sources() {
    let env = Env::start(meaning_only());
    env.write("invoice.txt", "the quarterly invoice is due");
    wait_until(Duration::from_secs(30), "the file to be searchable", || {
        !env.hits("quarterly").is_empty()
    });
    let hit = &env.hits("quarterly")[0];
    assert_eq!(
        (hit.file_name.as_str(), hit.kind),
        ("invoice.txt", magi_core::discovery::Kind::Text)
    );
    assert!(hit.modified_at > 0);
    let snippet = hit.snippet.as_ref().unwrap();
    assert_eq!(snippet.highlights.len(), 1);
    assert!(
        hit.match_sources
            .contains(&magi_core::dto::MatchSource::Keyword)
    );
    assert_eq!(
        env.host.file_path(hit.file_id).unwrap(),
        magi_core::paths::canonicalize(env.root())
            .unwrap()
            .join("invoice.txt")
    );
}

#[test]
fn search_falls_back_to_keywords_while_the_engine_is_down() {
    let env = Env::start(meaning_only());
    env.write("a.txt", "zebra stripes");
    wait_until(Duration::from_secs(30), "indexed", || {
        !env.hits("zebra").is_empty()
    });
    env.host.shutdown();
    let hits = env.hits("zebra");
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].match_sources,
        vec![magi_core::dto::MatchSource::Keyword]
    );
}

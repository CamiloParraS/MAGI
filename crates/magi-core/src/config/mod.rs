//! Load/save/validate `config.toml`, per-OS default exclusions.
//! Implemented in M1 (see SPEC.md §7 M1, §5.2 user config file).

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::discovery::Kind;
use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RootConfig {
    pub path: PathBuf,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct IndexingConfig {
    pub exclude_globs: Vec<String>,
    pub include_hidden: bool,
    pub follow_symlinks: bool,
    pub max_file_size_mb: u64,
    /// Kinds that get content extraction; others are indexed by filename
    /// only. An unknown name fails deserialization, listing the valid ones.
    pub file_types: Vec<Kind>,
    pub pause_on_battery: bool,
    pub worker_threads: u32,
    pub max_image_megapixels: u32,
    pub reconcile_interval_hours: u32,
}

impl Default for IndexingConfig {
    fn default() -> Self {
        Self {
            exclude_globs: vec![
                "**/node_modules/**".into(),
                "**/.git/**".into(),
                "**/target/**".into(),
                "**/.venv/**".into(),
                "**/__pycache__/**".into(),
                "**/*.tmp".into(),
                "**/*.part".into(),
                "**/*.crdownload".into(),
                "**/~$*".into(),
                "**/.~lock.*".into(),
            ],
            include_hidden: false,
            follow_symlinks: false,
            max_file_size_mb: 50,
            file_types: vec![Kind::Text, Kind::Code, Kind::Pdf, Kind::Office, Kind::Image],
            pause_on_battery: true,
            worker_threads: 0,
            max_image_megapixels: 64,
            reconcile_interval_hours: 6,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelsConfig {
    pub idle_unload_minutes: u32,
}

impl Default for ModelsConfig {
    fn default() -> Self {
        Self {
            idle_unload_minutes: 5,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub hotkey: String,
    pub theme: String,
    pub max_results: u32,
    pub launch_at_login: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            hotkey: "CmdOrCtrl+Shift+Space".into(),
            theme: "system".into(),
            max_results: 30,
            launch_at_login: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub schema_version: u32,
    pub roots: Vec<RootConfig>,
    pub indexing: IndexingConfig,
    pub models: ModelsConfig,
    pub ui: UiConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: 1,
            roots: Vec::new(),
            indexing: IndexingConfig::default(),
            models: ModelsConfig::default(),
            ui: UiConfig::default(),
        }
    }
}

impl Config {
    /// Rejects bad exclude globs and roots that don't exist on disk.
    pub fn validate(&self) -> Result<()> {
        for glob in &self.indexing.exclude_globs {
            glob::Pattern::new(glob).map_err(|e| Error::InvalidGlob {
                glob: glob.clone(),
                reason: e.to_string(),
            })?;
        }
        for root in &self.roots {
            if !root.path.exists() {
                return Err(Error::RootNotFound(root.path.clone()));
            }
        }
        Ok(())
    }
}

/// Path to `config.toml` under the resolved config directory.
pub fn config_path() -> PathBuf {
    crate::paths::config_dir().join("config.toml")
}

/// Loads the config, writing out defaults on first run.
pub fn load() -> Result<Config> {
    load_from(&config_path())
}

/// Validates and atomically saves the config (temp file + rename).
pub fn save(config: &Config) -> Result<()> {
    save_to(&config_path(), config)
}

fn load_from(path: &Path) -> Result<Config> {
    match fs::read_to_string(path) {
        Ok(raw) => {
            let config: Config = toml::from_str(&raw)?;
            config.validate()?;
            Ok(config)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let config = Config::default();
            save_to(path, &config)?;
            Ok(config)
        }
        Err(source) => Err(Error::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn save_to(path: &Path, config: &Config) -> Result<()> {
    config.validate()?;
    let dir = path.parent().expect("config path always has a parent");
    fs::create_dir_all(dir).map_err(|source| Error::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    let raw = toml::to_string_pretty(config)?;
    let tmp_path = dir.join(".config.toml.tmp");
    fs::write(&tmp_path, raw).map_err(|source| Error::Io {
        path: tmp_path.clone(),
        source,
    })?;
    fs::rename(&tmp_path, path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let root = tempfile::tempdir().unwrap();

        let mut config = Config::default();
        config.roots.push(RootConfig {
            path: root.path().to_path_buf(),
            enabled: true,
        });

        save_to(&path, &config).unwrap();
        let loaded = load_from(&path).unwrap();
        assert_eq!(config, loaded);
    }

    #[test]
    fn missing_file_creates_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let loaded = load_from(&path).unwrap();
        assert_eq!(loaded, Config::default());
        assert!(path.exists());
    }

    #[test]
    fn unknown_file_type_is_rejected_at_load() {
        // A typo would otherwise silently stop indexing that kind's content.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            "[indexing]
file_types = [\"text\", \"pdfs\"]
",
        )
        .unwrap();
        assert!(matches!(load_from(&path), Err(Error::ConfigParse(_))));
    }

    #[test]
    fn bad_glob_is_rejected() {
        let mut config = Config::default();
        config.indexing.exclude_globs.push("[unterminated".into());
        assert!(matches!(config.validate(), Err(Error::InvalidGlob { .. })));
    }

    #[test]
    fn nonexistent_root_is_rejected() {
        let mut config = Config::default();
        config.roots.push(RootConfig {
            path: PathBuf::from("/does/not/exist/hopefully"),
            enabled: true,
        });
        assert!(matches!(config.validate(), Err(Error::RootNotFound(_))));
    }

    #[test]
    fn crash_between_write_and_rename_leaves_previous_config_intact() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let mut config = Config::default();
        config.ui.max_results = 42;
        save_to(&path, &config).unwrap();

        // Simulate a crash: the temp file is written but the rename that
        // would replace config.toml never happens.
        let tmp_path = dir.path().join(".config.toml.tmp");
        fs::write(&tmp_path, "schema_version = 999").unwrap();

        let loaded = load_from(&path).unwrap();
        assert_eq!(loaded.ui.max_results, 42);
    }
}

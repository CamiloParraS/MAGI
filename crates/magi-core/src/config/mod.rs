//! Load/save/validate `config.toml`, per-OS default exclusions.
//! Implemented in M1 (see SPEC.md §7 M1, §5.2 user config file).

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::discovery::Kind;
use crate::error::{Error, Result};
use crate::features::Feature;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct RootConfig {
    #[ts(type = "string")]
    pub path: PathBuf,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(default)]
#[ts(export)]
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
                // Unity's regenerated caches: one project's Library held 35k
                // of a 41k-file benchmark folder (docs/benchmarks.md).
                "**/Library/PackageCache/**".into(),
                "**/Library/Bee/**".into(),
                "**/Library/ShaderCache/**".into(),
                "**/Library/BurstCache/**".into(),
                "**/Library/ScriptAssemblies/**".into(),
                "**/*.meta".into(),
                // Build output: binaries nobody searches for by name.
                "**/*.dll".into(),
                "**/*.pdb".into(),
                "**/*.obj".into(),
                "**/*.o".into(),
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(default)]
#[ts(export)]
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

/// Which optional search features the user wants (ADR-0010). Desired state
/// only: whether each one is downloaded is on disk, see `features`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(default)]
#[ts(export)]
pub struct FeaturesConfig {
    pub meaning: bool,
    pub image_text: bool,
    pub image_visual: bool,
}

impl Default for FeaturesConfig {
    fn default() -> Self {
        Self {
            meaning: true,
            image_text: true,
            image_visual: false,
        }
    }
}

impl FeaturesConfig {
    pub fn enabled(&self, feature: Feature) -> bool {
        match feature {
            Feature::Meaning => self.meaning,
            Feature::ImageText => self.image_text,
            Feature::ImageVisual => self.image_visual,
        }
    }

    pub fn set(&mut self, feature: Feature, on: bool) {
        *match feature {
            Feature::Meaning => &mut self.meaning,
            Feature::ImageText => &mut self.image_text,
            Feature::ImageVisual => &mut self.image_visual,
        } = on;
    }
}

/// `system` resolves any `es-*` OS locale to Spanish, everything else to
/// English (resolution happens in the UI; the core only stores the choice).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum Language {
    System,
    En,
    Es,
}

/// Search-window transparency (ADR-0010).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TransparencyMode {
    MatchSystem,
    Always,
    Never,
}

/// Allowed `ui.transparency_intensity`: below it text over a busy wallpaper
/// stops being readable; above it the effect is invisible.
pub const TRANSPARENCY_INTENSITY: std::ops::RangeInclusive<f32> = 0.40..=0.95;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(default)]
#[ts(export)]
pub struct UiConfig {
    pub hotkey: String,
    pub theme: String,
    pub max_results: u32,
    pub launch_at_login: bool,
    pub language: Language,
    pub transparency_mode: TransparencyMode,
    /// Alpha of the tint drawn over the native effect.
    pub transparency_intensity: f32,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            hotkey: "CmdOrCtrl+Shift+Space".into(),
            theme: "system".into(),
            max_results: 30,
            launch_at_login: false,
            language: Language::System,
            transparency_mode: TransparencyMode::MatchSystem,
            transparency_intensity: 0.75,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(default)]
#[ts(export)]
pub struct Config {
    pub schema_version: u32,
    pub roots: Vec<RootConfig>,
    pub indexing: IndexingConfig,
    pub models: ModelsConfig,
    pub features: FeaturesConfig,
    pub ui: UiConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: 1,
            roots: Vec::new(),
            indexing: IndexingConfig::default(),
            models: ModelsConfig::default(),
            features: FeaturesConfig::default(),
            ui: UiConfig::default(),
        }
    }
}

impl Config {
    /// Rejects out-of-range UI settings, bad exclude globs and roots that don't exist on disk.
    pub fn validate(&self) -> Result<()> {
        if !TRANSPARENCY_INTENSITY.contains(&self.ui.transparency_intensity) {
            return Err(Error::InvalidSetting {
                field: "ui.transparency_intensity",
                reason: format!(
                    "{} is outside {:?}",
                    self.ui.transparency_intensity, TRANSPARENCY_INTENSITY
                ),
            });
        }
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

/// Sections `update_settings` may change. Roots live in the database (root
/// commands); features have their own commands (ADR-0010).
const PATCHABLE: [&str; 3] = ["indexing", "models", "ui"];

/// Applies a partial settings object and validates the result. Objects
/// merge; anything else (numbers, strings, lists) replaces. A key the config
/// does not have is an error, so a typo never silently does nothing.
pub fn apply_patch(config: &Config, patch: &serde_json::Value) -> Result<Config> {
    let invalid = |reason: String| Error::InvalidSetting {
        field: "settings",
        reason,
    };
    let serde_json::Value::Object(sections) = patch else {
        return Err(invalid("expected an object".into()));
    };
    if let Some(key) = sections.keys().find(|k| !PATCHABLE.contains(&k.as_str())) {
        return Err(invalid(format!("{key} cannot be changed here")));
    }
    let mut merged = serde_json::to_value(config).map_err(|e| invalid(e.to_string()))?;
    merge(&mut merged, patch, "")?;
    let next: Config = serde_json::from_value(merged).map_err(|e| invalid(e.to_string()))?;
    next.validate()?;
    Ok(next)
}

fn merge(target: &mut serde_json::Value, patch: &serde_json::Value, path: &str) -> Result<()> {
    use serde_json::Value;
    match (target, patch) {
        (Value::Object(target), Value::Object(patch)) => {
            for (key, value) in patch {
                let path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                let slot = target.get_mut(key).ok_or_else(|| Error::InvalidSetting {
                    field: "settings",
                    reason: format!("unknown setting {path}"),
                })?;
                merge(slot, value, &path)?;
            }
        }
        (target, patch) => *target = patch.clone(),
    }
    Ok(())
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

pub(crate) fn load_from(path: &Path) -> Result<Config> {
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

pub(crate) fn save_to(path: &Path, config: &Config) -> Result<()> {
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
    #[test]
    fn feature_and_ui_defaults_match_adr_0010() {
        let c = Config::default();
        assert!(c.features.meaning && c.features.image_text && !c.features.image_visual);
        assert_eq!(c.ui.language, Language::System);
        assert_eq!(c.ui.transparency_mode, TransparencyMode::MatchSystem);
        assert_eq!(c.ui.transparency_intensity, 0.75);
    }

    #[test]
    fn features_and_ui_fields_round_trip_with_their_wire_names() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            "[features]
image_visual = true
image_text = false
             [ui]
language = \"es\"
transparency_mode = \"never\"
transparency_intensity = 0.4
",
        )
        .unwrap();
        let c = load_from(&path).unwrap();
        assert!(c.features.meaning, "missing keys keep their default");
        assert!(!c.features.image_text && c.features.image_visual);
        assert_eq!(c.ui.language, Language::Es);
        assert_eq!(c.ui.transparency_mode, TransparencyMode::Never);
        save_to(&path, &c).unwrap();
        assert_eq!(load_from(&path).unwrap(), c);
    }

    #[test]
    fn transparency_intensity_outside_its_range_is_rejected() {
        for bad in [0.39_f32, 0.96, f32::NAN] {
            let mut c = Config::default();
            c.ui.transparency_intensity = bad;
            assert!(
                matches!(
                    c.validate(),
                    Err(Error::InvalidSetting {
                        field: "ui.transparency_intensity",
                        ..
                    })
                ),
                "{bad} accepted"
            );
        }
    }

    #[test]
    fn an_unknown_language_is_rejected_at_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            "[ui]
language = \"fr\"
",
        )
        .unwrap();
        assert!(matches!(load_from(&path), Err(Error::ConfigParse(_))));
    }

    #[test]
    fn features_config_reads_and_writes_each_feature() {
        let mut f = FeaturesConfig::default();
        f.set(Feature::ImageVisual, true);
        f.set(Feature::Meaning, false);
        assert!(f.enabled(Feature::ImageVisual) && !f.enabled(Feature::Meaning));
    }

    #[test]
    fn a_settings_patch_merges_into_the_current_config() {
        let next = apply_patch(
            &Config::default(),
            &serde_json::json!({"ui": {"language": "es"}, "indexing": {"max_file_size_mb": 10}}),
        )
        .unwrap();
        assert_eq!(next.ui.language, Language::Es);
        assert_eq!(next.indexing.max_file_size_mb, 10);
        assert_eq!(
            next.ui.hotkey,
            UiConfig::default().hotkey,
            "untouched fields kept"
        );
    }

    #[test]
    fn a_settings_patch_rejects_typos_bad_values_and_protected_sections() {
        use serde_json::json;
        for patch in [
            json!({"ui": {"langauge": "es"}}),
            json!({"ui": {"transparency_intensity": 2.0}}),
            json!({"ui": {"language": "fr"}}),
            json!({"indexing": {"exclude_globs": ["[unclosed"]}}),
            json!({"features": {"meaning": false}}),
            json!({"roots": []}),
            json!(["ui"]),
        ] {
            assert!(
                matches!(
                    apply_patch(&Config::default(), &patch),
                    Err(Error::InvalidSetting { .. } | Error::InvalidGlob { .. })
                ),
                "{patch}"
            );
        }
    }
}

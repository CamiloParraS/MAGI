//! Load/save/validate `config.toml`, per-OS default exclusions (SPEC.md §5.2).

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::discovery::Kind;
use crate::error::{Error, Result};
use crate::features::Feature;

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

/// Which optional search features the user wants (ADR-0010). Desired state
/// only: whether each one is downloaded is on disk, see `features`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    System,
    En,
    Es,
}

/// Search-window transparency (ADR-0010).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransparencyMode {
    MatchSystem,
    Always,
    Never,
}

/// The first-run onboarding step to resume at (FR-10); each step saves the
/// next when it completes, the background step saves `Done`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Onboarding {
    Folders,
    Features,
    Background,
    Done,
}

/// Allowed `ui.transparency_intensity`: below it text over a busy wallpaper
/// stops being readable; above it the effect is invisible.
pub const TRANSPARENCY_INTENSITY: std::ops::RangeInclusive<f32> = 0.40..=0.95;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub hotkey: String,
    pub theme: String,
    pub max_results: u32,
    pub launch_at_login: bool,
    /// Copying a result puts the file on the clipboard (paste it in a file
    /// manager); off copies its path as text.
    pub copy_as_file: bool,
    /// The search window splits into the results and a pane with the
    /// selected one's preview and details. On for new installs; a config
    /// saved before the setting existed keeps the plain list, hence the
    /// field's own default rather than [`UiConfig::default`]'s.
    #[serde(default)]
    pub detail_pane: bool,
    pub language: Language,
    pub transparency_mode: TransparencyMode,
    /// Alpha of the tint drawn over the native effect.
    pub transparency_intensity: f32,
    pub onboarding: Onboarding,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            hotkey: "CmdOrCtrl+Shift+Space".into(),
            theme: "system".into(),
            max_results: 30,
            launch_at_login: false,
            copy_as_file: true,
            detail_pane: true,
            language: Language::System,
            transparency_mode: TransparencyMode::MatchSystem,
            transparency_intensity: 0.75,
            onboarding: Onboarding::Folders,
        }
    }
}

/// Ask Magi state that is not a secret (ADR-0014). The login itself lives in
/// the OS keychain, never here.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AskConfig {
    /// Shown as "signed in as"; the keychain is not read at launch.
    pub signed_in_as: Option<String>,
    /// The first-use disclosure was accepted.
    pub disclosed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub schema_version: u32,
    pub roots: Vec<RootConfig>,
    pub indexing: IndexingConfig,
    pub models: ModelsConfig,
    pub features: FeaturesConfig,
    pub ui: UiConfig,
    pub ask: AskConfig,
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
            ask: AskConfig::default(),
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
mod tests;

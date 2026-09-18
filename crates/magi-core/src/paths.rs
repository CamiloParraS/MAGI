//! Data/config/cache directory resolution, honoring `MAGI_*` env overrides,
//! and path canonicalization (see SPEC.md §4.5, §5.1).

use std::env;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;

use crate::error::{Error, Result};

fn project_dirs() -> ProjectDirs {
    ProjectDirs::from("dev", "magi", "magi").expect("no home directory found for this platform")
}

/// Data directory: database, models, thumbnails. Overridable via `MAGI_DATA_DIR`.
pub fn data_dir() -> PathBuf {
    env::var_os("MAGI_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| project_dirs().data_dir().to_path_buf())
}

/// Config directory: `config.toml`. Overridable via `MAGI_CONFIG_DIR`.
pub fn config_dir() -> PathBuf {
    env::var_os("MAGI_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| project_dirs().config_dir().to_path_buf())
}

/// Cache directory, nested under the data directory so `MAGI_DATA_DIR`
/// relocates it too (no separate `MAGI_CACHE_DIR` override exists).
pub fn cache_dir() -> PathBuf {
    data_dir().join("cache")
}

/// Resolves `path` to an absolute, symlink-resolved form with no trailing
/// separator and (on Windows) filesystem-canonical drive-letter case. The
/// path must exist.
pub fn canonicalize(path: &Path) -> Result<PathBuf> {
    dunce::canonicalize(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_dotdot_segments() {
        let dir = tempfile::tempdir().unwrap();
        let child = dir.path().join("child");
        std::fs::create_dir(&child).unwrap();

        let messy = child.join("..").join("child");
        let canonical = canonicalize(&messy).unwrap();
        assert_eq!(canonical, canonicalize(&child).unwrap());
    }

    #[test]
    fn strips_trailing_separator() {
        let dir = tempfile::tempdir().unwrap();
        let with_slash = PathBuf::from(format!(
            "{}{}",
            dir.path().display(),
            std::path::MAIN_SEPARATOR
        ));

        let canonical = canonicalize(&with_slash).unwrap();
        assert!(
            !canonical
                .to_string_lossy()
                .ends_with(std::path::MAIN_SEPARATOR)
        );
        assert_eq!(canonical, canonicalize(dir.path()).unwrap());
    }

    #[cfg(windows)]
    #[test]
    fn windows_drive_letter_case_is_normalized() {
        let dir = tempfile::tempdir().unwrap();
        let path_str = dir.path().to_string_lossy().to_string();
        let (drive, rest) = path_str.split_at(1);
        let lower = format!("{}{}", drive.to_lowercase(), rest);
        let upper = format!("{}{}", drive.to_uppercase(), rest);

        assert_eq!(
            canonicalize(Path::new(&lower)).unwrap(),
            canonicalize(Path::new(&upper)).unwrap()
        );
    }

    #[test]
    fn missing_path_is_an_error() {
        let result = canonicalize(Path::new("/does/not/exist/hopefully"));
        assert!(result.is_err());
    }
}

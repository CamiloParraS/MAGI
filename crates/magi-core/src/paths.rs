//! Data/config/cache directory resolution, honoring `MAGI_*` env overrides,
//! and path canonicalization (see SPEC.md §4.5, §5.1).

use std::env;
use std::path::{Path, PathBuf};

use directories::{ProjectDirs, UserDirs};

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

/// The user's Documents, Desktop, Downloads and Pictures folders that
/// exist: what onboarding offers to add with one click. Only offered;
/// nothing is indexed unless the user picks it.
pub fn user_folders() -> Vec<PathBuf> {
    let Some(dirs) = UserDirs::new() else {
        return Vec::new();
    };
    [
        dirs.document_dir(),
        dirs.desktop_dir(),
        dirs.download_dir(),
        dirs.picture_dir(),
    ]
    .into_iter()
    .flatten()
    .filter(|dir| dir.is_dir())
    .map(Path::to_path_buf)
    .collect()
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

/// A native library (PDFium, ONNX Runtime) shipped inside an installed
/// package, where cargo-packager puts resources (ADR-0012): next to the
/// executable (NSIS), `Contents/Resources` (macOS `.app`),
/// `$APPDIR/usr/lib/magi` (AppImage) or `/usr/lib/magi` (`.deb`). `None` in a
/// dev build, which uses `vendor/`.
pub fn bundled_library(file: &str, exe: &Path, appdir: Option<&Path>) -> Option<PathBuf> {
    let exe_dir = exe.parent()?;
    [
        Some(exe_dir.to_path_buf()),
        Some(exe_dir.join("../Resources")),
        appdir.map(|dir| dir.join("usr/lib/magi")),
        Some(PathBuf::from("/usr/lib/magi")),
    ]
    .into_iter()
    .flatten()
    .map(|dir| dir.join(file))
    .find(|path| path.is_file())
}

/// [`bundled_library`] for the running executable.
pub fn installed_library(file: &str) -> Option<PathBuf> {
    let exe = env::current_exe().ok()?;
    let appdir = env::var_os("APPDIR").map(PathBuf::from);
    bundled_library(file, &exe, appdir.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bundled_library_is_found_where_each_package_puts_it() {
        let dir = tempfile::tempdir().unwrap();
        let put = |rel: &str| {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"").unwrap();
        };
        // NSIS: next to the executable.
        put("win/lib.dll");
        assert!(bundled_library("lib.dll", &dir.path().join("win/magi.exe"), None).is_some());
        // macOS .app: Contents/Resources, beside Contents/MacOS.
        put("Magi.app/Contents/Resources/lib.dylib");
        let mac = dir.path().join("Magi.app/Contents/MacOS/magi");
        assert!(bundled_library("lib.dylib", &mac, None).is_some());
        // AppImage: $APPDIR/usr/lib/magi.
        put("appdir/usr/lib/magi/lib.so");
        let appdir = dir.path().join("appdir");
        assert!(
            bundled_library(
                "lib.so",
                &appdir.join("usr/bin/magi"),
                Some(appdir.as_path())
            )
            .is_some()
        );
        // A dev build has none of these.
        assert_eq!(bundled_library("lib.so", &mac, None), None);
    }

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

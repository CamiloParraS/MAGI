//! Walks a root folder, applying exclusion globs, the hidden-file policy,
//! the symlink policy, and opaque-bundle handling (see SPEC.md §7 M2, §5.2
//! per-OS default exclusions).

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use crate::error::Result;
use crate::platform::{CloudPlaceholder, Os};

/// Package-bundle directory suffixes treated as a single opaque entry
/// (indexed by name only, never descended) per SPEC.md §5.2.
const OPAQUE_BUNDLE_SUFFIXES: &[&str] = &[".app", ".photoslibrary", ".bundle", ".framework"];

pub struct WalkOptions {
    pub exclude_globs: Vec<glob::Pattern>,
    pub include_hidden: bool,
    pub follow_symlinks: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkEntry {
    pub path: PathBuf,
    pub size: u64,
    pub mtime_ns: i64,
    pub is_dir: bool,
    /// A cloud placeholder (OneDrive, iCloud, Dropbox) whose content is not on
    /// disk: never read, or reading it would download it (SPEC.md §6).
    pub cloud_only: bool,
}

fn is_opaque_bundle(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|name| {
            let lower = name.to_ascii_lowercase();
            OPAQUE_BUNDLE_SUFFIXES.iter().any(|s| lower.ends_with(s))
        })
        .unwrap_or(false)
}

fn matches_exclude(path: &Path, root: &Path, patterns: &[glob::Pattern]) -> bool {
    let rel = path.strip_prefix(root).unwrap_or(path);
    let normalized = rel.to_string_lossy().replace('\\', "/");
    patterns.iter().any(|p| p.matches(&normalized))
}

fn mtime_ns(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

/// The entry a walk would yield for `path`, stat'd now.
pub fn stat(path: &Path) -> std::io::Result<WalkEntry> {
    let meta = std::fs::metadata(path)?;
    Ok(WalkEntry {
        path: path.to_path_buf(),
        size: meta.len(),
        mtime_ns: mtime_ns(&meta),
        is_dir: meta.is_dir(),
        cloud_only: Os.is_cloud_only(&meta),
    })
}

/// Walks `root`, returning one entry per indexable file plus one entry per
/// opaque bundle directory (never descended into). Excluded and hidden
/// paths are omitted entirely.
pub fn walk(root: &Path, options: &WalkOptions) -> Result<Vec<WalkEntry>> {
    walk_under(root, root, options)
}

/// Whether a walk of `root` would reach `path` (a file, or a directory it
/// would descend into): not hidden, not matched by an exclusion glob on it or
/// any folder above it, not inside an opaque bundle, and not a symlink that
/// is not followed. The watcher uses this to judge one path without a walk.
///
/// ponytail: does not know Windows' hidden *attribute* or symlinked ancestor
/// folders; the periodic reconciliation corrects any disagreement.
pub fn is_wanted(root: &Path, path: &Path, options: &WalkOptions) -> bool {
    let Ok(rel) = path.strip_prefix(root) else {
        return false;
    };
    let names: Vec<_> = rel.components().collect();
    let mut prefix = PathBuf::new();
    for (i, part) in names.iter().enumerate() {
        prefix.push(part);
        let hidden = part.as_os_str().to_string_lossy().starts_with('.');
        if (hidden && !options.include_hidden)
            || matches_exclude(&root.join(&prefix), root, &options.exclude_globs)
            || (i + 1 < names.len() && is_opaque_bundle(&root.join(&prefix)))
        {
            return false;
        }
    }
    options.follow_symlinks
        || std::fs::symlink_metadata(path).is_ok_and(|m| !m.file_type().is_symlink())
}

/// [`walk`] restricted to the folder `start` inside `root`. Exclusion globs are
/// still matched relative to `root`.
pub fn walk_under(root: &Path, start: &Path, options: &WalkOptions) -> Result<Vec<WalkEntry>> {
    // Paths under an already-yielded opaque bundle are suppressed as the
    // walk continues past them (ignore::WalkBuilder has no "yield but
    // don't descend" primitive, so we yield the bundle root once and then
    // filter out everything nested beneath it).
    let opaque_roots: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
    let exclude_globs = options.exclude_globs.clone();
    let root_owned = root.to_path_buf();

    let walker = ignore::WalkBuilder::new(start)
        .hidden(!options.include_hidden)
        .follow_links(options.follow_symlinks)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .ignore(false)
        .parents(false)
        .filter_entry(move |entry| {
            let path = entry.path();
            if opaque_roots
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .iter()
                .any(|r| path != r && path.starts_with(r))
            {
                return false;
            }
            if path != root_owned && matches_exclude(path, &root_owned, &exclude_globs) {
                return false;
            }
            if entry.file_type().is_some_and(|t| t.is_dir()) && is_opaque_bundle(path) {
                opaque_roots
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(path.to_path_buf());
            }
            true
        })
        .build();

    let mut entries = Vec::new();
    for result in walker {
        let entry = match result {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if path == start {
            continue;
        }
        let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
        let is_file = entry.file_type().is_some_and(|t| t.is_file());
        if !is_file && !(is_dir && is_opaque_bundle(path)) {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        entries.push(WalkEntry {
            path: path.to_path_buf(),
            size: meta.len(),
            mtime_ns: mtime_ns(&meta),
            is_dir,
            cloud_only: Os.is_cloud_only(&meta),
        });
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn default_options() -> WalkOptions {
        WalkOptions {
            exclude_globs: vec![
                glob::Pattern::new("**/node_modules/**").unwrap(),
                glob::Pattern::new("**/*.tmp").unwrap(),
            ],
            include_hidden: false,
            follow_symlinks: false,
        }
    }

    fn rel_paths(root: &Path, entries: &[WalkEntry]) -> Vec<String> {
        let mut v: Vec<String> = entries
            .iter()
            .map(|e| {
                e.path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        v.sort();
        v
    }

    #[test]
    fn yields_files_and_skips_excluded_and_hidden() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("keep.txt"), "hi").unwrap();
        fs::create_dir(root.join("node_modules")).unwrap();
        fs::write(root.join("node_modules/pkg.js"), "x").unwrap();
        fs::write(root.join("temp.tmp"), "x").unwrap();
        fs::write(root.join(".hidden"), "x").unwrap();
        fs::create_dir(root.join("sub")).unwrap();
        fs::write(root.join("sub/nested.txt"), "hi").unwrap();

        let entries = walk(root, &default_options()).unwrap();
        assert_eq!(
            rel_paths(root, &entries),
            vec!["keep.txt".to_string(), "sub/nested.txt".to_string()]
        );
    }

    #[test]
    fn include_hidden_reveals_dotfiles() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join(".hidden"), "x").unwrap();

        let mut options = default_options();
        options.include_hidden = true;
        let entries = walk(root, &options).unwrap();
        assert_eq!(rel_paths(root, &entries), vec![".hidden".to_string()]);
    }

    #[test]
    fn opaque_bundle_is_yielded_once_and_never_descended() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir(root.join("Photos.photoslibrary")).unwrap();
        fs::write(root.join("Photos.photoslibrary/masters.db"), "x").unwrap();
        fs::create_dir(root.join("Photos.photoslibrary/originals")).unwrap();
        fs::write(root.join("Photos.photoslibrary/originals/a.jpg"), "x").unwrap();

        let entries = walk(root, &default_options()).unwrap();
        assert_eq!(
            rel_paths(root, &entries),
            vec!["Photos.photoslibrary".to_string()]
        );
    }

    #[test]
    fn exclude_globs_match_relative_to_root_not_ancestor_path() {
        let dir = tempfile::tempdir().unwrap();
        // The root itself is named "target", which would false-positive
        // against a "**/target/**" exclude if matched against the full
        // absolute path instead of paths relative to this root.
        let root = dir.path().join("target");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("keep.txt"), "hi").unwrap();

        let mut options = default_options();
        options.exclude_globs = vec![glob::Pattern::new("**/target/**").unwrap()];
        let entries = walk(&root, &options).unwrap();

        assert_eq!(rel_paths(&root, &entries), vec!["keep.txt".to_string()]);
    }

    #[test]
    fn deeply_nested_directories_are_all_walked() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut deep = root.to_path_buf();
        for i in 0..20 {
            deep = deep.join(format!("level{i}"));
        }
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("bottom.txt"), "hi").unwrap();

        let entries = walk(root, &default_options()).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].path.ends_with("bottom.txt"));
    }

    #[test]
    fn is_wanted_agrees_with_what_a_walk_yields() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("keep.txt"), "hi").unwrap();
        fs::create_dir(root.join("node_modules")).unwrap();
        fs::write(root.join("node_modules/pkg.js"), "x").unwrap();
        fs::write(root.join("temp.tmp"), "x").unwrap();
        fs::write(root.join(".hidden"), "x").unwrap();
        fs::create_dir(root.join(".git")).unwrap();
        fs::write(root.join(".git/config"), "x").unwrap();
        fs::create_dir(root.join("Photos.photoslibrary")).unwrap();
        fs::write(root.join("Photos.photoslibrary/a.jpg"), "x").unwrap();
        let options = default_options();

        let walked = rel_paths(root, &walk(root, &options).unwrap());
        for rel in [
            "keep.txt",
            "node_modules/pkg.js",
            "temp.tmp",
            ".hidden",
            ".git/config",
            "Photos.photoslibrary/a.jpg",
        ] {
            assert_eq!(
                is_wanted(root, &root.join(rel), &options),
                walked.contains(&rel.to_string()),
                "{rel}"
            );
        }
        assert!(!is_wanted(
            root,
            &root.parent().unwrap().join("outside.txt"),
            &options
        ));
    }

    #[test]
    fn walk_under_walks_one_folder_but_matches_exclusions_from_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("a/node_modules")).unwrap();
        fs::write(root.join("a/keep.txt"), "hi").unwrap();
        fs::write(root.join("a/node_modules/pkg.js"), "x").unwrap();
        fs::write(root.join("other.txt"), "hi").unwrap();

        let entries = walk_under(root, &root.join("a"), &default_options()).unwrap();

        assert_eq!(rel_paths(root, &entries), vec!["a/keep.txt".to_string()]);
    }

    #[test]
    fn records_size_and_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("f.txt"), "hello").unwrap();

        let entries = walk(root, &default_options()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].size, 5);
        assert!(entries[0].mtime_ns > 0);
    }
}

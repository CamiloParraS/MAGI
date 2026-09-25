//! Deleting a file removes its thumbnail only when no other file shares it
//! (thumbnails are keyed by content hash, so identical files share one).

use std::path::PathBuf;

use magi_core::db::files::{FileRecord, delete_file, upsert_file};
use magi_core::{db, thumbs};

#[test]
fn a_shared_thumbnail_survives_until_its_last_file_is_deleted() {
    let data = tempfile::tempdir().unwrap();
    // SAFETY: the only test in this binary, so nothing races the environment.
    unsafe { std::env::set_var("MAGI_DATA_DIR", data.path()) };
    let mut conn = db::open(&data.path().join("magi.db")).unwrap();
    let root_dir = tempfile::tempdir().unwrap();
    let root = db::roots::add(&conn, root_dir.path()).unwrap();

    let key = "ab".repeat(32);
    let thumb = thumbs::thumb_path(&key);
    thumbs::write_thumbnail(&thumb, &image::RgbImage::new(8, 8)).unwrap();
    assert!(thumb.exists());

    let mut add = |name: &str| {
        let path: PathBuf = root.path.join(name);
        let rel = PathBuf::from(name);
        let record = FileRecord {
            root_id: root.id,
            path: &path,
            rel_path: &rel,
            file_name: name,
            ext: Some("jpg"),
            kind: "image",
            size: 1,
            mtime_ns: 1,
            lang: None,
            state: magi_core::db::files::FileState::Indexed,
            skip_reason: None,
            error: None,
            seen_scan_id: 1,
            content_hash: None,
            thumb_key: Some(&key),
        };
        upsert_file(&mut conn, &record, &[], &[], None).unwrap()
    };
    let (first, second) = (add("a.jpg"), add("b.jpg"));

    delete_file(&mut conn, first).unwrap();
    assert!(thumb.exists(), "b.jpg still uses the thumbnail");

    delete_file(&mut conn, second).unwrap();
    assert!(!thumb.exists(), "no file uses it any more");
}

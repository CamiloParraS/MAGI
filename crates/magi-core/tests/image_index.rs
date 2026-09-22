use std::path::Path;

use magi_core::config::IndexingConfig;
use magi_core::embed::{FakeEmbedder, FakeImageEmbedder};
use magi_core::index::pipeline::{IndexContext, IndexRootOptions, index_root};
use magi_core::{db, paths, thumbs};

/// SPEC.md §7 M4: a photographed QR code is found by both `qr code` and
/// `código QR`, and every image/PDF gets a content hash and a thumbnail; an
/// image also gets its visual vector.
#[test]
fn qr_image_is_searchable_in_both_languages_and_gets_hash_and_thumbnail() {
    let data_dir = tempfile::tempdir().unwrap();
    // SAFETY: this is the only test in this binary, so nothing else reads
    // or writes the environment concurrently.
    unsafe { std::env::set_var("MAGI_DATA_DIR", data_dir.path()) };

    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/corpus");
    let root_dir = tempfile::tempdir().unwrap();
    for rel in ["images/phone_qr.heic", "pdf/report.pdf"] {
        let name = Path::new(rel).file_name().unwrap();
        std::fs::copy(corpus.join(rel), root_dir.path().join(name)).unwrap();
    }
    let root_path = paths::canonicalize(root_dir.path()).unwrap();

    let mut conn = db::open(&data_dir.path().join("magi.db")).unwrap();
    let root = db::roots::add(&conn, &root_path).unwrap();
    let options = IndexRootOptions::from_config(&IndexingConfig::default()).unwrap();
    let summary = index_root(
        &mut conn,
        root.id,
        &root_path,
        &options,
        1,
        &IndexContext {
            image_embedder: Some(std::sync::Arc::new(FakeImageEmbedder)),
            ..IndexContext::new(std::sync::Arc::new(FakeEmbedder))
        },
    )
    .unwrap();
    assert_eq!((summary.indexed, summary.errored), (2, 0));

    // Only the image gets a visual vector; the PDF must not.
    let vectors: i64 = conn
        .query_row("SELECT COUNT(*) FROM vec_image", [], |r| r.get(0))
        .unwrap();
    assert_eq!(vectors, 1, "vec_image rows");
    assert_eq!(
        db::meta::get(&conn, "image_model_id").unwrap().as_deref(),
        Some("fake-image-v1")
    );

    for query in ["qr code", "código QR"] {
        let hits = magi_core::search::fts::search_fts(&conn, query, 10).unwrap();
        assert!(
            hits.iter().any(|h| h.path.ends_with("phone_qr.heic")),
            "`{query}` did not find the QR photo: {hits:?}"
        );
    }

    let mut stmt = conn
        .prepare("SELECT file_name, content_hash, thumb_key FROM files")
        .unwrap();
    let rows: Vec<(String, Vec<u8>, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(rows.len(), 2);
    for (name, hash, key) in rows {
        assert_eq!(hash.len(), 32, "{name}: blake3 hash");
        assert!(thumbs::thumb_path(&key).exists(), "{name}: thumbnail");
    }
}

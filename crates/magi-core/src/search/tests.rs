use super::*;
use crate::db;
use crate::db::files::{FileRecord, upsert_file};
use crate::embed::FakeEmbedder;
use crate::extract::RawChunk;
use std::path::{Path, PathBuf};

pub(crate) fn open_test_db() -> (tempfile::TempDir, Connection) {
    let dir = tempfile::tempdir().unwrap();
    let conn = db::open(&dir.path().join("magi.db")).unwrap();
    let root_dir = tempfile::tempdir().unwrap();
    db::roots::add(&conn, root_dir.path()).unwrap();
    (dir, conn)
}

pub(crate) fn index_text(conn: &mut Connection, path: &str, body: &str, mtime_ns: i64) {
    let path_buf = PathBuf::from(path);
    let rel = PathBuf::from(Path::new(path).file_name().unwrap());
    let record = FileRecord {
        root_id: 1,
        path: &path_buf,
        rel_path: &rel,
        file_name: rel.to_str().unwrap(),
        ext: Some("txt"),
        kind: "text",
        size: body.len() as u64,
        mtime_ns,
        lang: None,
        state: crate::db::files::FileState::Indexed,
        skip_reason: None,
        error: None,
        seen_scan_id: 1,
        content_hash: None,
        thumb_key: None,
        features_missing: 0,
    };
    let chunks = vec![RawChunk::body(body.to_string())];
    let embeddings = FakeEmbedder.embed_passages(&[body]).unwrap();
    upsert_file(conn, &record, &chunks, Some(&embeddings), None).unwrap();
}

#[test]
fn hybrid_search_without_a_text_embedder_uses_keywords_only() {
    // vec_text rows from when meaning was on stay unused, and nothing
    // tries to embed the query.
    let (_dir, mut conn) = open_test_db();
    index_text(&mut conn, "/roots/a/port.txt", "the harbor at dawn", 0);
    let hits = hybrid_search(&conn, None, None, "harbor", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert!(
        conn.query_row("SELECT COUNT(*) FROM vec_text", [], |r| r.get::<_, i64>(0))
            .unwrap()
            > 0
    );
}

#[test]
fn visual_match_is_found_through_vec_image_and_labelled() {
    use crate::embed::FakeImageEmbedder;
    let (_dir, mut conn) = open_test_db();
    // A photo with no text at all: only its planted vector can match.
    let path = PathBuf::from("/roots/a/IMG_0042.jpg");
    let rel = PathBuf::from("IMG_0042.jpg");
    let record = FileRecord {
        root_id: 1,
        path: &path,
        rel_path: &rel,
        file_name: "IMG_0042.jpg",
        ext: Some("jpg"),
        kind: "image",
        size: 1,
        mtime_ns: 0,
        lang: None,
        state: crate::db::files::FileState::Indexed,
        skip_reason: None,
        error: None,
        seen_scan_id: 1,
        content_hash: None,
        thumb_key: None,
        features_missing: 0,
    };
    let name_chunk = vec![RawChunk::body("IMG 0042 jpg".to_string())];
    let embeddings = FakeEmbedder.embed_passages(&["IMG 0042 jpg"]).unwrap();
    let visual = FakeImageEmbedder.embed_query("dog on the beach").unwrap();
    upsert_file(
        &mut conn,
        &record,
        &name_chunk,
        Some(&embeddings),
        Some(&visual),
    )
    .unwrap();
    index_text(&mut conn, "/roots/a/tax.txt", "quarterly tax filing", 0);

    let hits = hybrid_search(
        &conn,
        Some(&FakeEmbedder),
        Some(&FakeImageEmbedder),
        "dog on the beach",
        10,
    )
    .unwrap();
    let photo = hits.iter().find(|h| h.path == path).expect("photo found");
    assert!(photo.match_sources.contains(&MatchSource::Visual));
    // An unrelated query is below the cosine floor: the photo is not
    // dragged into every search.
    let unrelated = hybrid_search(
        &conn,
        Some(&FakeEmbedder),
        Some(&FakeImageEmbedder),
        "quarterly tax filing",
        10,
    )
    .unwrap();
    assert!(
        !unrelated
            .iter()
            .any(|h| h.match_sources.contains(&MatchSource::Visual)),
        "{unrelated:?}"
    );
    // Without the image embedder the same query cannot reach it.
    let text_only =
        hybrid_search(&conn, Some(&FakeEmbedder), None, "dog on the beach", 10).unwrap();
    assert!(
        !text_only
            .iter()
            .any(|h| h.match_sources.contains(&MatchSource::Visual))
    );
}

/// Both towers in one: the visual one says it started, and the text one
/// waits a moment to see whether that happens while it is still running.
struct Watch {
    image_started: (
        crossbeam_channel::Sender<()>,
        crossbeam_channel::Receiver<()>,
    ),
    overlapped: std::sync::atomic::AtomicBool,
}

impl TextEmbedder for Watch {
    fn model_id(&self) -> &str {
        "watch"
    }
    fn dim(&self) -> usize {
        crate::embed::TEXT_EMBEDDING_DIM
    }
    fn embed_passages(&self, _: &[&str]) -> Result<Vec<Vec<f32>>> {
        unreachable!("only queries are embedded")
    }
    fn embed_query(&self, _: &str) -> Result<Vec<f32>> {
        let wait = std::time::Duration::from_millis(200);
        if self.image_started.1.recv_timeout(wait).is_ok() {
            self.overlapped
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(vec![1.0; crate::embed::TEXT_EMBEDDING_DIM])
    }
}

impl ImageEmbedder for Watch {
    fn model_id(&self) -> &str {
        "watch"
    }
    fn dim(&self) -> usize {
        crate::embed::IMAGE_EMBEDDING_DIM
    }
    fn embed_image(&self, _: &image::RgbImage) -> Result<Vec<f32>> {
        unreachable!("only queries are embedded")
    }
    fn embed_query(&self, _: &str) -> Result<Vec<f32>> {
        let _ = self.image_started.0.try_send(());
        Ok(vec![1.0; crate::embed::IMAGE_EMBEDDING_DIM])
    }
}

/// NFR-12: a cold query loads each tower's session, and building
/// SigLIP's briefly takes ~800 MB. Loaded alongside e5, a cold search
/// peaked at 1,206 MB instead of 844 (docs/benchmarks.md).
#[test]
fn the_visual_tower_starts_only_after_the_text_one() {
    let towers = Watch {
        image_started: crossbeam_channel::bounded(1),
        overlapped: Default::default(),
    };
    let vectors = QueryVectors::embed(Some(&towers), Some(&towers), "harbor").unwrap();
    assert!(vectors.text.is_some() && vectors.image.is_some());
    assert!(!towers.overlapped.into_inner(), "the towers ran at once");
}

#[test]
fn a_broken_visual_tower_degrades_to_text_only() {
    struct Broken;
    impl ImageEmbedder for Broken {
        fn model_id(&self) -> &str {
            "broken"
        }
        fn dim(&self) -> usize {
            crate::embed::IMAGE_EMBEDDING_DIM
        }
        fn embed_image(&self, _: &image::RgbImage) -> Result<Vec<f32>> {
            unreachable!()
        }
        fn embed_query(&self, _: &str) -> Result<Vec<f32>> {
            Err(crate::error::Error::Engine("no session".into()))
        }
    }
    let vectors = QueryVectors::embed(Some(&FakeEmbedder), Some(&Broken), "harbor").unwrap();
    assert!(vectors.text.is_some() && vectors.image.is_none());
}

#[test]
fn a_blank_query_is_not_embedded() {
    use crate::embed::FakeImageEmbedder;
    let vectors = QueryVectors::embed(Some(&FakeEmbedder), Some(&FakeImageEmbedder), "   ");
    assert_eq!(vectors.unwrap(), QueryVectors::default());
}

#[test]
fn empty_query_or_zero_limit_returns_no_results() {
    let (_dir, mut conn) = open_test_db();
    index_text(&mut conn, "/roots/a/notes.txt", "hello world", 0);
    let embedder = FakeEmbedder;

    assert!(
        hybrid_search(&conn, Some(&embedder), None, "", 10)
            .unwrap()
            .is_empty()
    );
    assert!(
        hybrid_search(&conn, Some(&embedder), None, "hello", 0)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn keyword_only_match_is_found_via_fts() {
    let (_dir, mut conn) = open_test_db();
    index_text(
        &mut conn,
        "/roots/a/recipe.txt",
        "receta de arepas con queso",
        0,
    );
    let embedder = FakeEmbedder;

    let hits = hybrid_search(&conn, Some(&embedder), None, "arepas", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].path, PathBuf::from("/roots/a/recipe.txt"));
    assert!(hits[0].match_sources.contains(&MatchSource::Keyword));
}

#[test]
fn file_matching_both_keyword_and_vector_ranks_first() {
    let (_dir, mut conn) = open_test_db();
    // Shares the query's exact words (keyword + vector match).
    index_text(&mut conn, "/roots/a/cats.txt", "cat sat on the mat", 0);
    // Unrelated content (neither list should surface it).
    index_text(
        &mut conn,
        "/roots/a/market.txt",
        "stock market rallied today",
        0,
    );
    let embedder = FakeEmbedder;

    let hits = hybrid_search(&conn, Some(&embedder), None, "cat sat mat", 10).unwrap();
    assert_eq!(hits[0].path, PathBuf::from("/roots/a/cats.txt"));
    assert!(hits[0].match_sources.contains(&MatchSource::Keyword));
    assert!(hits[0].match_sources.contains(&MatchSource::Semantic));
}

#[test]
fn filename_overlap_boosts_ranking() {
    let (_dir, mut conn) = open_test_db();
    // Identical body text, so FTS and vector scores tie exactly and
    // both lists break the tie by row id: `other.txt`, inserted first,
    // leads both. Only filename_boost (×1.1 for "budget") can overcome
    // that (RRF 1/61 vs 1/62 per list, a ~1.6% gap).
    index_text(
        &mut conn,
        "/roots/a/other.txt",
        "the quarterly numbers were reviewed",
        0,
    );
    index_text(
        &mut conn,
        "/roots/a/budget_report.txt",
        "the quarterly numbers were reviewed",
        0,
    );
    let embedder = FakeEmbedder;

    let hits = hybrid_search(&conn, Some(&embedder), None, "budget quarterly", 10).unwrap();
    assert_eq!(hits[0].path, PathBuf::from("/roots/a/budget_report.txt"));
}

#[test]
fn the_best_chunks_source_and_page_reach_the_hit() {
    use MatchSource::*;
    let hit = |id: i64, source, page| FileHit {
        file_id: id,
        path: PathBuf::from(format!("/r/{id}")),
        file_name: format!("f{id}"),
        kind: Kind::Text,
        mtime_ns: 0,
        thumb_key: None,
        snippet: "s".into(),
        page,
        source: Some(source),
    };
    let hits = rank_and_boost(
        "zzz",
        &[
            hit(1, ChunkSource::Ocr, None),
            hit(2, ChunkSource::Body, Some(3)),
        ],
        &[hit(3, ChunkSource::Qr, None)],
        &[],
        10,
    );
    let by_id = |id| hits.iter().find(|h| h.file_id == id).unwrap();
    assert_eq!(by_id(1).match_sources, vec![Keyword, Ocr]);
    assert_eq!(
        (by_id(2).match_sources.clone(), by_id(2).page),
        (vec![Keyword], Some(3))
    );
    assert_eq!(by_id(3).match_sources, vec![Semantic, Qr]);
}

#[test]
fn a_name_is_tagged_only_when_the_keywords_matched_it() {
    let name = |id: i64| FileHit {
        file_id: id,
        path: PathBuf::from(format!("/r/{id}")),
        file_name: format!("f{id}"),
        kind: Kind::Text,
        mtime_ns: 0,
        thumb_key: None,
        snippet: "s".into(),
        page: None,
        source: Some(ChunkSource::Filename),
    };
    let hits = rank_and_boost("zzz", &[name(1)], &[name(2)], &[], 10);
    let by_id = |id| hits.iter().find(|h| h.file_id == id).unwrap();
    assert_eq!(
        by_id(1).match_sources,
        vec![MatchSource::Keyword, MatchSource::Filename]
    );
    assert_eq!(by_id(2).match_sources, vec![MatchSource::Semantic]);
}

/// Every ranked list joins the file's row, so the Host never goes back
/// to the database per result for what the UI shows.
#[test]
fn each_ranked_list_carries_the_files_kind_and_thumbnail() {
    use crate::embed::FakeImageEmbedder;
    let (_dir, mut conn) = open_test_db();
    let path = PathBuf::from("/roots/a/IMG_0042.jpg");
    let rel = PathBuf::from("IMG_0042.jpg");
    let record = FileRecord {
        root_id: 1,
        path: &path,
        rel_path: &rel,
        file_name: "IMG_0042.jpg",
        ext: Some("jpg"),
        kind: "image",
        size: 1,
        mtime_ns: 7,
        lang: None,
        state: crate::db::files::FileState::Indexed,
        skip_reason: None,
        error: None,
        seen_scan_id: 1,
        content_hash: None,
        thumb_key: Some("thumb42"),
        features_missing: 0,
    };
    let chunks = vec![RawChunk::body("harbor at dawn".to_string())];
    let text = FakeEmbedder.embed_passages(&["harbor at dawn"]).unwrap();
    let visual = FakeImageEmbedder.embed_query("harbor at dawn").unwrap();
    upsert_file(&mut conn, &record, &chunks, Some(&text), Some(&visual)).unwrap();

    let query = FakeEmbedder.embed_query("harbor at dawn").unwrap();
    let lists = [
        fts::search_fts(&conn, "harbor", 10).unwrap(),
        vector::search_vector_text(&conn, &query, 10).unwrap(),
        vector::search_vector_image(&conn, &visual, 10, IMAGE_MIN_COSINE).unwrap(),
    ];
    for hits in lists {
        let hit = &hits[0];
        assert_eq!(
            (hit.kind, hit.thumb_key.as_deref(), hit.mtime_ns),
            (Kind::Image, Some("thumb42"), 7),
            "{hit:?}"
        );
    }
}

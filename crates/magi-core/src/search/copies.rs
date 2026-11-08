//! Byte-identical files as one result: a real folder holds downloads and
//! backups of the same file, and each copy would push other answers down.

use std::collections::HashMap;

use rusqlite::{Connection, params_from_iter};

use crate::error::Result;
use crate::search::SearchHit;

/// Folds each hit whose `content_hash` an earlier hit has into that hit's
/// `copies`, then keeps `limit` hits. A file with no hash stays on its own.
pub(super) fn collapse(
    conn: &Connection,
    hits: Vec<SearchHit>,
    limit: u32,
) -> Result<Vec<SearchHit>> {
    let hashes = content_hashes(conn, &hits)?;
    let mut first_with: HashMap<&[u8], usize> = HashMap::new();
    let mut kept: Vec<SearchHit> = Vec::with_capacity(hits.len());
    for hit in hits {
        match hashes.get(&hit.file_id) {
            Some(hash) => match first_with.get(hash.as_slice()) {
                Some(&at) => kept[at].copies.push(hit.path),
                None => {
                    first_with.insert(hash, kept.len());
                    kept.push(hit);
                }
            },
            None => kept.push(hit),
        }
    }
    kept.truncate(limit as usize);
    Ok(kept)
}

fn content_hashes(conn: &Connection, hits: &[SearchHit]) -> Result<HashMap<i64, Vec<u8>>> {
    if hits.is_empty() {
        return Ok(HashMap::new());
    }
    let marks = vec!["?"; hits.len()].join(",");
    let mut stmt = conn.prepare(&format!(
        "SELECT id, content_hash FROM files
         WHERE content_hash IS NOT NULL AND id IN ({marks})"
    ))?;
    let hashes = stmt
        .query_map(params_from_iter(hits.iter().map(|h| h.file_id)), |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?
        .collect::<std::result::Result<_, _>>()?;
    Ok(hashes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::files::{FileRecord, FileState, upsert_file};
    use crate::discovery::Kind;
    use crate::search::tests::open_test_db;
    use std::path::{Path, PathBuf};

    fn index(conn: &mut Connection, name: &str, hash: Option<&[u8]>) -> i64 {
        let path = PathBuf::from(format!("/roots/a/{name}"));
        let record = FileRecord {
            root_id: 1,
            path: &path,
            rel_path: Path::new(name),
            file_name: name,
            ext: Some("txt"),
            kind: "text",
            size: 1,
            mtime_ns: 0,
            lang: None,
            state: FileState::Indexed,
            skip_reason: None,
            error: None,
            seen_scan_id: 1,
            content_hash: hash,
            thumb_key: None,
            features_missing: 0,
        };
        upsert_file(conn, &record, &[], None, None).unwrap();
        conn.query_row("SELECT id FROM files WHERE file_name = ?1", [name], |r| {
            r.get(0)
        })
        .unwrap()
    }

    fn hit(file_id: i64, name: &str) -> SearchHit {
        SearchHit {
            file_id,
            path: PathBuf::from(format!("/roots/a/{name}")),
            file_name: name.into(),
            kind: Kind::Text,
            mtime_ns: 0,
            thumb_key: None,
            score: 1.0,
            snippet: String::new(),
            match_sources: vec![],
            page: None,
            text_similarity: None,
            image_similarity: None,
            copies: Vec::new(),
        }
    }

    #[test]
    fn the_best_ranked_copy_stands_for_the_others() {
        let (_dir, mut conn) = open_test_db();
        let a = index(&mut conn, "a.txt", Some(&[1; 32]));
        let b = index(&mut conn, "b.txt", Some(&[2; 32]));
        let c = index(&mut conn, "c.txt", Some(&[1; 32]));
        let hits = vec![hit(c, "c.txt"), hit(b, "b.txt"), hit(a, "a.txt")];
        let kept = collapse(&conn, hits, 10).unwrap();
        let names: Vec<&str> = kept.iter().map(|h| h.file_name.as_str()).collect();
        assert_eq!(names, ["c.txt", "b.txt"]);
        assert_eq!(kept[0].copies, [PathBuf::from("/roots/a/a.txt")]);
        assert!(kept[1].copies.is_empty());
    }

    #[test]
    fn files_without_a_hash_are_never_copies_and_the_limit_holds() {
        let (_dir, mut conn) = open_test_db();
        let a = index(&mut conn, "a.txt", None);
        let b = index(&mut conn, "b.txt", None);
        let kept = collapse(&conn, vec![hit(a, "a.txt"), hit(b, "b.txt")], 1).unwrap();
        assert_eq!(kept.len(), 1);
        assert!(kept[0].copies.is_empty());
    }
}

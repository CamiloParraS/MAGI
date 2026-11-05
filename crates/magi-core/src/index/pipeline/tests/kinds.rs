//! Each file kind through the whole pipeline, on real fixtures.

use super::*;

#[test]
fn pdf_files_are_extracted_per_page_and_searchable() {
    let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
    let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/corpus/pdf/report.pdf");
    fs::copy(&fixture, root_path.join("report.pdf")).unwrap();

    let summary = index_fake(&mut conn, root_id, &root_path);

    let row = db::files::get_by_path(&conn, &root_path.join("report.pdf"))
        .unwrap()
        .unwrap();
    assert_eq!(summary.indexed, 1);
    assert_eq!(row.state, "indexed");
    assert_eq!(row.kind, "pdf");
    assert_eq!(row.lang.as_deref(), Some("en"));

    let hits = crate::search::fts::search_fts(&conn, "Expenses", 10).unwrap();
    assert_eq!(hits.len(), 1);
}

#[test]
fn password_protected_pdf_is_indexed_by_filename_only_via_error_path() {
    let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
    let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/corpus/edge/password_protected.pdf");
    fs::copy(&fixture, root_path.join("password_protected.pdf")).unwrap();

    let summary = index_fake(&mut conn, root_id, &root_path);

    assert_eq!(summary.errored, 1);
    let row = db::files::get_by_path(&conn, &root_path.join("password_protected.pdf"))
        .unwrap()
        .unwrap();
    assert_eq!(row.state, "error");
    // Still findable by name even though content extraction failed.
    let hits = crate::search::fts::search_fts(&conn, "protected", 10).unwrap();
    assert_eq!(hits.len(), 1);
}

#[test]
fn office_files_are_extracted_and_searchable() {
    let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
    let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
    let office_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/corpus/office");
    fs::copy(office_dir.join("notes.docx"), root_path.join("notes.docx")).unwrap();
    fs::copy(
        office_dir.join("kickoff.pptx"),
        root_path.join("kickoff.pptx"),
    )
    .unwrap();
    fs::copy(
        office_dir.join("inventory.xlsx"),
        root_path.join("inventory.xlsx"),
    )
    .unwrap();

    let summary = index_fake(&mut conn, root_id, &root_path);

    assert_eq!(summary.indexed, 3);
    for name in ["notes.docx", "kickoff.pptx", "inventory.xlsx"] {
        let row = db::files::get_by_path(&conn, &root_path.join(name))
            .unwrap()
            .unwrap();
        assert_eq!(row.state, "indexed");
        assert_eq!(row.kind, "office");
    }

    // Not "warehouse" alone: it appears in both notes.docx ("the new
    // warehouse") and inventory.xlsx ("Warehouse A"/"Warehouse B").
    assert_eq!(
        crate::search::fts::search_fts(&conn, "migration", 10)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        crate::search::fts::search_fts(&conn, "presupuesto", 10)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        crate::search::fts::search_fts(&conn, "widget", 10)
            .unwrap()
            .len(),
        1
    );
}

/// Real files from an actual nested user document tree, not synthetic
/// writes: SPEC.md §7 M2's edge-case list (`fixtures/corpus/edge/`).
#[test]
fn real_empty_file_is_indexed_with_only_a_filename_chunk() {
    let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
    let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/corpus/edge/empty_real.gitignore");
    assert_eq!(fs::metadata(&fixture).unwrap().len(), 0);
    fs::copy(&fixture, root_path.join("empty_real.gitignore")).unwrap();

    let summary = index_fake(&mut conn, root_id, &root_path);

    assert_eq!(summary.indexed, 1);
    let row = db::files::get_by_path(&conn, &root_path.join("empty_real.gitignore"))
        .unwrap()
        .unwrap();
    assert_eq!(row.state, "indexed");
    assert_eq!(
        db::files::count_chunks(&conn).unwrap(),
        1,
        "the filename chunk alone"
    );
}

#[test]
fn real_file_over_cap_is_skipped_with_reason() {
    let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
    let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/corpus/edge/huge_real.pdf");
    assert!(fs::metadata(&fixture).unwrap().len() > 1024 * 1024);
    fs::copy(&fixture, root_path.join("huge_real.pdf")).unwrap();

    // default_options() caps at 1 MB; huge_real.pdf is ~1.4 MB.
    let summary = index_fake(&mut conn, root_id, &root_path);

    assert_eq!(summary.skipped, 1);
    let row = db::files::get_by_path(&conn, &root_path.join("huge_real.pdf"))
        .unwrap()
        .unwrap();
    assert_eq!(row.state, "skipped");
    assert_eq!(row.skip_reason.as_deref(), Some("too_large"));
}

#[test]
fn real_deeply_nested_file_is_indexed_and_searchable() {
    let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
    let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
    let fixture_root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/corpus/edge/deep_real");
    let rel = Path::new(
        "Tercer Semestre/Fisica/asuntosSinImportancia/solarcalculator/solarcalculator/src/main/java/com/example/solarcalculator/model/calculateRequest.java",
    );
    // Rebuild component-by-component rather than a single `join(rel)`:
    // `rel`'s forward slashes survive verbatim inside a joined PathBuf
    // on Windows, producing mixed separators that don't string-match
    // the walker's own (all-backslash) path for the same file.
    let dest = rel
        .components()
        .fold(root_path.clone(), |acc, c| acc.join(c));
    fs::create_dir_all(dest.parent().unwrap()).unwrap();
    fs::copy(fixture_root.join(rel), &dest).unwrap();

    let summary = index_fake(&mut conn, root_id, &root_path);

    assert_eq!(summary.indexed, 1);
    let row = db::files::get_by_path(&conn, &dest).unwrap().unwrap();
    assert_eq!(row.state, "indexed");
    assert_eq!(row.kind, "code");
    assert_eq!(
        crate::search::fts::search_fts(&conn, "irradiacionSolar", 10)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn code_files_are_chunked_by_symbol_and_searchable() {
    let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
    let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
    let code_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/corpus/code");
    fs::copy(code_dir.join("sample.rs"), root_path.join("sample.rs")).unwrap();
    fs::copy(code_dir.join("muestra.py"), root_path.join("muestra.py")).unwrap();

    let summary = index_fake(&mut conn, root_id, &root_path);

    assert_eq!(summary.indexed, 2);
    for name in ["sample.rs", "muestra.py"] {
        let row = db::files::get_by_path(&conn, &root_path.join(name))
            .unwrap()
            .unwrap();
        assert_eq!(row.state, "indexed");
        assert_eq!(row.kind, "code");
    }

    assert_eq!(
        crate::search::fts::search_fts(&conn, "origin", 10)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        crate::search::fts::search_fts(&conn, "calculadora", 10)
            .unwrap()
            .len(),
        1
    );
}

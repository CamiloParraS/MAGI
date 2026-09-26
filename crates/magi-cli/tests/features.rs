//! `magi-cli features`, run as a process against throwaway directories.

use std::process::Command;

fn cli(data: &std::path::Path, config: &std::path::Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_magi-cli"))
        .args(args)
        .env("MAGI_DATA_DIR", data)
        .env("MAGI_CONFIG_DIR", config)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn disabling_and_enabling_a_feature_is_listed_and_persisted() {
    let data = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    let list = cli(data.path(), config.path(), &["features", "list"]);
    assert!(
        list.lines()
            .any(|l| l.starts_with("image_text\ton\tnot installed")),
        "{list}"
    );
    assert!(
        list.lines()
            .any(|l| l.starts_with("image_visual\toff\tnot installed")),
        "{list}"
    );

    cli(
        data.path(),
        config.path(),
        &["features", "disable", "image_text"],
    );
    let list = cli(data.path(), config.path(), &["features", "list"]);
    assert!(
        list.lines().any(|l| l.starts_with("image_text\toff")),
        "{list}"
    );
    let saved = std::fs::read_to_string(config.path().join("config.toml")).unwrap();
    assert!(saved.contains("image_text = false"), "{saved}");
}

#[test]
fn an_unknown_feature_is_an_error_naming_the_valid_ones() {
    let data = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_magi-cli"))
        .args(["features", "disable", "ocr"])
        .env("MAGI_DATA_DIR", data.path())
        .env("MAGI_CONFIG_DIR", config.path())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("image_text"));
}

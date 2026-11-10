use super::*;

#[test]
fn copying_a_result_copies_the_file_unless_turned_off() {
    assert!(UiConfig::default().copy_as_file);
    let old: UiConfig = toml::from_str("launch_at_login = true").unwrap();
    assert!(old.copy_as_file);
}

#[test]
fn ask_magi_is_shown_unless_turned_off() {
    assert!(UiConfig::default().ask_magi);
    let old: UiConfig = toml::from_str("launch_at_login = true").unwrap();
    assert!(old.ask_magi);
}

#[test]
fn the_detail_pane_is_on_for_new_installs_and_saved_configs_keep_theirs() {
    assert!(UiConfig::default().detail_pane);
    let off: UiConfig = toml::from_str("detail_pane = false").unwrap();
    assert!(!off.detail_pane);
    // A config saved before the setting existed meant the plain list.
    let old: UiConfig = toml::from_str("launch_at_login = true").unwrap();
    assert!(!old.detail_pane);
}

#[test]
fn round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let root = tempfile::tempdir().unwrap();

    let mut config = Config::default();
    config.roots.push(RootConfig {
        path: root.path().to_path_buf(),
        enabled: true,
    });

    save_to(&path, &config).unwrap();
    let loaded = load_from(&path).unwrap();
    assert_eq!(config, loaded);
}

#[test]
fn missing_file_creates_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");

    let loaded = load_from(&path).unwrap();
    assert_eq!(loaded, Config::default());
    assert!(path.exists());
}

#[test]
fn unknown_file_type_is_rejected_at_load() {
    // A typo would otherwise silently stop indexing that kind's content.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        "[indexing]
file_types = [\"text\", \"pdfs\"]
",
    )
    .unwrap();
    assert!(matches!(load_from(&path), Err(Error::ConfigParse(_))));
}

#[test]
fn bad_glob_is_rejected() {
    let mut config = Config::default();
    config.indexing.exclude_globs.push("[unterminated".into());
    assert!(matches!(config.validate(), Err(Error::InvalidGlob { .. })));
}

#[test]
fn nonexistent_root_is_rejected() {
    let mut config = Config::default();
    config.roots.push(RootConfig {
        path: PathBuf::from("/does/not/exist/hopefully"),
        enabled: true,
    });
    assert!(matches!(config.validate(), Err(Error::RootNotFound(_))));
}

#[test]
fn crash_between_write_and_rename_leaves_previous_config_intact() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");

    let mut config = Config::default();
    config.ui.max_results = 42;
    save_to(&path, &config).unwrap();

    // Simulate a crash: the temp file is written but the rename that
    // would replace config.toml never happens.
    let tmp_path = dir.path().join(".config.toml.tmp");
    fs::write(&tmp_path, "schema_version = 999").unwrap();

    let loaded = load_from(&path).unwrap();
    assert_eq!(loaded.ui.max_results, 42);
}
#[test]
fn feature_and_ui_defaults_match_adr_0010() {
    let c = Config::default();
    assert!(c.features.meaning && c.features.image_text && !c.features.image_visual);
    assert_eq!(c.ui.language, Language::System);
    assert_eq!(c.ui.transparency_mode, TransparencyMode::MatchSystem);
    assert_eq!(c.ui.transparency_intensity, 0.75);
    assert_eq!(
        c.ui.onboarding,
        Onboarding::Folders,
        "a first run starts onboarding"
    );
}

#[test]
fn features_and_ui_fields_round_trip_with_their_wire_names() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        "[features]
image_visual = true
image_text = false
             [ui]
language = \"es\"
transparency_mode = \"never\"
transparency_intensity = 0.4
onboarding = \"background\"
",
    )
    .unwrap();
    let c = load_from(&path).unwrap();
    assert!(c.features.meaning, "missing keys keep their default");
    assert!(!c.features.image_text && c.features.image_visual);
    assert_eq!(c.ui.language, Language::Es);
    assert_eq!(c.ui.transparency_mode, TransparencyMode::Never);
    assert_eq!(c.ui.onboarding, Onboarding::Background);
    save_to(&path, &c).unwrap();
    assert_eq!(load_from(&path).unwrap(), c);
}

#[test]
fn transparency_intensity_outside_its_range_is_rejected() {
    for bad in [0.39_f32, 0.96, f32::NAN] {
        let mut c = Config::default();
        c.ui.transparency_intensity = bad;
        assert!(
            matches!(
                c.validate(),
                Err(Error::InvalidSetting {
                    field: "ui.transparency_intensity",
                    ..
                })
            ),
            "{bad} accepted"
        );
    }
}

#[test]
fn an_unknown_language_is_rejected_at_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        "[ui]
language = \"fr\"
",
    )
    .unwrap();
    assert!(matches!(load_from(&path), Err(Error::ConfigParse(_))));
}

#[test]
fn features_config_reads_and_writes_each_feature() {
    let mut f = FeaturesConfig::default();
    f.set(Feature::ImageVisual, true);
    f.set(Feature::Meaning, false);
    assert!(f.enabled(Feature::ImageVisual) && !f.enabled(Feature::Meaning));
}

#[test]
fn a_settings_patch_merges_into_the_current_config() {
    let next = apply_patch(
        &Config::default(),
        &serde_json::json!({"ui": {"language": "es"}, "indexing": {"max_file_size_mb": 10}}),
    )
    .unwrap();
    assert_eq!(next.ui.language, Language::Es);
    assert_eq!(next.indexing.max_file_size_mb, 10);
    assert_eq!(
        next.ui.hotkey,
        UiConfig::default().hotkey,
        "untouched fields kept"
    );
}

#[test]
fn a_settings_patch_rejects_typos_bad_values_and_protected_sections() {
    use serde_json::json;
    for patch in [
        json!({"ui": {"langauge": "es"}}),
        json!({"ui": {"transparency_intensity": 2.0}}),
        json!({"ui": {"language": "fr"}}),
        json!({"indexing": {"exclude_globs": ["[unclosed"]}}),
        json!({"features": {"meaning": false}}),
        json!({"roots": []}),
        json!(["ui"]),
    ] {
        assert!(
            matches!(
                apply_patch(&Config::default(), &patch),
                Err(Error::InvalidSetting { .. } | Error::InvalidGlob { .. })
            ),
            "{patch}"
        );
    }
}

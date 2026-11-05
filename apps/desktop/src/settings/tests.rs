use super::*;
use crate::i18n::Lang;
use magi_core::dto::IndexState;

#[test]
fn ten_quick_taps_on_the_version_open_the_onboarding() {
    let t0 = Instant::now();
    let mut taps = (0, None);
    let step = Duration::from_millis(200);
    for i in 0..VERSION_TAPS - 1 {
        assert!(!version_tap(&mut taps, t0 + step * i as u32));
    }
    assert!(version_tap(
        &mut taps,
        t0 + step * (VERSION_TAPS as u32 - 1)
    ));
    // The run starts over.
    assert!(!version_tap(&mut taps, t0 + step * VERSION_TAPS as u32));
}

#[test]
fn a_pause_between_taps_starts_the_count_again() {
    let t0 = Instant::now();
    let mut taps = (0, None);
    for i in 0..VERSION_TAPS - 1 {
        assert!(!version_tap(
            &mut taps,
            t0 + Duration::from_millis(100 * i as u64)
        ));
    }
    let later = t0 + VERSION_TAP_GAP * 3;
    assert!(!version_tap(&mut taps, later));
    assert_eq!(taps.0, 1);
}

#[test]
fn a_count_rolls_from_where_it_shows_to_the_new_value() {
    let t0 = Instant::now();
    let at = |ms: u64| t0 + Duration::from_millis(ms);
    let roll = Roll::steady(10).retarget(110, t0);
    assert_eq!(roll.shown(t0), 10);
    assert!(roll.shown(at(60)) > 10 && roll.shown(at(60)) < 110);
    assert_eq!(roll.shown(at(ROLL.as_millis() as u64)), 110);
    assert_eq!(roll.shown(at(10_000)), 110, "stays there");

    // Interrupted halfway, it carries on from the number on screen, not
    // from the old target.
    let mid = roll.shown(at(60));
    let next = roll.retarget(50, at(60));
    assert_eq!(next.shown(at(60)), mid);
    assert_eq!(next.shown(at(10_000)), 50);

    // The same target again changes nothing, so a status that repeats
    // doesn't restart it.
    assert_eq!(roll.retarget(110, at(60)), roll);
}

fn root(enabled: bool, status: Health) -> RootStatus {
    RootStatus {
        id: 1,
        path: "D:\\Photos".into(),
        enabled,
        status,
        indexed: 0,
    }
}

#[test]
fn an_empty_unreadable_list_says_reading_until_the_queue_is_done() {
    let status = |queued, errors| IndexStatus {
        state: IndexState::Indexing,
        paused_for: None,
        scanned: 0,
        queued,
        indexed: 10,
        skipped: 0,
        errors,
        current_file: None,
        roots: Vec::new(),
    };
    // Retried files are queued again: not read yet.
    assert!(still_reading(Some(&status(3, 0))));
    // The status from before the retry still counts them as errors.
    assert!(still_reading(Some(&status(0, 3))));
    assert!(!still_reading(Some(&status(0, 0))));
    assert!(!still_reading(None));
}

#[test]
fn a_healthy_root_is_quiet_and_anything_else_says_why() {
    let s = Lang::En.strings();
    let state = |enabled, status| root_state(&root(enabled, status), s);
    // Nothing to say when it works: no badge, no explanation.
    assert_eq!(state(true, Health::Ok), (None, Tone::Ok, None));
    assert_eq!(
        state(true, Health::WatchFailed),
        (
            Some("Checking"),
            Tone::Neutral,
            Some("Can't watch for changes here; checking now and then instead")
        )
    );
    assert_eq!(
        state(true, Health::Missing),
        (
            Some("Not found"),
            Tone::Warn,
            Some("Folder not found. Reconnect the drive; the index is kept.")
        )
    );
    assert_eq!(
        state(true, Health::PermissionDenied),
        (
            Some("No access"),
            Tone::Err,
            Some("Magi isn't allowed to read this folder.")
        )
    );
    // A switched-off root is not probed, so its last status is stale.
    assert_eq!(
        state(false, Health::Missing),
        (Some("Paused"), Tone::Neutral, Some("Paused, not searched"))
    );
}

fn feature(enabled: bool, install: Install) -> FeatureStatus {
    FeatureStatus {
        feature: Feature::Meaning,
        enabled,
        download_size: 1,
        install,
        backfill: None,
    }
}

#[test]
fn a_feature_offers_the_one_action_that_fits() {
    use FeatureAction::*;
    let installed = Install::Installed { size_bytes: 1 };
    assert_eq!(
        feature_action(&feature(false, Install::NotInstalled)),
        TurnOn
    );
    assert_eq!(
        feature_action(&feature(true, Install::NotInstalled)),
        TurnOn,
        "wanted but never downloaded (a cancelled download)"
    );
    assert_eq!(feature_action(&feature(false, installed.clone())), Remove);
    assert_eq!(feature_action(&feature(true, installed)), Nothing);
    assert_eq!(
        feature_action(&feature(true, Install::Downloading { bytes: 0, total: 1 })),
        Nothing
    );
    assert_eq!(
        feature_action(&feature(
            true,
            Install::Failed {
                code: DownloadError::ChecksumMismatch
            }
        )),
        Retry
    );
}

#[test]
fn the_hotkey_setting_becomes_a_gpui_keystroke() {
    assert_eq!(
        hotkey_keystroke("CmdOrCtrl+Shift+Space"),
        "secondary-shift-space"
    );
    assert_eq!(hotkey_keystroke("Alt+K"), "alt-k");
    assert_eq!(
        hotkey_keystroke("Control+Option+KeyK"),
        "ctrl-alt-k",
        "global-hotkey's other spellings"
    );
    assert_eq!(hotkey_keystroke("CommandOrControl+Digit1"), "secondary-1");
    assert_eq!(hotkey_keystroke("Command+Space"), "cmd-space");
}

#[test]
fn result_counts_keep_the_saved_one() {
    assert_eq!(result_counts(30), vec![10, 20, 30, 50, 100]);
    assert_eq!(result_counts(42), vec![10, 20, 30, 42, 50, 100]);
}

#[test]
fn every_read_error_has_its_own_words() {
    let s = Lang::En.strings();
    let mut labels: Vec<_> = FileErrorCode::ALL
        .into_iter()
        .map(|code| file_error_label(code, s))
        .collect();
    labels.sort();
    labels.dedup();
    assert_eq!(labels.len(), FileErrorCode::ALL.len());
}

#[test]
fn folder_errors_name_the_paths_involved() {
    let s = Lang::En.strings();
    let path = || "C:\\Users\\ana\\Documents\\notes".to_string();
    assert_eq!(
        error_text(
            &ErrorCode::NestedRoot {
                path: path(),
                conflicts_with: "C:\\Users\\ana\\Documents".into()
            },
            s
        ),
        "C:\\Users\\ana\\Documents\\notes is already searched as part of C:\\Users\\ana\\Documents."
    );
    assert_eq!(
        error_text(&ErrorCode::RootAlreadyExists { path: path() }, s),
        "C:\\Users\\ana\\Documents\\notes is already in the list."
    );
    assert_eq!(
        error_text(&ErrorCode::RootNotFound { path: path() }, s),
        "C:\\Users\\ana\\Documents\\notes doesn't exist."
    );
    assert_eq!(
        error_text(&ErrorCode::EngineStarting, s),
        "Magi is still starting. Try again in a moment."
    );
    assert_eq!(
        error_text(
            &ErrorCode::Internal {
                detail: "disk I/O error".into()
            },
            s
        ),
        "Couldn't save the change.",
        "internal details are for the log only"
    );
}

#[test]
fn index_setting_errors_say_what_to_fix() {
    let s = Lang::En.strings();
    assert_eq!(
        error_text(
            &ErrorCode::InvalidGlob {
                glob: "**/[bad".into()
            },
            s
        ),
        "“**/[bad” isn't a valid pattern."
    );
    assert_eq!(
        error_text(
            &ErrorCode::InvalidSetting {
                field: MAX_SIZE_FIELD.into()
            },
            s
        ),
        "Enter a whole number of megabytes, 1 or more."
    );
}

#[test]
fn toggling_a_kind_keeps_the_display_order() {
    use Kind::*;
    assert_eq!(
        toggle_kind(&[Image, Text], Pdf, true),
        vec![Text, Pdf, Image]
    );
    assert_eq!(toggle_kind(&[Text, Pdf], Pdf, false), vec![Text]);
    assert_eq!(
        toggle_kind(&[Text], Text, true),
        vec![Text],
        "no duplicates"
    );
    assert_eq!(toggle_kind(&[Text], Code, false), vec![Text]);
    assert_eq!(
        toggle_kind(&[Text, Other], Code, true),
        vec![Text, Code],
        "kinds the window doesn't show are dropped"
    );
}

#[test]
fn the_max_size_is_whole_megabytes() {
    assert_eq!(max_size_from_text(" 100 "), Some(100));
    assert_eq!(max_size_from_text("1"), Some(1));
    for bad in ["0", "", "-5", "1.5", "ten", "99999999999999999999999"] {
        assert_eq!(max_size_from_text(bad), None, "{bad:?}");
    }
}

#[test]
fn globs_are_one_per_line() {
    assert_eq!(
        globs_from_text("**/node_modules/**\r\n\n  **/*.tmp  \n\n"),
        vec!["**/node_modules/**", "**/*.tmp"]
    );
    assert!(globs_from_text("  \n").is_empty());
}

#[test]
fn the_system_language_names_what_it_resolves_to() {
    assert_eq!(
        system_language(Lang::En.strings(), Lang::En),
        "Same as system (English)"
    );
    assert_eq!(
        system_language(Lang::Es.strings(), Lang::Es),
        "Igual que el sistema (Español)"
    );
}

#[test]
fn the_amount_is_adjustable_only_when_see_through() {
    use TransparencyMode::*;
    let blur = BackdropSupport {
        blur: true,
        reduce_transparency: false,
    };
    assert!(see_through_adjustable(Always, blur));
    assert!(see_through_adjustable(MatchSystem, blur));
    assert!(!see_through_adjustable(Never, blur));
    let reduced = BackdropSupport {
        reduce_transparency: true,
        ..blur
    };
    assert!(!see_through_adjustable(MatchSystem, reduced));
    assert!(!see_through_adjustable(Always, BackdropSupport::default()));
}

#[test]
fn the_slider_maps_to_an_intensity_the_config_accepts() {
    // Ends of the slider are the ends of the allowed range, reversed.
    assert_eq!(intensity_from_slider(0.0), 0.95);
    assert_eq!(intensity_from_slider(1.0), 0.40);
    assert_eq!(intensity_from_slider(0.2), 0.84);
    // Out of range or float noise still lands inside the range.
    assert_eq!(intensity_from_slider(1.3), 0.40);
    assert_eq!(intensity_from_slider(-0.2), 0.95);
    for step in 0..=20 {
        let i = intensity_from_slider(step as f32 * 0.05);
        assert!(TRANSPARENCY_INTENSITY.contains(&i), "{i}");
        assert!(
            (intensity_from_slider(slider_from_intensity(i)) - i).abs() < 1e-6,
            "round trip {i}"
        );
    }
    assert_eq!(slider_from_intensity(0.95), 0.0);
    assert!((slider_from_intensity(0.40) - 1.0).abs() < 1e-6);
}

#[test]
fn a_root_line_joins_its_count_and_its_explanation() {
    let (lang, s) = (Lang::En, Lang::En.strings());
    let line = |indexed, enabled, status| {
        root_line(
            &RootStatus {
                indexed,
                ..root(enabled, status)
            },
            &[],
            s,
            lang,
        )
    };
    assert_eq!(
        line(8214, true, Health::Ok).as_deref(),
        Some("8,214 files indexed")
    );
    assert_eq!(
        line(1, false, Health::Ok).as_deref(),
        Some("1 file indexed · Paused, not searched")
    );
    // Nothing indexed yet: no "0 files" while the first scan runs.
    assert_eq!(line(0, true, Health::Ok), None);
    assert_eq!(
        line(0, false, Health::Ok).as_deref(),
        Some("Paused, not searched")
    );
}

#[test]
fn a_reported_root_says_what_to_do_about_it() {
    let (lang, s) = (Lang::En, Lang::En.strings());
    let issue = |guidance| PermissionIssue {
        root_id: 1,
        path: "D:\\Photos".into(),
        status: Health::PermissionDenied,
        guidance,
    };
    let explain = |guidance| {
        root_explain(
            &root(true, Health::PermissionDenied),
            &[issue(guidance)],
            s,
            lang,
        )
    };
    assert_eq!(
        explain(Guidance::MacFilesAndFolders).as_deref(),
        Some(
            "macOS hasn't allowed Magi to read this folder. Allow Magi in \
             Privacy & Security › Files and Folders."
        )
    );
    assert_eq!(
        explain(Guidance::MacFullDiskAccess).as_deref(),
        Some(
            "macOS hasn't allowed Magi to read this folder. Allow Magi in \
             Privacy & Security › Full Disk Access, then restart Magi."
        )
    );
    assert_eq!(
        explain(Guidance::WatchLimit { limit: Some(8192) }).as_deref(),
        Some(
            "Linux can't watch more folders (limit 8,192), so this one is \
             checked now and then. Raise fs.inotify.max_user_watches to \
             watch it live."
        )
    );
    // Before the first report arrives, the status alone still explains.
    assert_eq!(
        root_explain(&root(true, Health::PermissionDenied), &[], s, lang).as_deref(),
        Some("Magi isn't allowed to read this folder.")
    );
    // Another root's issue is not this one's.
    let other = PermissionIssue {
        root_id: 2,
        ..issue(Guidance::MacFullDiskAccess)
    };
    assert_eq!(
        root_explain(&root(true, Health::Ok), &[other], s, lang),
        None
    );
}

#[test]
fn image_features_need_the_images_type() {
    let on = |f| FeatureStatus {
        feature: f,
        ..feature(true, Install::NotInstalled)
    };
    let all = [Kind::Text, Kind::Image];
    let no_images = [Kind::Text];
    assert!(!images_needed(&all, &[on(Feature::ImageText)]));
    assert!(images_needed(&no_images, &[on(Feature::ImageText)]));
    assert!(images_needed(&no_images, &[on(Feature::ImageVisual)]));
    // Search by meaning reads text, not images.
    assert!(!images_needed(&no_images, &[on(Feature::Meaning)]));
    let off = FeatureStatus {
        enabled: false,
        ..on(Feature::ImageText)
    };
    assert!(!images_needed(&no_images, &[off]));
}

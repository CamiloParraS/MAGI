use super::*;
use crate::db::roots::Health;

#[test]
fn root_health_crosses_ipc_as_its_database_name() {
    for health in [
        Health::Ok,
        Health::PermissionDenied,
        Health::Missing,
        Health::WatchFailed,
    ] {
        assert_eq!(
            serde_json::to_value(health).unwrap(),
            serde_json::json!(health.as_str())
        );
    }
}

#[test]
fn snippet_highlights_are_byte_ranges_and_brackets_stay_text() {
    let s = Snippet::from_marked("😀 [draft] \u{E000}invoice\u{E001} total");
    assert_eq!(s.text, "😀 [draft] invoice total");
    // "😀 [draft] " is 13 bytes: the emoji takes four.
    assert_eq!(s.highlights, vec![[13, 20]]);
    assert_eq!(&s.text[13..20], "invoice");
    // An unclosed highlight ends with the text.
    assert_eq!(
        Snippet::from_marked("\u{E000}open").highlights,
        vec![[0, 4]]
    );
}

#[test]
fn file_error_codes_classify_failures_and_keep_one_name() {
    use std::io::{Error as IoError, ErrorKind};
    let io = |kind| crate::Error::Io {
        path: "/f".into(),
        source: IoError::from(kind),
    };
    assert_eq!(
        FileErrorCode::classify(&io(ErrorKind::PermissionDenied)),
        FileErrorCode::PermissionDenied
    );
    assert_eq!(
        FileErrorCode::classify(&io(ErrorKind::UnexpectedEof)),
        FileErrorCode::ReadFailed
    );
    assert_eq!(
        FileErrorCode::classify(&crate::Error::ExtractionTimeout {
            path: "/f".into(),
            seconds: 30
        }),
        FileErrorCode::TimedOut
    );
    assert_eq!(
        FileErrorCode::classify(&crate::Error::Pdf("bad xref".into())),
        FileErrorCode::ExtractFailed
    );
    assert_eq!(
        FileErrorCode::classify(&crate::Error::PdfPasswordProtected),
        FileErrorCode::PasswordProtected
    );
    assert_eq!(
        FileErrorCode::classify(&crate::Error::Model("nan".into())),
        FileErrorCode::EmbedFailed
    );
    assert_eq!(
        FileErrorCode::classify(&crate::Error::Db(rusqlite::Error::InvalidQuery)),
        FileErrorCode::WriteFailed
    );
    for code in FileErrorCode::ALL {
        assert_eq!(
            serde_json::to_value(code).unwrap(),
            serde_json::json!(code.as_str())
        );
    }
}

#[test]
fn errors_cross_ipc_as_codes_with_parameters() {
    let json = |e: crate::Error| serde_json::to_value(ErrorCode::from(&e)).unwrap();
    assert_eq!(
        json(crate::Error::RootIdNotFound(7)),
        serde_json::json!({"code": "RootIdNotFound", "id": 7})
    );
    assert_eq!(
        json(crate::Error::InvalidSetting {
            field: "ui.transparency_intensity",
            reason: "english prose".into()
        }),
        serde_json::json!({"code": "InvalidSetting", "field": "ui.transparency_intensity"})
    );
    assert_eq!(json(crate::Error::EngineStarting)["code"], "EngineStarting");
    assert_eq!(
        json(crate::Error::Engine("boom".into()))["code"],
        "Internal"
    );
}

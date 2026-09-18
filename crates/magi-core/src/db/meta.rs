//! Key-value metadata table (`meta`): pipeline version and model IDs
//! (SPEC.md §5.5). Implemented starting M3.

use rusqlite::{Connection, OptionalExtension, params};

use crate::error::Result;

pub fn get(conn: &Connection, key: &str) -> Result<Option<String>> {
    conn.query_row(
        "SELECT value FROM meta WHERE key = ?1",
        params![key],
        |row| row.get(0),
    )
    .optional()
    .map_err(crate::error::Error::Db)
}

pub fn set(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn open_test_db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open(&dir.path().join("magi.db")).unwrap();
        (dir, conn)
    }

    #[test]
    fn get_missing_key_returns_none() {
        let (_dir, conn) = open_test_db();
        assert_eq!(get(&conn, "text_model_id").unwrap(), None);
    }

    #[test]
    fn set_then_get_roundtrips() {
        let (_dir, conn) = open_test_db();
        set(&conn, "text_model_id", "fake-v1").unwrap();
        assert_eq!(
            get(&conn, "text_model_id").unwrap(),
            Some("fake-v1".to_string())
        );
    }

    #[test]
    fn set_overwrites_existing_value() {
        let (_dir, conn) = open_test_db();
        set(&conn, "text_model_id", "fake-v1").unwrap();
        set(&conn, "text_model_id", "intfloat/multilingual-e5-small").unwrap();
        assert_eq!(
            get(&conn, "text_model_id").unwrap(),
            Some("intfloat/multilingual-e5-small".to_string())
        );
    }
}

//! One application SQLite file; each non-credential document has its own schema version.
use rusqlite::{Connection, OptionalExtension};
use serde::{Serialize, de::DeserializeOwned};
use std::{io::Read, path::Path};
fn error(_: impl std::fmt::Display) -> String {
    "应用数据存储失败，请检查磁盘、权限或文件版本；旧数据已保留".into()
}
fn open(path: &Path) -> Result<Connection, String> {
    let mut c = crate::connection_store::open(path)?;
    let version: i64 = c
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(error)?;
    if version < 3 {
        drop(c);
        crate::connection_store::load(path)?;
        c = crate::connection_store::open(path)?;
    }
    c.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(error)?;
    c.execute_batch("CREATE TABLE IF NOT EXISTS app_documents(name TEXT PRIMARY KEY NOT NULL, version INTEGER NOT NULL, value TEXT NOT NULL CHECK(json_valid(value))); ").map_err(error)?;
    Ok(c)
}
pub fn load<T: DeserializeOwned + Serialize + Default>(
    legacy: &Path,
    key: &str,
    limit: usize,
    validate: impl Fn(&T) -> bool,
) -> Result<T, String> {
    let mut connection = open(&legacy.with_file_name("connections.db"))?;
    let c = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(error)?;
    let stored: Option<(i64, String)> = c
        .query_row(
            "SELECT version,value FROM app_documents WHERE name=?1",
            [key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(error)?;
    if let Some((version, text)) = stored {
        if version != 1 || text.len() > limit {
            return Err(error("version"));
        }
        let value: T = serde_json::from_str(&text).map_err(error)?;
        if !validate(&value) {
            return Err(error("value"));
        }
        c.commit().map_err(error)?;
        return Ok(value);
    }
    let value: T = if legacy.is_file() {
        let mut bytes = Vec::new();
        std::fs::File::open(legacy)
            .map_err(error)?
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(error)?;
        if bytes.len() > limit {
            return Err(error("size"));
        }
        serde_json::from_slice(&bytes).map_err(error)?
    } else {
        T::default()
    };
    if !validate(&value) {
        return Err(error("version"));
    }
    let text = serde_json::to_string(&value).map_err(error)?;
    c.execute(
        "INSERT OR IGNORE INTO app_documents VALUES(?1,1,?2)",
        rusqlite::params![key, text],
    )
    .map_err(error)?;
    c.commit().map_err(error)?;
    Ok(value)
}
pub fn save<T: Serialize>(legacy: &Path, key: &str, value: &T, limit: usize) -> Result<(), String> {
    let text = serde_json::to_string(value).map_err(error)?;
    if text.len() > limit {
        return Err(error("size"));
    }
    let mut c = open(&legacy.with_file_name("connections.db"))?;
    let tx = c
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(error)?;
    let version: Option<i64> = tx
        .query_row(
            "SELECT version FROM app_documents WHERE name=?1",
            [key],
            |r| r.get(0),
        )
        .optional()
        .map_err(error)?;
    if version.is_some_and(|v| v != 1) {
        return Err(error("version"));
    }
    tx.execute("INSERT INTO app_documents VALUES(?1,1,?2) ON CONFLICT(name) DO UPDATE SET value=excluded.value",rusqlite::params![key,text]).map_err(error)?;
    tx.commit().map_err(error)
}
pub fn settings(path: &Path) -> Result<crate::preferences::Preferences, String> {
    let legacy = path.with_file_name("settings.json");
    let c = open(path)?;
    let present: bool = c
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM app_documents WHERE name='settings')",
            [],
            |r| r.get(0),
        )
        .map_err(error)?;
    if !present {
        let old = path.with_file_name("settings.db");
        if old.is_file() {
            let old = Connection::open_with_flags(old, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(error)?;
            let version: i64 = old
                .pragma_query_value(None, "user_version", |r| r.get(0))
                .map_err(error)?;
            if version != 1 {
                return Err(error("future settings"));
            }
            let text: Option<String> = old
                .query_row("SELECT value FROM settings WHERE id=1", [], |r| r.get(0))
                .optional()
                .map_err(error)?;
            if let Some(text) = text {
                if text.len() > 1_048_576 {
                    return Err(error("size"));
                }
                let prefs: crate::preferences::Preferences =
                    serde_json::from_str(&text).map_err(error)?;
                let text = serde_json::to_string(&prefs).map_err(error)?;
                c.execute(
                    "INSERT OR IGNORE INTO app_documents VALUES('settings',1,?1)",
                    [text],
                )
                .map_err(error)?;
            }
        }
    }
    load(
        &legacy,
        "settings",
        1_048_576,
        |_: &crate::preferences::Preferences| true,
    )
}
#[cfg(test)]
mod tests {
    #[test]
    fn settings_database_is_imported_once_and_new_note_index_is_valid() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("settings.db");
        let c = rusqlite::Connection::open(&old).unwrap();
        c.execute_batch(
            "CREATE TABLE settings(id INTEGER PRIMARY KEY,value TEXT); PRAGMA user_version=1;",
        )
        .unwrap();
        c.execute(
            "INSERT INTO settings VALUES(1,?1)",
            [r#"{"general":{"font_size":18.0},"database":{"page_size":100}}"#],
        )
        .unwrap();
        let shared = dir.path().join("connections.db");
        assert_eq!(super::settings(&shared).unwrap().general.font_size, 18.);
        c.execute("UPDATE settings SET value='{}'", []).unwrap();
        assert_eq!(super::settings(&shared).unwrap().database.page_size, 100);
        let index: turbodbn_services::note_store::Index = super::load(
            &dir.path().join("note-library.json"),
            "note-library",
            1024,
            |index: &turbodbn_services::note_store::Index| index.version == 1,
        )
        .unwrap();
        assert_eq!(index.version, 1);
        assert!(old.is_file());
    }
    #[test]
    fn imports_once_and_never_overwrites_future_or_invalid_documents() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("note-library.json");
        std::fs::write(&legacy, "[1,2]").unwrap();
        let first: Vec<i32> = super::load(&legacy, "notes", 100, |_| true).unwrap();
        assert_eq!(first, vec![1, 2]);
        std::fs::write(&legacy, "[9]").unwrap();
        let second: Vec<i32> = super::load(&legacy, "notes", 100, |_| true).unwrap();
        assert_eq!(second, first);
        super::save(&legacy, "notes", &vec![3], 100).unwrap();
        assert_eq!(std::fs::read_to_string(&legacy).unwrap(), "[9]");
        let c = super::open(&legacy.with_file_name("connections.db")).unwrap();
        c.execute("UPDATE app_documents SET version=2 WHERE name='notes'", [])
            .unwrap();
        assert!(super::load::<Vec<i32>>(&legacy, "notes", 100, |_| true).is_err());
        assert!(super::save(&legacy, "notes", &vec![4], 100).is_err());
    }
}

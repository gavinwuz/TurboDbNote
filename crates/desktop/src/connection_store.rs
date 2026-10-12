//! Local metadata and current-user DPAPI encrypted passwords.
use chrono::{DateTime, Local, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use std::{collections::HashMap, io::Read, path::Path, time::Duration};
use turbodbn_services::mysql::{self, Profile};
use uuid::Uuid;

#[derive(Clone, Default)]
pub struct History {
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub last_used_at: Option<String>,
    pub credential_id: Option<Uuid>,
}
pub struct Snapshot {
    pub profiles: Vec<Profile>,
    pub history: HashMap<Uuid, History>,
}
const APPLICATION_ID: i64 = 0x5444424e;
fn db_error(_: impl std::fmt::Display) -> String {
    "连接存储读写失败，请检查权限、磁盘空间或数据库是否被其他程序锁定".into()
}
pub(crate) fn open(path: &Path) -> Result<Connection, String> {
    let parent = path.parent().ok_or("连接存储目录无效")?;
    std::fs::create_dir_all(parent).map_err(db_error)?;
    let c = Connection::open(path).map_err(db_error)?;
    c.busy_timeout(Duration::from_secs(5)).map_err(db_error)?;
    let version: i64 = c
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(db_error)?;
    let app: i64 = c
        .pragma_query_value(None, "application_id", |r| r.get(0))
        .map_err(db_error)?;
    if version > 3 || (app != 0 && app != APPLICATION_ID) {
        return Err("不支持的连接数据库版本，未修改原文件".into());
    }
    if version >= 1 && app != APPLICATION_ID {
        return Err("连接数据库标识无效，未修改原文件".into());
    }
    c.pragma_update(None, "foreign_keys", "ON")
        .map_err(db_error)?;
    Ok(c)
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct LegacyHistory {
    created: HashMap<Uuid, String>,
}
fn legacy_history(path: &Path) -> Result<LegacyHistory, String> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(LegacyHistory::default()),
        Err(e) => return Err(db_error(e)),
    };
    let mut bytes = Vec::new();
    file.take(65537).read_to_end(&mut bytes).map_err(db_error)?;
    if bytes.len() > 65536 {
        return Err("旧导航文件过大，迁移未完成".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "旧导航文件格式无效，迁移未完成".into())
}
pub fn load(path: &Path) -> Result<Snapshot, String> {
    let mut c = open(path)?;
    // The transaction includes initialization: a failed import can be retried safely.
    let tx = c
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(db_error)?;
    let version: i64 = tx
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(db_error)?;
    if version == 0 {
        let count:i64=tx.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",[],|r|r.get(0)).map_err(db_error)?;
        if count != 0 {
            return Err("连接存储文件包含未知表，未覆盖原文件".into());
        }
        let profiles = mysql::load_profiles(&path.with_file_name("connections.json"))?;
        let legacy = legacy_history(&path.with_file_name("database-navigation.json"))?;
        tx.execute_batch("CREATE TABLE connections(id TEXT PRIMARY KEY NOT NULL, profile TEXT NOT NULL CHECK(json_valid(profile)), created_at TEXT, updated_at TEXT, last_used_at TEXT); CREATE INDEX connections_used ON connections(last_used_at DESC);").map_err(db_error)?;
        for p in profiles {
            // A legacy date remains a date, never an invented midnight timestamp.
            let date = legacy
                .created
                .get(&p.id)
                .filter(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok());
            let json = serde_json::to_string(&p).map_err(db_error)?;
            if json.len() > 65536 {
                return Err("旧连接记录过大，迁移未完成".into());
            }
            tx.execute(
                "INSERT INTO connections(id,profile,created_at) VALUES(?1,?2,?3)",
                params![p.id.to_string(), json, date],
            )
            .map_err(db_error)?;
        }
        tx.pragma_update(None, "application_id", APPLICATION_ID)
            .map_err(db_error)?;
        tx.pragma_update(None, "user_version", 1)
            .map_err(db_error)?;
    }
    if version <= 1 {
        tx.execute_batch("ALTER TABLE connections ADD COLUMN credential_id TEXT; UPDATE connections SET credential_id=id WHERE json_extract(profile,'$.remember_password')=1; CREATE UNIQUE INDEX connection_credential_ref ON connections(credential_id) WHERE credential_id IS NOT NULL; CREATE TABLE credential_gc(id TEXT PRIMARY KEY NOT NULL);").map_err(db_error)?;
        tx.pragma_update(None, "user_version", 2)
            .map_err(db_error)?;
    }
    if version <= 2 {
        tx.execute_batch("CREATE TABLE encrypted_passwords(id TEXT PRIMARY KEY NOT NULL, format INTEGER NOT NULL CHECK(format=1), ciphertext BLOB NOT NULL);").map_err(db_error)?;
        tx.pragma_update(None, "user_version", 3)
            .map_err(db_error)?;
    }
    tx.commit().map_err(db_error)?;
    c.pragma_update(None, "journal_mode", "WAL")
        .map_err(db_error)?;
    let mut statement=c.prepare("SELECT id,profile,created_at,updated_at,last_used_at,credential_id FROM connections ORDER BY rowid LIMIT 201").map_err(db_error)?;
    let rows = statement
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                History {
                    created_at: r.get(2)?,
                    updated_at: r.get(3)?,
                    last_used_at: r.get(4)?,
                    credential_id: r
                        .get::<_, Option<String>>(5)?
                        .map(|id| Uuid::parse_str(&id).map_err(|_| rusqlite::Error::InvalidQuery))
                        .transpose()?,
                },
            ))
        })
        .map_err(db_error)?;
    let mut profiles = Vec::new();
    let mut history = HashMap::new();
    for row in rows {
        let (id, json, times) = row.map_err(db_error)?;
        if json.len() > 65536 {
            return Err("连接记录过大".into());
        }
        let p: Profile = serde_json::from_str(&json).map_err(|_| "连接记录格式无效")?;
        p.validate()?;
        if p.remember_password != times.credential_id.is_some() {
            return Err("连接的凭据引用无效，未修改记录".into());
        }
        if p.id.to_string() != id {
            return Err("连接记录标识不一致".into());
        }
        history.insert(p.id, times);
        profiles.push(p);
    }
    if profiles.len() > 200 {
        return Err("连接记录超过 200 条".into());
    }
    Ok(Snapshot { profiles, history })
}
/// Rename or remove a project without touching credentials or connections.
pub fn rename_project(path: &Path, old: &str, new: &str) -> Result<(), String> {
    if old.is_empty() || new.chars().count() > 80 {
        return Err("项目名称无效".into());
    }
    let mut db = open(path)?;
    let tx = db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(db_error)?;
    let profiles = {
        let mut query = tx
            .prepare("SELECT profile FROM connections")
            .map_err(db_error)?;
        query
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)?
    };
    for json in profiles {
        let mut profile: Profile = serde_json::from_str(&json).map_err(db_error)?;
        if profile.group == old {
            profile.group = new.into();
            tx.execute(
                "UPDATE connections SET profile=?1,updated_at=?2 WHERE id=?3",
                params![
                    serde_json::to_string(&profile).map_err(db_error)?,
                    timestamp(),
                    profile.id.to_string()
                ],
            )
            .map_err(db_error)?;
        }
    }
    tx.commit().map_err(db_error)
}
pub fn save(
    path: &Path,
    profile: &Profile,
    credential_id: Option<Uuid>,
) -> Result<History, String> {
    save_inner(path, profile, credential_id, None)
}
pub fn save_password(path: &Path, profile: &Profile, password: &str) -> Result<History, String> {
    let secret = if profile.remember_password {
        let id = Uuid::new_v4();
        Some((
            id,
            turbodbn_services::password_protection::protect(id, password)?,
        ))
    } else {
        None
    };
    save_inner(
        path,
        profile,
        secret.as_ref().map(|s| s.0),
        secret.as_ref().map(|s| s.1.as_slice()),
    )
}
fn save_inner(
    path: &Path,
    profile: &Profile,
    credential_id: Option<Uuid>,
    ciphertext: Option<&[u8]>,
) -> Result<History, String> {
    if profile.remember_password != credential_id.is_some() {
        return Err("连接的密码保存状态与凭据引用不一致".into());
    }
    profile.validate()?;
    let json = serde_json::to_string(profile).map_err(db_error)?;
    if json.len() > 65536 {
        return Err("连接记录过大".into());
    }
    let mut c = open(path)?;
    let tx = c
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(db_error)?;
    let existing: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM connections WHERE id=?1)",
            [profile.id.to_string()],
            |r| r.get(0),
        )
        .map_err(db_error)?;
    let count: i64 = tx
        .query_row("SELECT count(*) FROM connections", [], |r| r.get(0))
        .map_err(db_error)?;
    if !existing && count >= 200 {
        return Err("最多保存 200 个连接".into());
    }
    let now = timestamp();
    tx.execute("INSERT OR IGNORE INTO credential_gc(id) SELECT credential_id FROM connections WHERE id=?1 AND credential_id IS NOT NULL AND (?2 IS NULL OR credential_id<>?2)",params![profile.id.to_string(),credential_id.map(|id|id.to_string())]).map_err(db_error)?;
    tx.execute("INSERT INTO connections(id,profile,created_at,updated_at,credential_id) VALUES(?1,?2,?3,?3,?4) ON CONFLICT(id) DO UPDATE SET profile=excluded.profile,updated_at=excluded.updated_at,credential_id=excluded.credential_id",params![profile.id.to_string(),json,now,credential_id.map(|id|id.to_string())]).map_err(db_error)?;
    if let (Some(id), Some(blob)) = (credential_id, ciphertext) {
        tx.execute(
            "INSERT INTO encrypted_passwords(id,format,ciphertext) VALUES(?1,1,?2)",
            params![id.to_string(), blob],
        )
        .map_err(db_error)?;
    }
    tx.execute("DELETE FROM encrypted_passwords WHERE NOT EXISTS(SELECT 1 FROM connections WHERE credential_id=encrypted_passwords.id)", []).map_err(db_error)?;
    let history = tx
        .query_row(
            "SELECT created_at,updated_at,last_used_at,credential_id FROM connections WHERE id=?1",
            [profile.id.to_string()],
            |r| {
                Ok(History {
                    created_at: r.get(0)?,
                    updated_at: r.get(1)?,
                    last_used_at: r.get(2)?,
                    credential_id: r
                        .get::<_, Option<String>>(3)?
                        .map(|id| Uuid::parse_str(&id).map_err(|_| rusqlite::Error::InvalidQuery))
                        .transpose()?,
                })
            },
        )
        .map_err(db_error)?;
    tx.commit().map_err(db_error)?;
    Ok(history)
}
/// Import basic profiles as new records only; commit the entire selection atomically.
pub fn import_profiles(
    path: &Path,
    profiles: &[Profile],
) -> Result<HashMap<Uuid, History>, String> {
    let mut c = open(path)?;
    let tx = c
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(db_error)?;
    let count: i64 = tx
        .query_row("SELECT count(*) FROM connections", [], |r| r.get(0))
        .map_err(db_error)?;
    if count + profiles.len() as i64 > 200 {
        return Err("最多保存 200 个连接".into());
    }
    let now = timestamp();
    let mut history = HashMap::new();
    for p in profiles {
        p.validate()?;
        if p.remember_password {
            return Err("导入不接受密码引用".into());
        }
        if p.kind != turbodbn_services::DatabaseKind::Sqlite && p.database.trim().is_empty() {
            return Err("请补全数据库名".into());
        }
        let data = serde_json::to_string(p).map_err(db_error)?;
        if data.len() > 65536 {
            return Err("连接记录过大".into());
        }
        tx.execute(
            "INSERT INTO connections(id,profile,created_at,updated_at) VALUES(?1,?2,?3,?3)",
            params![p.id.to_string(), data, now],
        )
        .map_err(db_error)?;
        history.insert(
            p.id,
            History {
                created_at: Some(now.clone()),
                updated_at: Some(now.clone()),
                ..Default::default()
            },
        );
    }
    tx.commit().map_err(db_error)?;
    Ok(history)
}
pub fn mark_used(path: &Path, id: Uuid, at: &str) -> Result<(), String> {
    let c = open(path)?;
    c.execute("UPDATE connections SET last_used_at=?2 WHERE id=?1 AND (last_used_at IS NULL OR last_used_at<?2)",params![id.to_string(),at]).map_err(db_error)?;
    Ok(())
}
pub fn delete(path: &Path, id: Uuid) -> Result<(), String> {
    let mut c = open(path)?;
    let tx = c
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(db_error)?;
    tx.execute("INSERT OR IGNORE INTO credential_gc(id) SELECT credential_id FROM connections WHERE id=?1 AND credential_id IS NOT NULL",[id.to_string()]).map_err(db_error)?;
    tx.execute("DELETE FROM connections WHERE id=?1", [id.to_string()])
        .map_err(db_error)?;
    tx.execute("DELETE FROM encrypted_passwords WHERE NOT EXISTS(SELECT 1 FROM connections WHERE credential_id=encrypted_passwords.id)", []).map_err(db_error)?;
    tx.commit().map_err(db_error)
}
/// Retired references are recorded in the same transaction as the profile change.
/// A write transaction prevents another connection from reclaiming a key during cleanup.
pub fn cleanup_credentials(path: &Path) -> Result<(), String> {
    let mut c = open(path)?;
    let tx = c
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(db_error)?;
    let ids = {
        let mut q=tx.prepare("SELECT id FROM credential_gc WHERE NOT EXISTS(SELECT 1 FROM connections WHERE connections.credential_id=credential_gc.id) OR EXISTS(SELECT 1 FROM encrypted_passwords WHERE encrypted_passwords.id=credential_gc.id) LIMIT 200").map_err(db_error)?;
        q.query_map([], |r| r.get::<_, String>(0))
            .map_err(db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)?
    };
    for id in ids {
        let key = Uuid::parse_str(&id).map_err(|_| "待清理凭据引用无效")?;
        turbodbn_services::credentials::delete(key)
            .map_err(|_| "连接已保存，但旧系统凭据清理失败；下次启动将重试")?;
        tx.execute("DELETE FROM credential_gc WHERE id=?1", [id])
            .map_err(db_error)?;
    }
    tx.commit().map_err(db_error)
}
pub fn read_password(path: &Path, id: Uuid) -> Result<Option<String>, String> {
    let c = open(path)?;
    let blob: Option<Vec<u8>> = c
        .query_row(
            "SELECT ciphertext FROM encrypted_passwords WHERE id=?1 AND format=1",
            [id.to_string()],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error)?;
    match blob {
        Some(blob) => turbodbn_services::password_protection::unprotect(id, &blob).map(Some),
        None => turbodbn_services::credentials::read(id), // Retryable legacy migration.
    }
}
pub fn migrate_passwords(path: &Path) -> Result<(), String> {
    let mut c = open(path)?;
    let tx = c
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(db_error)?;
    let ids = {
        let mut q = tx.prepare("SELECT credential_id FROM connections WHERE credential_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM encrypted_passwords WHERE id=credential_id)").map_err(db_error)?;
        q.query_map([], |r| r.get::<_, String>(0))
            .map_err(db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)?
    };
    for reference in ids {
        let id = Uuid::parse_str(&reference).map_err(|_| "旧凭据引用无效")?;
        let value = turbodbn_services::credentials::read(id)?
            .ok_or("旧密码不可用，请重新输入并保存；原配置已保留")?;
        let blob = turbodbn_services::password_protection::protect(id, &value)?;
        tx.execute(
            "INSERT INTO encrypted_passwords(id,format,ciphertext) VALUES(?1,1,?2)",
            params![reference, blob],
        )
        .map_err(db_error)?;
        tx.execute(
            "INSERT OR IGNORE INTO credential_gc(id) VALUES(?1)",
            [reference],
        )
        .map_err(db_error)?;
    }
    tx.commit().map_err(db_error)?;
    cleanup_credentials(path)
}
pub fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
pub fn display_time(value: Option<&str>) -> Option<String> {
    let value = value?;
    if let Ok(time) = DateTime::parse_from_rfc3339(value) {
        Some(
            time.with_timezone(&Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string(),
        )
    } else {
        day(Some(value))
    }
}
pub fn day(value: Option<&str>) -> Option<String> {
    let value = value?;
    if let Ok(t) = DateTime::parse_from_rfc3339(value) {
        Some(t.with_timezone(&Local).format("%Y-%m-%d").to_string())
    } else {
        chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
            .ok()
            .map(|d| d.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn project_rename_and_removal_preserve_connection_and_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.db");
        load(&path).unwrap();
        let p = Profile {
            name: "fixture".into(),
            database: "demo".into(),
            group: "old".into(),
            remember_password: cfg!(windows),
            ..Default::default()
        };
        save_password(&path, &p, "test-secret").unwrap();
        let before = load(&path).unwrap();
        let key = before.history[&p.id].credential_id;
        rename_project(&path, "old", "new").unwrap();
        let renamed = load(&path).unwrap();
        assert_eq!(renamed.profiles[0].group, "new");
        assert_eq!(renamed.history[&p.id].credential_id, key);
        if cfg!(windows) {
            assert!(read_password(&path, key.unwrap()).unwrap().as_deref() == Some("test-secret"));
        } else {
            assert!(key.is_none());
        }
        rename_project(&path, "new", "").unwrap();
        let removed = load(&path).unwrap();
        assert_eq!(removed.profiles.len(), 1);
        assert!(removed.profiles[0].group.is_empty());
        assert_eq!(
            removed.history[&p.id].created_at,
            before.history[&p.id].created_at
        );
    }
    #[test]
    fn migration_preserves_unknown_dates_and_does_not_resurrect_deleted_profiles() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.db");
        let p = Profile {
            database: "demo".into(),
            ..Default::default()
        };
        mysql::save_profiles(&dir.path().join("connections.json"), vec![p.clone()]).unwrap();
        let original = std::fs::read(dir.path().join("connections.json")).unwrap();
        let initial = load(&path).unwrap();
        assert_eq!(initial.profiles.len(), 1);
        assert!(initial.history[&p.id].created_at.is_none());
        mark_used(&path, p.id, "2026-09-20T12:00:00.000Z").unwrap();
        let mut changed = p.clone();
        changed.name = "Renamed".into();
        let history = save(&path, &changed, None).unwrap();
        assert!(history.created_at.is_none());
        assert_eq!(
            history.last_used_at.as_deref(),
            Some("2026-09-20T12:00:00.000Z")
        );
        assert!(history.updated_at.is_some());
        delete(&path, p.id).unwrap();
        assert!(load(&path).unwrap().profiles.is_empty());
        assert_eq!(
            std::fs::read(dir.path().join("connections.json")).unwrap(),
            original
        );
    }
    #[test]
    fn timestamps_survive_edits_and_stale_usage_cannot_move_time_backwards() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.db");
        load(&path).unwrap();
        let p = Profile::default();
        let first = save(&path, &p, None).unwrap();
        assert!(first.created_at.is_some());
        mark_used(&path, p.id, "2026-09-20T12:00:00.000Z").unwrap();
        mark_used(&path, p.id, "2026-09-19T12:00:00.000Z").unwrap();
        let second = save(&path, &p, None).unwrap();
        assert_eq!(first.created_at, second.created_at);
        assert_eq!(
            second.last_used_at.as_deref(),
            Some("2026-09-20T12:00:00.000Z")
        );
        let c = Connection::open(&path).unwrap();
        let json: String = c
            .query_row("SELECT profile FROM connections", [], |r| r.get(0))
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(value.get("password").is_none());
    }
    #[test]
    fn version_one_store_keeps_legacy_credential_reference() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.db");
        let p = Profile {
            remember_password: true,
            ..Default::default()
        };
        let c = Connection::open(&path).unwrap();
        c.execute_batch("CREATE TABLE connections(id TEXT PRIMARY KEY,profile TEXT NOT NULL,created_at TEXT,updated_at TEXT,last_used_at TEXT)").unwrap();
        c.execute(
            "INSERT INTO connections VALUES(?1,?2,'2026-09-20',NULL,NULL)",
            params![p.id.to_string(), serde_json::to_string(&p).unwrap()],
        )
        .unwrap();
        c.pragma_update(None, "application_id", APPLICATION_ID)
            .unwrap();
        c.pragma_update(None, "user_version", 1).unwrap();
        let snapshot = load(&path).unwrap();
        assert_eq!(snapshot.history[&p.id].credential_id, Some(p.id));
        assert_eq!(
            snapshot.history[&p.id].created_at.as_deref(),
            Some("2026-09-20")
        );
        assert_eq!(
            c.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            3
        );
    }
    #[cfg(windows)]
    #[test]
    fn credential_rotation_and_deletion_keep_profile_and_secret_paired() {
        use turbodbn_services::credentials;
        struct Cleanup(Vec<Uuid>);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                for id in &self.0 {
                    let _ = credentials::delete(*id);
                }
            }
        }
        let keys = [Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()];
        let _cleanup = Cleanup(keys.to_vec());
        let values = [Uuid::new_v4().to_string(), Uuid::new_v4().to_string()];
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.db");
        load(&path).unwrap();
        let mut p = Profile {
            remember_password: true,
            ..Default::default()
        };
        credentials::write(keys[0], &values[0]).unwrap();
        save(&path, &p, Some(keys[0])).unwrap();
        credentials::write(keys[1], &values[1]).unwrap();
        p.host = "new-target".into();
        save(&path, &p, Some(keys[1])).unwrap();
        let current = load(&path).unwrap();
        assert_eq!(current.profiles[0].host, "new-target");
        assert_eq!(current.history[&p.id].credential_id, Some(keys[1]));
        cleanup_credentials(&path).unwrap();
        assert!(credentials::read(keys[0]).unwrap().is_none());
        // Assert booleans so a failure can never print credential values.
        assert!(credentials::read(keys[1]).unwrap().as_deref() == Some(values[1].as_str()));
        credentials::write(keys[2], &values[0]).unwrap();
        let mut invalid = p.clone();
        invalid.name.clear();
        assert!(save(&path, &invalid, Some(keys[2])).is_err());
        credentials::delete(keys[2]).unwrap();
        assert_eq!(
            load(&path).unwrap().history[&p.id].credential_id,
            Some(keys[1])
        );
        delete(&path, p.id).unwrap();
        cleanup_credentials(&path).unwrap();
        assert!(credentials::read(keys[1]).unwrap().is_none());
    }
    #[cfg(windows)]
    #[test]
    fn encrypted_save_is_atomic_and_clear_removes_ciphertext() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.db");
        load(&path).unwrap();
        let mut p = Profile {
            remember_password: true,
            ..Default::default()
        };
        let value = Uuid::new_v4().to_string();
        let h = save_password(&path, &p, &value).unwrap();
        let id = h.credential_id.unwrap();
        assert!(read_password(&path, id).unwrap().as_deref() == Some(value.as_str()));
        let c = Connection::open(&path).unwrap();
        let blob: Vec<u8> = c
            .query_row("SELECT ciphertext FROM encrypted_passwords", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(!blob.windows(value.len()).any(|w| w == value.as_bytes()));
        c.execute_batch("CREATE TRIGGER fail_secret BEFORE INSERT ON encrypted_passwords BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
        p.host = "new-host".into();
        assert!(save_password(&path, &p, &Uuid::new_v4().to_string()).is_err());
        assert!(load(&path).unwrap().profiles[0].host != p.host);
        assert!(read_password(&path, id).unwrap().as_deref() == Some(value.as_str()));
        c.execute_batch("DROP TRIGGER fail_secret;").unwrap();
        p.remember_password = false;
        save_password(&path, &p, "").unwrap();
        let count: i64 = c
            .query_row("SELECT count(*) FROM encrypted_passwords", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
        assert!(load(&path).unwrap().history[&p.id].credential_id.is_none());
    }
    #[cfg(windows)]
    #[test]
    fn legacy_password_migration_preserves_reference_then_retires_os_credential() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.db");
        load(&path).unwrap();
        let p = Profile {
            remember_password: true,
            ..Default::default()
        };
        let id = Uuid::new_v4();
        let value = Uuid::new_v4().to_string();
        struct Cleanup(Uuid);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = turbodbn_services::credentials::delete(self.0);
            }
        }
        let _cleanup = Cleanup(id);
        turbodbn_services::credentials::write(id, &value).unwrap();
        save(&path, &p, Some(id)).unwrap();
        let c = Connection::open(&path).unwrap();
        c.execute_batch("CREATE TRIGGER fail_migration BEFORE INSERT ON encrypted_passwords BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
        assert!(migrate_passwords(&path).is_err());
        assert!(
            turbodbn_services::credentials::read(id).unwrap().as_deref() == Some(value.as_str())
        );
        c.execute_batch("DROP TRIGGER fail_migration;").unwrap();
        migrate_passwords(&path).unwrap();
        assert!(turbodbn_services::credentials::read(id).unwrap().is_none());
        assert!(read_password(&path, id).unwrap().as_deref() == Some(value.as_str()));
        migrate_passwords(&path).unwrap();
        assert_eq!(load(&path).unwrap().history[&p.id].credential_id, Some(id));
    }
    #[test]
    fn invalid_legacy_and_future_schema_are_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.db");
        std::fs::write(dir.path().join("connections.json"), b"invalid").unwrap();
        assert!(load(&path).is_err());
        mysql::save_profiles(&dir.path().join("connections.json"), vec![]).unwrap();
        assert!(load(&path).is_ok());
        let c = Connection::open(&path).unwrap();
        c.pragma_update(None, "user_version", 4).unwrap();
        assert!(load(&path).is_err());
        assert_eq!(
            c.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            4
        );
    }
}

#[cfg(test)]
mod import_tests {
    use super::*;
    #[test]
    fn batch_import_never_overwrites_and_rolls_back_partial_batch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.db");
        load(&path).unwrap();
        let old = turbodbn_services::mysql::Profile {
            name: "Existing".into(),
            database: "demo".into(),
            remember_password: false,
            ..Default::default()
        };
        save(&path, &old, None).unwrap();
        let mut new = old.clone();
        new.id = Uuid::new_v4();
        new.name = "Imported".into();
        assert!(import_profiles(&path, &[new.clone(), old.clone()]).is_err());
        assert_eq!(load(&path).unwrap().profiles.len(), 1);
        import_profiles(&path, &[new]).unwrap();
        let snapshot = load(&path).unwrap();
        assert_eq!(snapshot.profiles.len(), 2);
        assert!(
            snapshot
                .profiles
                .iter()
                .any(|p| p.id == old.id && p.name == "Existing")
        );
    }
}

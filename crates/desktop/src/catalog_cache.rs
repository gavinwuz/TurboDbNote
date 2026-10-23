//! Persistent directory snapshots. Incomplete refreshes never replace a readable snapshot.
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use turbodbn_services::{database::workbench::Object, mysql::Profile};

#[derive(Clone)]
pub struct Cache {
    path: PathBuf,
    target: String,
    database: String,
    category: String,
}
pub struct Page {
    pub objects: Vec<Object>,
    pub more: bool,
    pub updated: i64,
}
#[derive(Clone)]
pub struct SearchHit {
    pub database: String,
    pub object: Object,
}
#[derive(Clone, Default)]
pub struct SearchPage {
    pub hits: Vec<SearchHit>,
    pub more: bool,
    pub indexed: bool,
    pub complete: bool,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DatabaseRule {
    pub enabled: bool,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
}
impl DatabaseRule {
    pub fn valid(&self) -> bool {
        self.include.len() <= 32
            && self.exclude.len() <= 32
            && self.include.iter().chain(&self.exclude).all(|p| {
                !p.is_empty() && p.chars().count() <= 256 && !p.chars().any(char::is_control)
            })
    }
    pub fn matches(&self, name: &str) -> bool {
        if !self.enabled {
            return true;
        }
        let name = name.to_lowercase().chars().collect::<Vec<_>>();
        let matches =
            |pattern: &String| glob(&name, &pattern.to_lowercase().chars().collect::<Vec<_>>());
        (self.include.is_empty() || self.include.iter().any(matches))
            && !self.exclude.iter().any(matches)
    }
    fn register(&self, c: &Connection) -> Result<(), String> {
        let rule = self.clone();
        let mut cache = std::collections::HashMap::new();
        c.create_scalar_function(
            "directory_visible",
            1,
            rusqlite::functions::FunctionFlags::SQLITE_UTF8
                | rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
            move |ctx| {
                if !rule.enabled {
                    return Ok(true);
                }
                let name = ctx.get::<String>(0)?;
                Ok(*cache
                    .entry(name.clone())
                    .or_insert_with(|| rule.matches(&name)))
            },
        )
        .map_err(failure)
    }
}
fn glob(name: &[char], pattern: &[char]) -> bool {
    let (mut n, mut p, mut star, mut retry) = (0, 0, None, 0);
    while n < name.len() {
        if p < pattern.len() && matches!(pattern[p], '%' | '*') {
            star = Some(p);
            p += 1;
            retry = n;
        } else if p < pattern.len() && pattern[p] == name[n] {
            n += 1;
            p += 1;
        } else if let Some(last) = star {
            retry += 1;
            n = retry;
            p = last + 1;
        } else {
            return false;
        }
    }
    while p < pattern.len() && matches!(pattern[p], '%' | '*') {
        p += 1;
    }
    p == pattern.len()
}
fn failure(_: impl std::fmt::Display) -> String {
    "database.cache.storage_error".into()
}
impl Cache {
    pub fn ticket() -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static LAST: AtomicU64 = AtomicU64::new(0);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        let previous = LAST
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |old| {
                Some(now.max(old.saturating_add(1)))
            })
            .expect("infallible ticket update");
        format!(
            "{:020}-{}",
            now.max(previous.saturating_add(1)),
            uuid::Uuid::new_v4()
        )
    }
    pub fn new(path: &Path, profile: &Profile, database: &str, category: &str) -> Self {
        // No credentials, display names or runtime generations in persistent target identity.
        let target = serde_json::to_string(&(
            profile.id,
            profile.kind,
            &profile.host,
            profile.port,
            &profile.user,
            &profile.database,
            profile.list_all_databases,
            &profile.sqlite_path,
            &profile.driver_id,
            (
                &profile.driver,
                &profile.legacy_library,
                &profile.charset,
                profile.mysql_raw_gbk,
            ),
        ))
        .expect("serializable directory identity");
        Self {
            path: path.with_file_name("connections.db"),
            target,
            database: database.into(),
            category: category.into(),
        }
    }
    fn open(&self) -> Result<Connection, String> {
        let mut c = crate::connection_store::open(&self.path)?;
        c.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(failure)?;
        let initialized:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='directory_version')",[],|r|r.get(0)).map_err(failure)?;
        if initialized {
            let version: i64 = c
                .query_row(
                    "SELECT version FROM directory_version WHERE id=1",
                    [],
                    |r| r.get(0),
                )
                .map_err(failure)?;
            if version != 1 {
                return Err(failure("version"));
            }
            return Ok(c);
        }
        // Initialize atomically once. Normal local filtering must not acquire a write lock.
        let tx = c
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(failure)?;
        tx.execute_batch("CREATE TABLE IF NOT EXISTS directory_version(id INTEGER PRIMARY KEY CHECK(id=1),version INTEGER NOT NULL);
            INSERT OR IGNORE INTO directory_version VALUES(1,1);").map_err(failure)?;
        let version: i64 = tx
            .query_row(
                "SELECT version FROM directory_version WHERE id=1",
                [],
                |r| r.get(0),
            )
            .map_err(failure)?;
        if version != 1 {
            return Err(failure("version"));
        }
        tx.execute_batch("CREATE TABLE IF NOT EXISTS directory_snapshots(target TEXT NOT NULL,db TEXT NOT NULL,category TEXT NOT NULL,snapshot TEXT NOT NULL,updated INTEGER NOT NULL,PRIMARY KEY(target,db,category));
            CREATE TABLE IF NOT EXISTS directory_refreshes(target TEXT NOT NULL,db TEXT NOT NULL,category TEXT NOT NULL,snapshot TEXT NOT NULL,PRIMARY KEY(target,db,category));
            CREATE TABLE IF NOT EXISTS directory_objects(target TEXT NOT NULL,db TEXT NOT NULL,category TEXT NOT NULL,snapshot TEXT NOT NULL,identity TEXT NOT NULL,name TEXT NOT NULL,search TEXT NOT NULL,payload TEXT NOT NULL,PRIMARY KEY(target,db,category,snapshot,identity));
            CREATE INDEX IF NOT EXISTS directory_names ON directory_objects(target,db,category,snapshot,name,identity);").map_err(failure)?;
        tx.commit().map_err(failure)?;
        Ok(c)
    }
    pub fn begin(&self, snapshot: &str) -> Result<(), String> {
        let c = self.open()?;
        // Build the optional prefix index only during explicit catalog synchronization.
        c.execute_batch("CREATE INDEX IF NOT EXISTS directory_prefix ON directory_objects(target,db,category,snapshot,name COLLATE NOCASE,identity)").map_err(failure)?;
        c.execute("INSERT INTO directory_refreshes VALUES(?1,?2,?3,?4) ON CONFLICT(target,db,category) DO UPDATE SET snapshot=excluded.snapshot WHERE excluded.snapshot>directory_refreshes.snapshot",params![self.target,self.database,self.category,snapshot]).map_err(failure)?;
        Ok(())
    }
    pub fn append(&self, snapshot: &str, objects: &[Object]) -> Result<(), String> {
        let mut c = self.open()?;
        let tx = c.transaction().map_err(failure)?;
        {
            let mut insert = tx
                .prepare_cached(
                    "INSERT OR REPLACE INTO directory_objects VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                )
                .map_err(failure)?;
            for o in objects {
                let identity = serde_json::to_string(&(&o.schema, &o.name, &o.kind, &o.parent))
                    .map_err(failure)?;
                let search = format!("{} {} {}", o.schema, o.name, o.parent).to_lowercase();
                insert
                    .execute(params![
                        self.target,
                        self.database,
                        self.category,
                        snapshot,
                        identity,
                        o.name,
                        search,
                        serde_json::to_string(o).map_err(failure)?
                    ])
                    .map_err(failure)?;
            }
        }
        tx.commit().map_err(failure)
    }
    pub fn commit(&self, snapshot: &str) -> Result<(), String> {
        let mut c = self.open()?;
        let tx = c.transaction().map_err(failure)?;
        let active: Option<String> = tx.query_row("SELECT snapshot FROM directory_refreshes WHERE target=?1 AND db=?2 AND category=?3",params![self.target,self.database,self.category],|r|r.get(0)).optional().map_err(failure)?;
        if active.as_deref().is_some_and(|active| active != snapshot) {
            return Err("mcp.stale".into());
        }
        tx.execute("INSERT INTO directory_snapshots VALUES(?1,?2,?3,?4,unixepoch()) ON CONFLICT(target,db,category) DO UPDATE SET snapshot=excluded.snapshot,updated=excluded.updated",params![self.target,self.database,self.category,snapshot]).map_err(failure)?;
        tx.execute("DELETE FROM directory_objects WHERE target=?1 AND db=?2 AND category=?3 AND snapshot<>?4",params![self.target,self.database,self.category,snapshot]).map_err(failure)?;
        tx.commit().map_err(failure)
    }
    pub fn discard(&self, snapshot: &str) -> Result<(), String> {
        self.open()?.execute("DELETE FROM directory_objects WHERE target=?1 AND db=?2 AND category=?3 AND snapshot=?4 AND NOT EXISTS(SELECT 1 FROM directory_snapshots WHERE target=?1 AND db=?2 AND category=?3 AND snapshot=?4)",params![self.target,self.database,self.category,snapshot]).map_err(failure)?;
        Ok(())
    }
    /// Search only published local snapshots. Missing scopes never trigger remote reads.
    #[cfg(test)]
    pub fn search(&self, query: &str, offset: usize, limit: usize) -> Result<SearchPage, String> {
        self.search_filtered(query, offset, limit, &DatabaseRule::default())
    }
    #[cfg(test)]
    pub fn search_filtered(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
        rule: &DatabaseRule,
    ) -> Result<SearchPage, String> {
        self.search_scope(query, offset, limit, rule, false)
    }
    #[cfg(test)]
    pub fn search_scope(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
        rule: &DatabaseRule,
        programs: bool,
    ) -> Result<SearchPage, String> {
        self.search_visible(query, offset, limit, rule, programs, None)
    }
    pub fn search_visible(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
        rule: &DatabaseRule,
        programs: bool,
        databases: Option<&[String]>,
    ) -> Result<SearchPage, String> {
        if !(1..=200).contains(&limit) {
            return Err(failure("page"));
        }
        let mut connection = self.open()?;
        rule.register(&connection)?;
        let databases = databases.map(|names| {
            names
                .iter()
                .cloned()
                .collect::<std::collections::HashSet<_>>()
        });
        connection
            .create_scalar_function(
                "directory_scope",
                1,
                rusqlite::functions::FunctionFlags::SQLITE_UTF8
                    | rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
                move |ctx| {
                    Ok(databases.as_ref().is_none_or(|names| {
                        ctx.get::<String>(0).is_ok_and(|name| names.contains(&name))
                    }))
                },
            )
            .map_err(failure)?;
        let c = connection.transaction().map_err(failure)?;
        let complete: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM directory_snapshots WHERE target=?1 AND category='databases') AND NOT EXISTS(SELECT 1 FROM directory_objects n JOIN directory_snapshots s ON s.target=n.target AND s.db=n.db AND s.category=n.category AND s.snapshot=n.snapshot WHERE n.target=?1 AND n.category='databases' AND directory_scope(n.name) AND directory_visible(n.name) AND (NOT EXISTS(SELECT 1 FROM directory_snapshots d WHERE d.target=n.target AND d.db=n.name AND d.category=CASE WHEN ?2 THEN 'procedure' ELSE 'relations' END) OR NOT EXISTS(SELECT 1 FROM directory_snapshots d WHERE d.target=n.target AND d.db=n.name AND d.category=CASE WHEN ?2 THEN 'trigger' ELSE 'index' END) OR (?2 AND (NOT EXISTS(SELECT 1 FROM directory_snapshots d WHERE d.target=n.target AND d.db=n.name AND d.category='function') OR NOT EXISTS(SELECT 1 FROM directory_snapshots d WHERE d.target=n.target AND d.db=n.name AND d.category='event')))))",params![self.target, programs],|r|r.get(0)).map_err(failure)?;
        let indexed: bool = complete || c.query_row("SELECT EXISTS(SELECT 1 FROM directory_snapshots WHERE target=?1 AND ((?2=0 AND category IN ('relations','index')) OR (?2=1 AND category IN ('procedure','trigger','function','event'))) AND directory_scope(db) AND directory_visible(db))",params![self.target, programs],|r|r.get::<_,bool>(0)).map_err(failure)?;
        let mut statement = c.prepare("SELECT o.db,o.payload FROM directory_objects o JOIN directory_snapshots s ON s.target=o.target AND s.db=o.db AND s.category=o.category AND s.snapshot=o.snapshot WHERE o.target=?1 AND directory_scope(o.db) AND directory_visible(o.db) AND ((?5=0 AND o.category IN ('relations','index') AND json_extract(o.payload,'$.kind') IN ('table','view','index')) OR (?5=1 AND o.category IN ('procedure','trigger','function','event'))) AND instr(o.search,?2)>0 ORDER BY o.db,json_extract(o.payload,'$.kind'),o.name,o.identity LIMIT ?3 OFFSET ?4").map_err(failure)?;
        let mut hits = statement
            .query_map(
                params![
                    self.target,
                    query.to_lowercase(),
                    limit + 1,
                    offset,
                    programs
                ],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .map_err(failure)?
            .map(|r| {
                let (database, payload) = r.map_err(failure)?;
                Ok(SearchHit {
                    database,
                    object: serde_json::from_str(&payload).map_err(failure)?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let more = hits.len() > limit;
        hits.truncate(limit);
        Ok(SearchPage {
            hits,
            more,
            indexed,
            complete,
        })
    }
    /// Local published relation names only. Never opens or refreshes a remote directory.
    pub fn table_prefix(
        &self,
        prefix: &str,
        schema: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Object>, String> {
        if prefix.len() > 256 || limit == 0 || limit > 60 {
            return Err(failure("prefix"));
        }
        if !self.path.exists() {
            return Ok(vec![]);
        }
        let c = Connection::open_with_flags(
            &self.path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(failure)?;
        c.busy_timeout(std::time::Duration::from_millis(20))
            .map_err(failure)?;
        let exists:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='directory_version')",[],|r|r.get(0)).map_err(failure)?;
        if !exists {
            return Ok(vec![]);
        }
        let version: i64 = c
            .query_row(
                "SELECT version FROM directory_version WHERE id=1",
                [],
                |r| r.get(0),
            )
            .map_err(failure)?;
        if version != 1 {
            return Err(failure("version"));
        }
        c.create_scalar_function(
            "table_prefix_match",
            2,
            rusqlite::functions::FunctionFlags::SQLITE_UTF8
                | rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
            |ctx| {
                Ok(ctx
                    .get::<String>(0)?
                    .to_lowercase()
                    .starts_with(&ctx.get::<String>(1)?.to_lowercase()))
            },
        )
        .map_err(failure)?;
        let ascii = prefix.is_ascii();
        let pattern = prefix
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
            + "%";
        let indexed = if ascii {
            " AND o.name LIKE ?6 ESCAPE '\\'"
        } else {
            " AND ?6 IS NOT NULL"
        };
        let sql = format!(
            "SELECT json_extract(o.payload,'$.schema'),o.name,substr(coalesce(json_extract(o.payload,'$.comment'),''),1,2048),json_extract(o.payload,'$.kind') FROM directory_objects o JOIN directory_snapshots s ON s.target=o.target AND s.db=o.db AND s.category=o.category AND s.snapshot=o.snapshot WHERE o.target=?1 AND o.db=?2 AND o.category='relations' AND json_extract(o.payload,'$.kind') IN ('table','view') AND table_prefix_match(o.name,?3) AND (?4 IS NULL OR json_extract(o.payload,'$.schema')=?4 COLLATE NOCASE){indexed} ORDER BY lower(o.name)<>lower(?3),o.name,o.identity LIMIT ?5"
        );
        let mut statement = c.prepare(&sql).map_err(failure)?;
        statement
            .query_map(
                params![self.target, self.database, prefix, schema, limit, pattern],
                |row| {
                    Ok(Object {
                        schema: row.get(0)?,
                        name: row.get(1)?,
                        comment: row.get(2)?,
                        kind: row.get(3)?,
                        parent: String::new(),
                    })
                },
            )
            .map_err(failure)?
            .map(|row| row.map_err(failure))
            .collect()
    }
    pub fn read(&self, query: &str, offset: usize, limit: usize) -> Result<Option<Page>, String> {
        self.read_filtered(query, offset, limit, &DatabaseRule::default())
    }
    pub fn read_filtered(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
        rule: &DatabaseRule,
    ) -> Result<Option<Page>, String> {
        if !(1..=200).contains(&limit) {
            return Err(failure("page"));
        }
        let mut connection = self.open()?;
        rule.register(&connection)?;
        let c = connection.transaction().map_err(failure)?;
        let snapshot: Option<(String,i64)> = c.query_row("SELECT snapshot,updated FROM directory_snapshots WHERE target=?1 AND db=?2 AND category=?3",params![self.target,self.database,self.category],|r| Ok((r.get(0)?,r.get(1)?))).optional().map_err(failure)?;
        let Some((snapshot, updated)) = snapshot else {
            return Ok(None);
        };
        let mut statement = c.prepare("SELECT payload FROM directory_objects WHERE target=?1 AND db=?2 AND category=?3 AND snapshot=?4 AND directory_visible(name) AND instr(search,?5)>0 ORDER BY name,identity LIMIT ?6 OFFSET ?7").map_err(failure)?;
        let objects = statement
            .query_map(
                params![
                    self.target,
                    self.database,
                    self.category,
                    snapshot,
                    query.to_lowercase(),
                    limit + 1,
                    offset
                ],
                |r| r.get::<_, String>(0),
            )
            .map_err(failure)?
            .map(|r| serde_json::from_str(&r.map_err(failure)?).map_err(failure))
            .collect::<Result<Vec<Object>, String>>()?;
        let more = objects.len() > limit;
        Ok(Some(Page {
            objects: objects.into_iter().take(limit).collect(),
            more,
            updated,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn database_rules_are_full_name_case_insensitive_and_exclusions_win() {
        let rule = DatabaseRule {
            enabled: true,
            include: vec!["DRX%".into(), "项目*".into()],
            exclude: vec!["*_backup".into()],
        };
        for name in ["drx", "drxtest306", "项目开发"] {
            assert!(rule.matches(name), "{name}");
        }
        for name in ["other_drx", "drx_backup", "项目_backup"] {
            assert!(!rule.matches(name), "{name}");
        }
        let literal = DatabaseRule {
            enabled: true,
            include: vec!["a_b".into()],
            exclude: vec![],
        };
        assert!(literal.matches("a_b"));
        assert!(!literal.matches("acb"));
        assert!(!literal.matches("a_b_more"));
        let disabled = DatabaseRule {
            enabled: false,
            ..rule
        };
        assert!(disabled.matches("other"));
        assert_eq!(
            serde_json::from_str::<DatabaseRule>(&serde_json::to_string(&disabled).unwrap())
                .unwrap(),
            disabled
        );
    }
    #[test]
    fn database_rules_apply_before_local_pagination_and_object_filtering() {
        let dir = std::env::temp_dir().join(format!("tdbn-rules-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let profile = Profile::default();
        let path = dir.join("connections.db");
        let names = Cache::new(&path, &profile, "", "databases");
        let mut objects = (0..620)
            .map(|i| Object {
                name: format!("aaa_{i:04}"),
                kind: "database".into(),
                schema: String::new(),
                parent: String::new(),
                comment: String::new(),
            })
            .collect::<Vec<_>>();
        objects.extend(
            ["drxtest306", "drx_backup", "项目测试"]
                .into_iter()
                .map(|name| Object {
                    name: name.into(),
                    kind: "database".into(),
                    schema: String::new(),
                    parent: String::new(),
                    comment: String::new(),
                }),
        );
        names.append("names", &objects).unwrap();
        names.commit("names").unwrap();
        let rule = DatabaseRule {
            enabled: true,
            include: vec!["drx%".into()],
            exclude: vec!["*_backup".into()],
        };
        let first = names.read_filtered("", 0, 200, &rule).unwrap().unwrap();
        assert_eq!(first.objects.len(), 1);
        assert_eq!(first.objects[0].name, "drxtest306");
        assert!(!first.more);
        for database in ["drxtest306", "drx_backup"] {
            let cache = Cache::new(&path, &profile, database, "relations");
            let item = Object {
                name: "items".into(),
                kind: "table".into(),
                schema: String::new(),
                parent: String::new(),
                comment: String::new(),
            };
            cache.append("objects", &[item]).unwrap();
            cache.commit("objects").unwrap();
        }
        let matches = names.search_filtered("items", 0, 200, &rule).unwrap();
        assert_eq!(matches.hits.len(), 1);
        assert_eq!(matches.hits[0].database, "drxtest306");
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn searches_freeze_visible_database_names_and_separate_programs() {
        let dir = std::env::temp_dir().join(format!("tdbn-visible-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("connections.db");
        let profile = Profile::default();
        let object = |name: &str, kind: &str| Object {
            name: name.into(),
            kind: kind.into(),
            schema: String::new(),
            parent: String::new(),
            comment: String::new(),
        };
        let names = Cache::new(&path, &profile, "", "databases");
        names
            .append(
                "names",
                &[
                    object("visible%_库", "database"),
                    object("hidden", "database"),
                ],
            )
            .unwrap();
        names.commit("names").unwrap();
        for database in ["visible%_库", "hidden"] {
            for category in [
                "relations",
                "index",
                "procedure",
                "trigger",
                "function",
                "event",
            ] {
                let cache = Cache::new(&path, &profile, database, category);
                let kind = if category == "relations" {
                    "table"
                } else {
                    category
                };
                cache
                    .append("data", &[object(&format!("match_{kind}"), kind)])
                    .unwrap();
                cache.commit("data").unwrap();
            }
        }
        let visible = vec!["visible%_库".to_owned()];
        let rule = DatabaseRule::default();
        let relations = names
            .search_visible("match", 0, 200, &rule, false, Some(&visible))
            .unwrap();
        assert!(relations.complete);
        assert_eq!(relations.hits.len(), 2);
        assert!(relations.hits.iter().all(|hit| hit.database == visible[0]
            && matches!(hit.object.kind.as_str(), "table" | "index")));
        let programs = names
            .search_visible("match", 0, 200, &rule, true, Some(&visible))
            .unwrap();
        assert!(programs.complete);
        assert_eq!(programs.hits.len(), 4);
        assert!(programs.hits.iter().all(|hit| hit.database == visible[0]
            && !matches!(hit.object.kind.as_str(), "table" | "index")));
        let blocked = DatabaseRule {
            enabled: true,
            include: vec![],
            exclude: vec!["visible*".into()],
        };
        assert!(
            names
                .search_visible("match", 0, 200, &blocked, false, Some(&visible))
                .unwrap()
                .hits
                .is_empty()
        );
        assert!(
            names
                .search_visible("match", 0, 200, &rule, false, Some(&[]))
                .unwrap()
                .hits
                .is_empty()
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn local_search_reports_coverage_and_never_includes_unpublished_or_other_types() {
        let dir = std::env::temp_dir().join(format!("tdbn-filter-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let profile = Profile::default();
        let path = dir.join("connections.db");
        let relations = Cache::new(&path, &profile, "db", "relations");
        let empty = relations.search("item", 0, 200).unwrap();
        assert!(!empty.indexed && !empty.complete);
        let object = |name: &str, kind: &str| Object {
            name: name.into(),
            kind: kind.into(),
            schema: String::new(),
            parent: String::new(),
            comment: String::new(),
        };
        let databases = Cache::new(&path, &profile, "", "databases");
        databases
            .append("names", &[object("db", "database")])
            .unwrap();
        databases.commit("names").unwrap();
        relations
            .append(
                "tables",
                &[object("item_table", "table"), object("item_view", "view")],
            )
            .unwrap();
        relations.commit("tables").unwrap();
        let partial = relations.search("item", 0, 200).unwrap();
        assert!(partial.indexed && !partial.complete);
        let indexes = Cache::new(&path, &profile, "db", "index");
        indexes
            .append("indexes", &[object("item_index", "index")])
            .unwrap();
        indexes.commit("indexes").unwrap();
        let procedures = Cache::new(&path, &profile, "db", "procedure");
        procedures
            .append("procedures", &[object("item_procedure", "procedure")])
            .unwrap();
        procedures.commit("procedures").unwrap();
        relations
            .append("unpublished", &[object("item_ghost", "table")])
            .unwrap();
        let results = relations.search("item", 0, 2).unwrap();
        assert!(results.complete && results.more);
        assert_eq!(results.hits.len(), 2);
        let last = relations.search("item", 2, 2).unwrap();
        assert!(!last.more);
        assert_eq!(last.hits.len(), 1);
        assert!(relations.search("ghost", 0, 200).unwrap().hits.is_empty());
        assert!(
            relations
                .search("procedure", 0, 200)
                .unwrap()
                .hits
                .is_empty()
        );
        assert!(relations.search("%", 0, 200).unwrap().hits.is_empty());
        {
            let mut writer = relations.open().unwrap();
            let transaction = writer
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .unwrap();
            transaction
                .execute("UPDATE directory_version SET version=version", [])
                .unwrap();
            assert_eq!(
                relations.search("item", 0, 200).unwrap().hits.len(),
                3,
                "filtering must stay read-only while a writer is reserved"
            );
        }
        relations
            .open()
            .unwrap()
            .execute("UPDATE directory_version SET version=2", [])
            .unwrap();
        assert!(relations.search("item", 0, 200).is_err());
        assert_eq!(
            relations.open().err().as_deref(),
            Some("database.cache.storage_error")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn complete_snapshots_are_unbounded_paginated_and_atomic() {
        let dir = std::env::temp_dir().join(format!("tdbn-directory-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let profile = Profile::default();
        let cache = Cache::new(&dir.join("connections.db"), &profile, "db", "relations");
        let objects: Vec<_> = (0..6200)
            .map(|i| Object {
                name: format!("table_{i:05}"),
                kind: "table".into(),
                schema: String::new(),
                parent: String::new(),
                comment: String::new(),
            })
            .collect();
        for batch in objects.chunks(200) {
            cache.append("first", batch).unwrap();
        }
        assert!(cache.read("", 0, 200).unwrap().is_none());
        cache.commit("first").unwrap();
        assert_eq!(
            cache.read("", 6000, 200).unwrap().unwrap().objects.len(),
            200
        );
        assert!(!cache.read("", 6000, 200).unwrap().unwrap().more);
        assert_eq!(
            cache
                .read("TABLE_06199", 0, 200)
                .unwrap()
                .unwrap()
                .objects
                .len(),
            1
        );
        assert!(cache.read("%", 0, 200).unwrap().unwrap().objects.is_empty());
        let search = cache.search("TABLE_06199", 0, 200).unwrap();
        assert_eq!(search.hits.len(), 1);
        assert_eq!(search.hits[0].database, "db");
        assert!(search.indexed);
        assert!(!search.complete);
        assert!(cache.search("table", 6000, 200).unwrap().hits.len() == 200);
        let older = Cache::ticket();
        let newer = Cache::ticket();
        assert!(older < newer);
        cache.begin(&older).unwrap();
        cache.append(&older, &objects[..1]).unwrap();
        cache.begin(&newer).unwrap();
        cache.begin(&older).unwrap(); // Older work can start late on another background thread.
        assert!(
            cache.commit(&older).is_err(),
            "a newer refresh supersedes an older writer"
        );
        cache.discard(&older).unwrap();
        assert_eq!(
            cache.read("", 6000, 200).unwrap().unwrap().objects.len(),
            200
        );
        cache.append("interrupted", &objects[..1]).unwrap();
        cache.discard("interrupted").unwrap();
        assert_eq!(
            cache.read("", 6000, 200).unwrap().unwrap().objects.len(),
            200
        );
        let mut other = profile.clone();
        other.host = "different".into();
        assert!(
            Cache::new(&dir.join("connections.db"), &other, "db", "relations")
                .read("", 0, 200)
                .unwrap()
                .is_none()
        );
        let empty = Cache::ticket();
        cache.begin(&empty).unwrap();
        cache.append(&empty, &[]).unwrap();
        cache.commit(&empty).unwrap();
        assert!(cache.read("", 0, 200).unwrap().unwrap().objects.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod prefix_tests {
    use super::*;
    #[test]
    fn prefix_reads_full_published_scope_without_wildcard_interpretation_or_initialization() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("connections.db");
        let profile = Profile::default();
        let cache = Cache::new(&path, &profile, "db", "relations");
        assert!(cache.table_prefix("cq", None, 8).unwrap().is_empty());
        assert!(!path.exists());
        let mut objects = (0..6200)
            .map(|n| Object {
                name: format!("aaa_{n:05}"),
                schema: String::new(),
                kind: "table".into(),
                parent: String::new(),
                comment: String::new(),
            })
            .collect::<Vec<_>>();
        for name in ["cq_itemtype", "CQ_Inventory", "cq%literal", "cq_other"] {
            objects.push(Object {
                name: name.into(),
                schema: String::new(),
                kind: "table".into(),
                parent: String::new(),
                comment: "Cached table annotation".into(),
            });
        }
        cache.append("ready", &objects).unwrap();
        cache.commit("ready").unwrap();
        assert_eq!(
            cache.table_prefix("cq_it", None, 8).unwrap()[0].name,
            "cq_itemtype"
        );
        assert_eq!(
            cache.table_prefix("cq_it", None, 8).unwrap()[0].comment,
            "Cached table annotation"
        );
        assert_eq!(
            cache.table_prefix("cq%", None, 8).unwrap()[0].name,
            "cq%literal"
        );
        assert_eq!(cache.table_prefix("cq_i", None, 8).unwrap().len(), 2);
        cache
            .append(
                "pending",
                &[Object {
                    name: "cq_ghost".into(),
                    schema: String::new(),
                    kind: "table".into(),
                    parent: String::new(),
                    comment: String::new(),
                }],
            )
            .unwrap();
        assert!(cache.table_prefix("cq_g", None, 8).unwrap().is_empty());
        assert!(
            Cache::new(&path, &profile, "other", "relations")
                .table_prefix("cq", None, 8)
                .unwrap()
                .is_empty()
        );
        let c = Connection::open(&path).unwrap();
        c.execute("UPDATE directory_version SET version=2", [])
            .unwrap();
        assert!(cache.table_prefix("cq", None, 8).is_err());
    }
}

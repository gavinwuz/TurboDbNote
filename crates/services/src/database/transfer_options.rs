use super::*;
use serde::Serialize;

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub enum WriteMode {
    #[default]
    Append,
    Replace,
    Upsert,
}
#[derive(Clone, Serialize)]
pub struct Options {
    pub collation: Option<String>,
    pub crlf: bool,
    pub excluded_objects: Vec<(String, String)>,
    pub tables: Option<Vec<String>>,
    pub views: bool,
    pub triggers: bool,
    pub routines: bool,
    pub events: bool,
    pub batch_size: usize,
    pub write_mode: WriteMode,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            collation: None,
            crlf: false,
            excluded_objects: vec![],
            tables: None,
            views: true,
            triggers: true,
            routines: true,
            events: true,
            batch_size: 1000,
            write_mode: WriteMode::Append,
        }
    }
}
impl Options {
    pub(super) fn full_sqlite(&self) -> bool {
        self.tables.is_none() && self.views && self.triggers && self.excluded_objects.is_empty()
    }
    pub(super) fn validate(
        &self,
        mode: Mode,
        contents: Contents,
        profile: &Profile,
    ) -> Result<(), String> {
        if let Some(collation) = &self.collation
            && (mode != Mode::Copy
                || crate::mysql::uses_legacy(profile)
                || profile.kind != DatabaseKind::MySql
                || collation.is_empty()
                || collation.len() > 64
                || !collation
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_'))
        {
            return Err("排序规则仅支持 MySQL 新库，且须为合法名称".into());
        }
        if !self.excluded_objects.is_empty()
            && (mode == Mode::ImportFile
                || profile.kind == DatabaseKind::PostgreSql
                || (profile.kind == DatabaseKind::MySql
                    && self.excluded_objects.iter().any(|(kind, _)| kind != "view")))
        {
            return Err("当前驱动不支持这些对象的逐项排除，请按类型选择".into());
        }
        if !(1..=10000).contains(&self.batch_size) {
            return Err("分批行数须为 1–10000".into());
        }
        if self.tables.as_ref().is_some_and(Vec::is_empty) {
            return Err("请选择至少一个表".into());
        }
        if mode == Mode::ImportFile
            && (self.tables.is_some() || self.write_mode != WriteMode::Append)
        {
            return Err("SQL 文件按原始脚本执行，不支持改写导入模式或筛选表".into());
        }
        if self.write_mode != WriteMode::Append
            && (mode != Mode::ImportConnection || contents.schema || !contents.data)
        {
            return Err("覆盖或更新插入仅适用于连接间的仅数据导入".into());
        }
        if profile.kind == DatabaseKind::PostgreSql && self.write_mode != WriteMode::Append {
            return Err("PostgreSQL 当前仅支持追加；覆盖与主键更新尚未接通".into());
        }
        if crate::mysql::uses_legacy(profile)
            && (self.tables.is_some() || self.write_mode != WriteMode::Append)
        {
            return Err("旧版 MySQL 当前不支持选择表或修改写入模式".into());
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct ObjectChoice {
    pub name: String,
    pub kind: String,
}
#[derive(Clone, Default)]
pub struct Preview {
    pub objects: Vec<ObjectChoice>,
    pub table: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

pub fn preview_grid(endpoint: &Endpoint, table: Option<&str>) -> Result<Preview, String> {
    if endpoint.profile.kind == DatabaseKind::Sqlite {
        let control = Control::default();
        let db = sqlite_open(Path::new(&endpoint.profile.sqlite_path), false, &control)?;
        let objects = super::sqlite_objects(&db)?
            .into_iter()
            .map(|(kind, name, _)| ObjectChoice { name, kind })
            .collect::<Vec<_>>();
        let chosen = table
            .and_then(|t| objects.iter().find(|o| o.kind == "table" && o.name == t))
            .or_else(|| objects.iter().find(|o| o.kind == "table"));
        let mut preview = Preview::default();
        if let Some(object) = chosen {
            preview.table = object.name.clone();
            let mut fields = db
                .prepare(&format!("PRAGMA table_xinfo({})", quote(&object.name)))
                .map_err(db_error)?;
            preview.columns = fields
                .query_map([], |row| row.get::<_, String>(1))
                .map_err(db_error)?
                .take(16)
                .collect::<Result<Vec<_>, _>>()
                .map_err(db_error)?;
            let expressions=preview.columns.iter().map(|n| {let q=quote(n);format!("CASE WHEN typeof({q})='blob' THEN '[binary '||length({q})||' bytes]' ELSE substr(CAST({q} AS TEXT),1,128) END")}).collect::<Vec<_>>().join(",");
            let mut query = db
                .prepare(&format!(
                    "SELECT {expressions} FROM {} LIMIT 5",
                    quote(&object.name)
                ))
                .map_err(db_error)?;
            let mut rows = query.query([]).map_err(db_error)?;
            while let Some(row) = rows.next().map_err(db_error)? {
                preview.rows.push(
                    (0..preview.columns.len())
                        .map(|i| {
                            row.get::<_, Option<String>>(i)
                                .map(|v| v.unwrap_or_else(|| "NULL".into()))
                        })
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(db_error)?,
                );
            }
        }
        preview.objects = objects;
        return Ok(preview);
    }
    if crate::mysql::uses_legacy(&endpoint.profile) {
        use crate::database::{
            OperationKind, OperationResult,
            workbench::{Action, Reply, Request},
        };
        let c = Control::default();
        let result = service_operation(
            endpoint,
            OperationKind::Workbench(Box::new(Request {
                database: endpoint.profile.database.clone(),
                action: Action::Catalog,
            })),
            &c,
        )?;
        let OperationResult::Workbench(reply) = result else {
            return Err("无法读取兼容数据库目录".into());
        };
        let Reply::Catalog { objects, .. } = *reply else {
            return Err("无法读取兼容数据库目录".into());
        };
        let mut preview = Preview {
            objects: objects
                .iter()
                .map(|o| ObjectChoice {
                    name: o.name.clone(),
                    kind: o.kind.clone(),
                })
                .collect(),
            ..Default::default()
        };
        let object = table
            .and_then(|name| objects.iter().find(|o| o.name == name && o.kind == "table"))
            .or_else(|| objects.iter().find(|o| o.kind == "table"));
        if let Some(object) = object {
            preview.table = object.name.clone();
            if let Ok(OperationResult::Workbench(reply)) = service_operation(
                endpoint,
                OperationKind::Workbench(Box::new(Request {
                    database: endpoint.profile.database.clone(),
                    action: Action::Page {
                        object: object.clone(),
                        offset: 0,
                        page_size: 5,
                    },
                })),
                &c,
            ) && let Reply::Page { rows, .. } = *reply
            {
                preview.columns = rows.columns.into_iter().take(16).collect();
                preview.rows = rows
                    .values
                    .into_iter()
                    .take(5)
                    .map(|r| {
                        r.into_iter()
                            .take(16)
                            .map(|v| {
                                v.unwrap_or_else(|| "NULL".into())
                                    .chars()
                                    .take(128)
                                    .collect()
                            })
                            .collect()
                    })
                    .collect();
            }
        }
        return Ok(preview);
    }
    let objects = inventory(endpoint)?
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(name, kind)| ObjectChoice {
            name: name.to_owned(),
            kind: if kind == "BASE TABLE" {
                "table"
            } else {
                "view"
            }
            .into(),
        })
        .collect::<Vec<_>>();
    let chosen = table
        .and_then(|t| objects.iter().find(|o| o.name == t && o.kind == "table"))
        .or_else(|| objects.iter().find(|o| o.kind == "table"));
    let mut preview = Preview::default();
    if let Some(object) = chosen {
        preview.table = object.name.clone();
        let mysql = endpoint.profile.kind == DatabaseKind::MySql;
        let fields = if mysql {
            let columns = mysql_columns(endpoint, &object.name)?;
            preview.columns = columns.iter().take(16).map(|c| c.name.clone()).collect();
            preview
                .columns
                .iter()
                .map(|n| format!("CAST(LEFT({},128) AS CHAR)", mysql_quote(n)))
                .collect::<Vec<_>>()
                .join(",")
        } else {
            let literal =
                crate::database::workbench::literal_for(DatabaseKind::PostgreSql, &object.name);
            let names = query(
                endpoint,
                &format!(
                    "SELECT quote_ident(attname) FROM pg_attribute WHERE attrelid={literal}::regclass AND attnum>0 AND NOT attisdropped ORDER BY attnum LIMIT 16;"
                ),
            )?;
            preview.columns = names.lines().map(str::to_owned).collect();
            preview
                .columns
                .iter()
                .map(|n| format!("left({n}::text,128)"))
                .collect::<Vec<_>>()
                .join(",")
        };
        if !fields.is_empty() {
            let table = if mysql {
                mysql_quote(&object.name)
            } else {
                object.name.clone()
            };
            preview.rows = query_inner(
                endpoint,
                &format!("SELECT {fields} FROM {table} LIMIT 5;"),
                true,
            )?
            .lines()
            .map(|line| line.split('\t').map(str::to_owned).collect())
            .collect();
        }
    }
    preview.objects = objects;
    Ok(preview)
}

pub(super) fn sqlite_objects(
    db: &rusqlite::Connection,
    options: &Options,
) -> Result<Vec<(String, String, String)>, String> {
    let objects = super::sqlite_objects(db)?;
    let mut result = Vec::new();
    for (kind, name, sql) in objects {
        if options
            .excluded_objects
            .contains(&(kind.clone(), name.clone()))
        {
            continue;
        }
        let include = match kind.as_str() {
            "view" => options.views && options.tables.is_none(),
            "trigger" => options.triggers,
            _ => true,
        };
        if !include {
            continue;
        }
        if let Some(tables) = &options.tables {
            let owner: String = db
                .query_row(
                    "SELECT tbl_name FROM sqlite_schema WHERE type=?1 AND name=?2",
                    [&kind, &name],
                    |r| r.get(0),
                )
                .map_err(db_error)?;
            if !tables.contains(&owner) {
                continue;
            }
        }
        result.push((kind, name, sql));
    }
    if let Some(tables) = &options.tables {
        for table in tables {
            if !result
                .iter()
                .any(|(kind, name, _)| kind == "table" && name == table)
            {
                return Err("所选表已变化，请重新读取预览".into());
            }
        }
    }
    Ok(result)
}

pub(super) fn import_data(
    source: &Endpoint,
    target: &Endpoint,
    options: &Options,
    c: &Control,
    tx: &async_channel::Sender<Event>,
) -> Result<(), String> {
    match source.profile.kind {
        DatabaseKind::Sqlite => sqlite_data(source, target, options, c, tx),
        DatabaseKind::MySql => mysql_data(source, target, options, c, tx),
        DatabaseKind::PostgreSql => {
            let file = tempfile::NamedTempFile::new().map_err(io_error)?;
            pg_export(
                source,
                file.path(),
                Contents {
                    schema: false,
                    data: true,
                },
                options,
                c,
                tx,
            )?;
            pg_import(target, file.path(), c, tx)
        }
        _ => Err("当前驱动不支持数据传输".into()),
    }
}

pub(super) fn preflight_data(
    source: &Endpoint,
    target: &Endpoint,
    options: &Options,
    c: &Control,
) -> Result<(), String> {
    let tables = if source.profile.kind == DatabaseKind::Sqlite {
        let db = sqlite_open(Path::new(&source.profile.sqlite_path), false, c)?;
        sqlite_objects(&db, options)?
            .into_iter()
            .filter(|o| o.0 == "table")
            .map(|o| o.1)
            .collect::<Vec<_>>()
    } else if source.profile.kind == DatabaseKind::MySql {
        options.tables.clone().unwrap_or(
            inventory(source)?
                .lines()
                .filter_map(|line| {
                    line.split_once('\t')
                        .filter(|(_, kind)| *kind == "BASE TABLE")
                        .map(|(name, _)| name.to_owned())
                })
                .collect(),
        )
    } else {
        return Ok(());
    };
    for table in tables {
        c.check()?;
        let (from, to) = if source.profile.kind == DatabaseKind::Sqlite {
            let from = sqlite_open(Path::new(&source.profile.sqlite_path), false, c)?;
            let to = sqlite_open(Path::new(&target.profile.sqlite_path), false, c)?;
            (sqlite_columns(&from, &table)?, sqlite_columns(&to, &table)?)
        } else {
            (
                mysql_columns(source, &table)?,
                mysql_columns(target, &table)?,
            )
        };
        if from.is_empty() || from != to {
            return Err(format!(
                "表 {table} 的来源与目标列定义或主键不同；请先建立匹配表结构，或选择复制到新库。未执行该表写入。"
            ));
        }
        if options.write_mode == WriteMode::Upsert {
            suffix(&to, source.profile.kind == DatabaseKind::MySql)?;
        }
    }
    Ok(())
}

#[derive(PartialEq)]
struct Column {
    name: String,
    kind: String,
    pk: bool,
}
fn sqlite_columns(db: &rusqlite::Connection, table: &str) -> Result<Vec<Column>, String> {
    let mut query = db
        .prepare(&format!("PRAGMA table_xinfo({})", quote(table)))
        .map_err(db_error)?;
    let rows = query
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(5)?,
                r.get::<_, i64>(6)?,
            ))
        })
        .map_err(db_error)?;
    Ok(rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?
        .into_iter()
        .filter(|r| r.3 == 0)
        .map(|r| Column {
            name: r.0,
            kind: r.1,
            pk: r.2 > 0,
        })
        .collect())
}
fn suffix(columns: &[Column], mysql: bool) -> Result<String, String> {
    let keys = columns.iter().filter(|c| c.pk).collect::<Vec<_>>();
    if keys.is_empty() {
        return Err("更新插入要求来源与目标均有相同主键".into());
    }
    let q = |s: &str| if mysql { mysql_quote(s) } else { quote(s) };
    let updates = columns
        .iter()
        .filter(|c| !c.pk)
        .map(|c| {
            let n = q(&c.name);
            if mysql {
                format!("{n}=VALUES({n})")
            } else {
                format!("{n}=excluded.{n}")
            }
        })
        .collect::<Vec<_>>();
    if mysql {
        let updates = if updates.is_empty() {
            let n = q(&keys[0].name);
            vec![format!("{n}={n}")]
        } else {
            updates
        };
        Ok(format!(" ON DUPLICATE KEY UPDATE {}", updates.join(",")))
    } else {
        let keys = keys
            .iter()
            .map(|c| q(&c.name))
            .collect::<Vec<_>>()
            .join(",");
        Ok(if updates.is_empty() {
            format!(" ON CONFLICT ({keys}) DO NOTHING")
        } else {
            format!(" ON CONFLICT ({keys}) DO UPDATE SET {}", updates.join(","))
        })
    }
}
fn sqlite_data(
    source: &Endpoint,
    target: &Endpoint,
    options: &Options,
    c: &Control,
    tx: &async_channel::Sender<Event>,
) -> Result<(), String> {
    let from = sqlite_open(Path::new(&source.profile.sqlite_path), false, c)?;
    let mut to = sqlite_open(Path::new(&target.profile.sqlite_path), true, c)?;
    from.execute_batch("BEGIN").map_err(db_error)?;
    let tables = sqlite_objects(&from, options)?
        .into_iter()
        .filter(|o| o.0 == "table")
        .map(|o| o.1)
        .collect::<Vec<_>>();
    to.execute_batch("PRAGMA foreign_keys=ON;")
        .map_err(db_error)?;
    let transaction = to.transaction().map_err(db_error)?;
    transaction
        .execute_batch("PRAGMA defer_foreign_keys=ON;")
        .map_err(db_error)?;
    let mut plans = Vec::new();
    for table in &tables {
        let columns = sqlite_columns(&from, table)?;
        if columns.is_empty() || columns != sqlite_columns(&transaction, table)? {
            return Err("来源与目标列或主键不同，请核对表结构".into());
        }
        let suffix = if options.write_mode == WriteMode::Upsert {
            suffix(&columns, false)?
        } else {
            String::new()
        };
        plans.push((table, columns, suffix));
    }
    if options.write_mode == WriteMode::Replace {
        for table in &tables {
            c.check()?;
            transaction
                .execute(&format!("DELETE FROM {}", quote(table)), [])
                .map_err(db_error)?;
        }
    }
    let mut progress = Progress {
        unit: Unit::Tables,
        total: Some(tables.len() as u64),
        importing: true,
        ..Default::default()
    };
    for (table, columns, suffix) in plans {
        let fields = columns
            .iter()
            .map(|c| quote(&c.name))
            .collect::<Vec<_>>()
            .join(",");
        let mut input = from
            .prepare(&format!("SELECT {fields} FROM {}", quote(table)))
            .map_err(db_error)?;
        let mut rows = input.query([]).map_err(db_error)?;
        let mut batch = Vec::new();
        let mut bytes = 0usize;
        // Stay within conservative SQLite parameter and memory limits, even on wide tables.
        let max_rows = options.batch_size.min((900 / columns.len()).max(1));
        let flush = |values: &mut Vec<rusqlite::types::Value>| -> Result<(), String> {
            if values.is_empty() {
                return Ok(());
            }
            let group = format!("({})", vec!["?"; columns.len()].join(","));
            let groups = vec![group; values.len() / columns.len()].join(",");
            transaction
                .execute(
                    &format!(
                        "INSERT INTO {} ({fields}) VALUES {groups}{suffix}",
                        quote(table)
                    ),
                    rusqlite::params_from_iter(values.iter()),
                )
                .map_err(db_error)?;
            values.clear();
            Ok(())
        };
        while let Some(row) = rows.next().map_err(db_error)? {
            c.check()?;
            for i in 0..columns.len() {
                let value: rusqlite::types::Value = row.get(i).map_err(db_error)?;
                bytes += match &value {
                    rusqlite::types::Value::Text(v) => v.len(),
                    rusqlite::types::Value::Blob(v) => v.len(),
                    _ => 8,
                };
                batch.push(value);
            }
            progress.rows += 1;
            if batch.len() / columns.len() >= max_rows || bytes >= 4 * 1024 * 1024 {
                flush(&mut batch)?;
                bytes = 0;
                notify(tx, progress.clone());
            }
        }
        flush(&mut batch)?;
        progress.completed += 1;
        notify(tx, progress.clone());
    }
    c.check()?;
    transaction.commit().map_err(db_error)
}

fn mysql_quote(s: &str) -> String {
    format!("`{}`", s.replace('`', "``"))
}

// Only regroup INSERT lines emitted by our own mysqldump invocation, never user SQL.
fn dump_insert(line: &str) -> Option<(&str, &str)> {
    if !line.starts_with("INSERT INTO ") {
        return None;
    }
    let bytes = line.as_bytes();
    let mut quoted = false;
    let mut i = 12;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            if quoted && bytes.get(i + 1) == Some(&b'`') {
                i += 2;
                continue;
            }
            quoted = !quoted;
        }
        if !quoted && bytes[i..].starts_with(b" VALUES ") {
            let end = i + 8;
            let row = line[end..].trim_end().strip_suffix(';')?;
            return (row.starts_with('(') && row.ends_with(')')).then_some((&line[..end], row));
        }
        i += 1;
    }
    None
}
pub(super) fn rebatch_mysql(path: &Path, batch_size: usize, c: &Control) -> Result<(), String> {
    let staged = tempfile::NamedTempFile::new().map_err(io_error)?;
    let mut out = BufWriter::new(staged.reopen().map_err(io_error)?);
    let mut reader = BufReader::new(File::open(path).map_err(io_error)?);
    let mut prefix = String::new();
    let mut values = String::new();
    let mut count = 0;
    let flush = |out: &mut BufWriter<File>,
                 prefix: &str,
                 values: &mut String,
                 count: &mut usize|
     -> Result<(), String> {
        if *count > 0 {
            writeln!(out, "{prefix}{values};").map_err(io_error)?;
            values.clear();
            *count = 0;
        }
        Ok(())
    };
    loop {
        c.check()?;
        let mut line = Vec::new();
        let n = reader
            .by_ref()
            .take(8 * 1024 * 1024)
            .read_until(b'\n', &mut line)
            .map_err(io_error)?;
        if n == 0 {
            break;
        }
        if !line.ends_with(b"\n") && n == 8 * 1024 * 1024 {
            // Keep oversized native rows streaming and unchanged.
            flush(&mut out, &prefix, &mut values, &mut count)?;
            loop {
                out.write_all(&line).map_err(io_error)?;
                if line.ends_with(b"\n") {
                    break;
                }
                c.check()?;
                line.clear();
                if reader
                    .by_ref()
                    .take(8 * 1024 * 1024)
                    .read_until(b'\n', &mut line)
                    .map_err(io_error)?
                    == 0
                {
                    break;
                }
            }
            continue;
        }
        if let Some((next, row)) = std::str::from_utf8(&line).ok().and_then(dump_insert) {
            if next != prefix || count >= batch_size || values.len() + row.len() > 4 * 1024 * 1024 {
                flush(&mut out, &prefix, &mut values, &mut count)?;
            }
            prefix = next.to_owned();
            if count > 0 {
                values.push(',');
            }
            values.push_str(row);
            count += 1;
        } else {
            flush(&mut out, &prefix, &mut values, &mut count)?;
            out.write_all(&line).map_err(io_error)?;
        }
    }
    flush(&mut out, &prefix, &mut values, &mut count)?;
    out.flush().map_err(io_error)?;
    c.check()?;
    std::fs::copy(staged.path(), path).map_err(io_error)?;
    Ok(())
}
fn unhex(s: &str) -> Result<String, String> {
    if !s.is_ascii() || !s.len().is_multiple_of(2) {
        return Err("无效列名编码".into());
    }
    let bytes = (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| "无效列名编码".to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    String::from_utf8(bytes).map_err(|_| "列名不是 UTF-8".into())
}
fn mysql_columns(endpoint: &Endpoint, table: &str) -> Result<Vec<Column>, String> {
    let table = crate::database::workbench::literal_for(DatabaseKind::MySql, table);
    let text = query(
        endpoint,
        &format!(
            "SELECT HEX(COLUMN_NAME),COLUMN_TYPE,COLUMN_KEY,EXTRA FROM information_schema.COLUMNS WHERE TABLE_SCHEMA=DATABASE() AND BINARY TABLE_NAME=BINARY {table} ORDER BY ORDINAL_POSITION;"
        ),
    )?;
    text.lines()
        .filter(|l| !l.contains("VIRTUAL GENERATED") && !l.contains("STORED GENERATED"))
        .map(|line| {
            let parts = line.split('\t').collect::<Vec<_>>();
            if parts.len() < 3 {
                return Err("无法读取列元数据".into());
            }
            Ok(Column {
                name: unhex(parts[0])?,
                kind: parts[1].into(),
                pk: parts[2] == "PRI",
            })
        })
        .collect()
}
fn mysql_data(
    source: &Endpoint,
    target: &Endpoint,
    options: &Options,
    c: &Control,
    tx: &async_channel::Sender<Event>,
) -> Result<(), String> {
    if crate::mysql::uses_legacy(&source.profile) {
        return Err("旧版 MySQL 仅数据传输请使用 SQL 文件".into());
    }
    let available = inventory(source)
        .map_err(|_| "无法读取来源表列表")?
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .filter(|(_, kind)| *kind == "BASE TABLE")
        .map(|(n, _)| n.to_owned())
        .collect::<Vec<_>>();
    let tables = options.tables.clone().unwrap_or_else(|| available.clone());
    let file = tempfile::NamedTempFile::new().map_err(io_error)?;
    let mut out = BufWriter::new(file.reopen().map_err(io_error)?);
    writeln!(out, "SET SESSION sql_mode=CONCAT_WS(',',@@sql_mode,'STRICT_ALL_TABLES','NO_AUTO_VALUE_ON_ZERO');").map_err(io_error)?;
    let mut plans = Vec::new();
    for table in &tables {
        c.check()?;
        if !available.contains(table) {
            return Err("所选表已变化，请重新预览".into());
        }
        let columns = mysql_columns(source, table).map_err(|e| format!("无法读取来源列：{e}"))?;
        if columns.is_empty()
            || columns
                != mysql_columns(target, table).map_err(|e| format!("无法读取目标列：{e}"))?
        {
            return Err("来源与目标列或主键不同，请核对表结构".into());
        }
        if columns.iter().any(|col| {
            [
                "geometry",
                "point",
                "linestring",
                "polygon",
                "multipoint",
                "multilinestring",
                "multipolygon",
                "geometrycollection",
            ]
            .iter()
            .any(|kind| col.kind.eq_ignore_ascii_case(kind))
        }) {
            return Err("空间列请使用原生 SQL 导出导入，当前分批模式不转换空间类型".into());
        }
        if options.write_mode == WriteMode::Replace {
            let literal = crate::database::workbench::literal_for(DatabaseKind::MySql, table);
            if query(target,&format!("SELECT COUNT(*) FROM information_schema.KEY_COLUMN_USAGE WHERE REFERENCED_TABLE_SCHEMA=DATABASE() AND BINARY REFERENCED_TABLE_NAME=BINARY {literal};")).map_err(|e| format!("无法核对目标外键引用：{e}"))?.trim()!="0" {
                return Err("所选目标表被外键引用，不能安全 TRUNCATE；请选择追加或更新插入".into());
            }
        }
        let definition = query(
            target,
            &format!("SHOW CREATE TABLE {};", mysql_quote(table)),
        )
        .map_err(|e| format!("无法读取目标建表定义：{e}"))?;
        let suffix = if options.write_mode == WriteMode::Upsert {
            // MySQL resolves every unique index, not just the primary key. Refuse ambiguous targets.
            let literal = crate::database::workbench::literal_for(DatabaseKind::MySql, table);
            if query(target,&format!("SELECT COUNT(*) FROM information_schema.STATISTICS WHERE TABLE_SCHEMA=DATABASE() AND BINARY TABLE_NAME=BINARY {literal} AND NON_UNIQUE=0 AND INDEX_NAME<>'PRIMARY';")).map_err(|e| format!("无法核对目标唯一索引：{e}"))?.trim()!="0" {
                return Err("目标含主键以外的唯一索引，无法保证仅按主键更新，请使用追加或调整目标".into());
            }
            suffix(&columns, true)?
        } else {
            String::new()
        };
        plans.push((table, columns, suffix, definition));
    }
    if options.write_mode == WriteMode::Replace {
        for table in &tables {
            writeln!(out, "TRUNCATE TABLE {};", mysql_quote(table)).map_err(io_error)?;
        }
    }
    let mut progress = Progress {
        unit: Unit::Tables,
        total: Some(tables.len() as u64),
        ..Default::default()
    };
    for (table, columns, suffix, _) in &plans {
        // The server emits ASCII SQL literals; binary/text are hex encoded without lossy escaping.
        let values = columns
            .iter()
            .map(|col| {
                let name = mysql_quote(&col.name);
                let kind = col
                    .kind
                    .split(['(', ' '])
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase();
                let numeric = matches!(
                    kind.as_str(),
                    "tinyint"
                        | "smallint"
                        | "mediumint"
                        | "int"
                        | "bigint"
                        | "decimal"
                        | "float"
                        | "double"
                        | "year"
                );
                if numeric {
                    format!("IF({name} IS NULL,'NULL',CAST({name} AS CHAR))")
                } else if matches!(kind.as_str(),"binary"|"varbinary"|"tinyblob"|"blob"|"mediumblob"|"longblob"|"bit") {
                    format!("IF({name} IS NULL,'NULL',CONCAT('X',CHAR(39),HEX({name}),CHAR(39)))")
                } else {
                    format!("IF({name} IS NULL,'NULL',CONCAT('CONVERT(X',CHAR(39),HEX(CONVERT({name} USING utf8mb4)),CHAR(39),' USING utf8mb4)'))")
                }
            })
            .collect::<Vec<_>>()
            .join(",',',");
        let sql = format!(
            "SELECT CONCAT('(',{values},')') FROM {};",
            mysql_quote(table)
        );
        let rows = tempfile::NamedTempFile::new().map_err(io_error)?;
        let mut cmd = mysql_command(source, "mysql")?;
        cmd.args([
            "--batch",
            "--raw",
            "--quick",
            "--skip-column-names",
            "--binary-mode",
            "--local-infile=0",
        ])
        .arg(format!("--database={}", source.profile.database))
        .stdin(Stdio::piped())
        .stdout(rows.reopen().map_err(io_error)?)
        .stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(io_error)?;
        if let Err(error) = child
            .stdin
            .take()
            .ok_or("客户端输入不可用")?
            .write_all(sql.as_bytes())
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io_error(error));
        }
        wait_child(child, c, false, || notify(tx, progress.clone()))?;
        let fields = columns
            .iter()
            .map(|c| mysql_quote(&c.name))
            .collect::<Vec<_>>()
            .join(",");
        let prefix = format!("INSERT INTO {} ({fields}) VALUES ", mysql_quote(table));
        let mut reader = BufReader::new(rows.reopen().map_err(io_error)?);
        let mut batch = String::new();
        let mut count = 0;
        loop {
            c.check()?;
            let mut line = String::new();
            let n = reader
                .by_ref()
                .take(8 * 1024 * 1024 + 1)
                .read_line(&mut line)
                .map_err(io_error)?;
            if n == 0 {
                break;
            }
            if n > 8 * 1024 * 1024 {
                return Err("单行编码超过 8 MiB，请使用原生完整导出".into());
            }
            if count > 0
                && (count >= options.batch_size || batch.len() + line.len() > 4 * 1024 * 1024)
            {
                writeln!(out, "{prefix}{batch}{suffix};").map_err(io_error)?;
                batch.clear();
                count = 0;
            }
            if count > 0 {
                batch.push(',');
            }
            batch.push_str(line.trim_end());
            count += 1;
            progress.rows += 1;
        }
        if count > 0 {
            writeln!(out, "{prefix}{batch}{suffix};").map_err(io_error)?;
        }
        progress.completed += 1;
        notify(tx, progress.clone());
    }
    out.flush().map_err(io_error)?;
    // Recheck all target definitions after reading source data, before the first write.
    for (table, columns, _, definition) in &plans {
        if *columns != mysql_columns(target, table)?
            || *definition
                != query(
                    target,
                    &format!("SHOW CREATE TABLE {};", mysql_quote(table)),
                )?
        {
            return Err("目标表结构已变化，传输未执行".into());
        }
    }
    c.check()?;
    mysql_import(target, file.path(), c, tx)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sqlite_object_exclusions_preserve_other_definitions() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE items(id INTEGER); CREATE VIEW kept AS SELECT * FROM items; CREATE VIEW omitted AS SELECT * FROM items; CREATE TRIGGER omitted_trigger AFTER INSERT ON items BEGIN SELECT 1; END;").unwrap();
        let options = Options {
            excluded_objects: vec![
                ("view".into(), "omitted".into()),
                ("trigger".into(), "omitted_trigger".into()),
            ],
            ..Default::default()
        };
        assert!(!options.full_sqlite());
        let objects = sqlite_objects(&db, &options).unwrap();
        assert!(objects.iter().any(|(_, name, _)| name == "items"));
        assert!(objects.iter().any(|(_, name, _)| name == "kept"));
        assert!(
            !objects
                .iter()
                .any(|(_, name, _)| name.starts_with("omitted"))
        );
    }
    #[test]
    fn native_dump_batching_preserves_quoted_identifiers_and_other_statements() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dump.sql");
        let prefix = "INSERT INTO `odd VALUES ``name` (`id`,`text`) VALUES ";
        let input = format!(
            "-- prelude\n{prefix}(1,'first');\n{prefix}(2,'second');\n{prefix}(3,'third');\nDELIMITER ;;\nSELECT 1;\n"
        );
        std::fs::write(&path, input).unwrap();
        rebatch_mysql(&path, 2, &Control::default()).unwrap();
        let output = std::fs::read_to_string(&path).unwrap();
        assert!(
            output
                == format!(
                    "-- prelude\n{prefix}(1,'first'),(2,'second');\n{prefix}(3,'third');\nDELIMITER ;;\nSELECT 1;\n"
                )
        );
    }
    fn endpoint(path: &Path) -> Endpoint {
        Endpoint {
            profile: Profile {
                kind: DatabaseKind::Sqlite,
                sqlite_path: path.display().to_string(),
                sqlite_read_only: false,
                ..Default::default()
            },
            password: String::new(),
        }
    }
    #[test]
    fn selected_sqlite_export_and_grid_preserve_scope_and_cells() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.db");
        let db = rusqlite::Connection::open(&source).unwrap();
        db.execute_batch("CREATE TABLE selected(id INTEGER PRIMARY KEY,v TEXT);CREATE TABLE untouched(id INTEGER);CREATE VIEW shown AS SELECT * FROM selected;CREATE TRIGGER mark AFTER INSERT ON selected BEGIN INSERT INTO untouched VALUES(new.id);END;").unwrap();
        db.execute("INSERT INTO selected VALUES(1,?1)", ["a\tb\nc"])
            .unwrap();
        let preview = preview_grid(&endpoint(&source), Some("selected")).unwrap();
        assert!(preview.columns == ["id", "v"] && preview.rows[0][1] == "a\tb\nc");
        let path = dir.path().join("selection.sql");
        let (tx, _) = async_channel::bounded(16);
        let options = Options {
            tables: Some(vec!["selected".into()]),
            triggers: false,
            ..Default::default()
        };
        super::super::sqlite_export(
            &source,
            &path,
            Contents {
                schema: true,
                data: true,
            },
            &options,
            &Control::default(),
            &tx,
        )
        .unwrap();
        let restored = dir.path().join("restored.db");
        super::super::sqlite_import(&path, &restored, &Control::default(), &tx).unwrap();
        let db = rusqlite::Connection::open(restored).unwrap();
        let names = db
            .prepare("SELECT name FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(names == ["selected"]);
    }
    #[test]
    fn sqlite_modes_batch_and_rollback_only_selected_tables() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.db");
        let target = dir.path().join("target.db");
        for path in [&source, &target] {
            rusqlite::Connection::open(path).unwrap().execute_batch("CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT UNIQUE);CREATE TABLE keep(v TEXT);INSERT INTO keep VALUES('keep');").unwrap();
        }
        let from = rusqlite::Connection::open(&source).unwrap();
        let to = rusqlite::Connection::open(&target).unwrap();
        from.execute_batch("INSERT INTO items VALUES(1,'updated'),(2,'new');")
            .unwrap();
        to.execute_batch("INSERT INTO items VALUES(1,'old'),(3,'retained');")
            .unwrap();
        let (tx, _) = async_channel::bounded(16);
        let mut options = Options {
            tables: Some(vec!["items".into()]),
            batch_size: 1,
            write_mode: WriteMode::Append,
            ..Default::default()
        };
        assert!(
            import_data(
                &endpoint(&source),
                &endpoint(&target),
                &options,
                &Control::default(),
                &tx
            )
            .is_err()
        );
        options.write_mode = WriteMode::Upsert;
        import_data(
            &endpoint(&source),
            &endpoint(&target),
            &options,
            &Control::default(),
            &tx,
        )
        .unwrap();
        assert!(
            to.query_row("SELECT count(*) FROM items", [], |r| r.get::<_, i64>(0))
                .unwrap()
                == 3
        );
        assert!(
            to.query_row("SELECT v FROM items WHERE id=1", [], |r| r
                .get::<_, String>(0))
                .unwrap()
                == "updated"
        );
        options.write_mode = WriteMode::Replace;
        import_data(
            &endpoint(&source),
            &endpoint(&target),
            &options,
            &Control::default(),
            &tx,
        )
        .unwrap();
        assert!(
            to.query_row("SELECT count(*) FROM items", [], |r| r.get::<_, i64>(0))
                .unwrap()
                == 2
        );
        assert!(
            to.query_row("SELECT v FROM keep", [], |r| r.get::<_, String>(0))
                .unwrap()
                == "keep"
        );
        to.execute_batch(
            "CREATE TABLE child(id INTEGER REFERENCES items(id));INSERT INTO child VALUES(2);",
        )
        .unwrap();
        from.execute_batch("DELETE FROM items WHERE id=2;").unwrap();
        assert!(
            import_data(
                &endpoint(&source),
                &endpoint(&target),
                &options,
                &Control::default(),
                &tx
            )
            .is_err()
        );
        assert!(
            to.query_row("SELECT count(*) FROM items", [], |r| r.get::<_, i64>(0))
                .unwrap()
                == 2
        );
        let cancel = Control::default();
        cancel.cancel();
        assert!(
            import_data(
                &endpoint(&source),
                &endpoint(&target),
                &options,
                &cancel,
                &tx
            )
            .is_err()
        );
    }
    #[test]
    fn invalid_modes_and_missing_primary_keys_are_rejected() {
        let profile = Profile {
            kind: DatabaseKind::Sqlite,
            ..Default::default()
        };
        let options = Options {
            write_mode: WriteMode::Replace,
            ..Default::default()
        };
        assert!(
            options
                .validate(
                    Mode::ImportFile,
                    Contents {
                        schema: true,
                        data: true
                    },
                    &profile
                )
                .is_err()
        );
        assert!(
            options
                .validate(
                    Mode::ImportConnection,
                    Contents {
                        schema: true,
                        data: true
                    },
                    &profile
                )
                .is_err()
        );
        assert!(
            suffix(
                &[Column {
                    name: "v".into(),
                    kind: "text".into(),
                    pk: false
                }],
                false
            )
            .is_err()
        );
        let options = Options {
            tables: Some(Vec::new()),
            ..Default::default()
        };
        assert!(
            options
                .validate(
                    Mode::Export,
                    Contents {
                        schema: true,
                        data: true
                    },
                    &profile
                )
                .is_err()
        );
    }
}

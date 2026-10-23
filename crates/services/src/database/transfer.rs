#[path = "transfer_diagnostic.rs"]
mod diagnostic;
#[path = "export_text.rs"]
mod export_text;
#[path = "transfer_options.rs"]
mod options;
#[path = "pg_sql_guard.rs"]
mod pg_sql_guard;
pub use options::{ObjectChoice, Options, Preview, WriteMode, preview_grid};
// Disk-backed, cancellable database transfers. No query-preview row or byte limits.
use crate::{
    DatabaseKind,
    mysql::{Profile, TlsMode},
};
use std::{
    fs::File,
    io::{BufRead, BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    Export,
    Copy,
    ImportFile,
    ImportConnection,
}
#[derive(Clone, Copy)]
pub struct Contents {
    pub schema: bool,
    pub data: bool,
}
pub struct Endpoint {
    pub profile: Profile,
    pub password: String,
}
pub struct Job {
    pub mode: Mode,
    pub source: Endpoint,
    pub target: Option<Endpoint>,
    pub database: String,
    pub file: PathBuf,
    pub contents: Contents,
    pub options: Options,
}
#[derive(Clone, Copy, Default)]
pub enum Unit {
    #[default]
    Unknown,
    Bytes,
    Tables,
    Pages,
}
#[derive(Clone, Default)]
pub struct Progress {
    pub unit: Unit,
    pub bytes: u64,
    pub rows: u64,
    pub completed: u64,
    pub total: Option<u64>,
    pub importing: bool,
}
pub enum Event {
    Progress(Progress),
    Done(Result<(), String>),
}
#[derive(Default)]
pub struct Control {
    cancelled: AtomicBool,
    sqlite: std::sync::Mutex<Option<rusqlite::InterruptHandle>>,
}
impl Control {
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
        if let Some(db) = self.sqlite.lock().unwrap().as_ref() {
            db.interrupt();
        }
    }
    fn check(&self) -> Result<(), String> {
        if self.cancelled.load(Ordering::Relaxed) {
            Err("任务已取消；已提交的目标数据可能保留".into())
        } else {
            Ok(())
        }
    }
}
fn notify(tx: &async_channel::Sender<Event>, p: Progress) {
    let _ = tx.try_send(Event::Progress(p));
}
fn io_error(_: impl std::fmt::Display) -> String {
    "文件或客户端操作失败，请检查目标位置、权限及磁盘空间".into()
}
fn db_error(_: rusqlite::Error) -> String {
    "SQLite 操作失败，请检查目标是否存在同名对象、SQL 格式及约束；已提交的内容可能保留".into()
}
fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}
fn supported(p: &Profile) -> Result<(), String> {
    if p.mysql_raw_gbk {
        return Err("mysql.gbk.transfer_disabled".into());
    }
    if p.driver_id == "doris" {
        return Err("drivers.operation_unsupported".into());
    }
    if matches!(
        p.kind,
        DatabaseKind::MySql | DatabaseKind::PostgreSql | DatabaseKind::Sqlite
    ) {
        Ok(())
    } else {
        Err("当前任务支持 MySQL、PostgreSQL 与 SQLite；其他驱动暂不支持此传输流程".into())
    }
}

/// Validate the same transports used by execution, without changing server state.
pub fn database_names(endpoint: &Endpoint) -> Result<Vec<String>, String> {
    let mut endpoint = Endpoint {
        profile: endpoint.profile.clone(),
        password: endpoint.password.clone(),
    };
    endpoint.profile.list_all_databases = true;
    match service_operation(
        &endpoint,
        crate::database::OperationKind::Databases,
        &Control::default(),
    )? {
        crate::database::OperationResult::Databases(names) => Ok(names),
        _ => Err("无法读取目标数据库目录".into()),
    }
}
pub fn preflight(
    source: &Endpoint,
    target: Option<&Endpoint>,
    mode: Mode,
    new_database: &str,
    c: &Control,
) -> Result<String, String> {
    c.check()?;
    if source.profile.mysql_raw_gbk || target.is_some_and(|t| t.profile.mysql_raw_gbk) {
        return Err("mysql.gbk.transfer_disabled".into());
    }
    let legacy = crate::mysql::uses_legacy(&source.profile);
    let charset = if mode == Mode::ImportFile {
        String::new()
    } else if source.profile.kind == DatabaseKind::Sqlite {
        let db = sqlite_open(Path::new(&source.profile.sqlite_path), false, c)?;
        db.query_row("PRAGMA encoding", [], |r| r.get::<_, String>(0))
            .map_err(db_error)?
    } else if legacy {
        let result = service_operation(
            source,
            crate::database::OperationKind::Details {
                database: source.profile.database.clone(),
                object: crate::database::ObjectItem::new(
                    String::new(),
                    source.profile.database.clone(),
                    "database".into(),
                ),
            },
            c,
        )?;
        let charset = match result {
            crate::database::OperationResult::Details(values) => values
                .into_iter()
                .find(|(key, _)| key == "charset")
                .map(|(_, v)| v)
                .unwrap_or_default(),
            _ => String::new(),
        };
        if mode != Mode::Copy || target.is_none_or(|t| t.profile.id != source.profile.id) {
            mysql_command(source, "mysqldump")?;
            query(source, "SELECT 1;").map_err(|e| format!("来源执行客户端连接失败：{e}"))?;
        }
        charset
    } else {
        query(source, "SELECT 1;").map_err(|e| format!("来源执行客户端连接失败：{e}"))?;
        if source.profile.kind == DatabaseKind::MySql {
            let database = crate::database::workbench::literal_for(
                DatabaseKind::MySql,
                &source.profile.database,
            );
            query(source,&format!("SELECT DEFAULT_CHARACTER_SET_NAME FROM information_schema.SCHEMATA WHERE BINARY SCHEMA_NAME=BINARY {database};"))?.trim().into()
        } else {
            query(source, "SHOW server_encoding;")?.trim().into()
        }
    };
    if mode != Mode::ImportFile && !legacy && source.profile.kind != DatabaseKind::Sqlite {
        tool(
            &source.profile,
            if source.profile.kind == DatabaseKind::MySql {
                "mysqldump"
            } else {
                "pg_dump"
            },
        )?;
    }
    if let Some(target) = target {
        if source.profile.kind != target.profile.kind {
            return Err("仅支持相同数据库类型之间传输".into());
        }
        if mode == Mode::ImportConnection {
            let from = &source.profile;
            let to = &target.profile;
            let same = if from.kind == DatabaseKind::Sqlite {
                std::fs::canonicalize(&from.sqlite_path)
                    .ok()
                    .zip(std::fs::canonicalize(&to.sqlite_path).ok())
                    .is_some_and(|(a, b)| a == b)
            } else {
                from.host.eq_ignore_ascii_case(&to.host)
                    && from.port == to.port
                    && from.database == to.database
            };
            if same {
                return Err("来源与目标是同一个数据库，请选择其他目标库".into());
            }
        }
        if mode == Mode::Copy {
            if new_database.trim().is_empty() {
                return Err("请填写新数据库名称或 SQLite 新文件路径".into());
            }
            if target.profile.kind == DatabaseKind::Sqlite {
                if Path::new(new_database).exists() {
                    return Err("目标数据库文件已存在，请更换新库位置；不会覆盖".into());
                }
            } else {
                let names = if crate::mysql::uses_legacy(&target.profile) {
                    match service_operation(target, crate::database::OperationKind::Databases, c)? {
                        crate::database::OperationResult::Databases(names) => names,
                        _ => return Err("无法核对目标数据库列表".into()),
                    }
                } else {
                    query(
                        target,
                        if target.profile.kind == DatabaseKind::MySql {
                            "SHOW DATABASES;"
                        } else {
                            "SELECT datname FROM pg_database;"
                        },
                    )?
                    .lines()
                    .map(str::to_owned)
                    .collect()
                };
                if names
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(new_database))
                {
                    return Err("目标数据库已存在，请更换新库名称；不会覆盖或合并".into());
                }
            }
        } else if target.profile.kind == DatabaseKind::Sqlite {
            if target.profile.sqlite_read_only {
                return Err("目标连接为只读，不能导入".into());
            }
            sqlite_open(Path::new(&target.profile.sqlite_path), false, c)?;
        } else {
            query(target, "SELECT 1;").map_err(|e| format!("目标执行客户端连接失败：{e}"))?;
            if crate::mysql::uses_legacy(&target.profile) {
                return Err("旧版 MySQL 的文件/跨连接导入尚未接入安全执行通道；同连接复制至新库可使用兼容驱动".into());
            }
        }
    }
    Ok(charset)
}

pub fn validate_options(
    source: &Endpoint,
    target: Option<&Endpoint>,
    mode: Mode,
    contents: Contents,
    options: &Options,
    c: &Control,
) -> Result<(), String> {
    options.validate(mode, contents, &source.profile)?;
    if mode == Mode::ImportConnection && !contents.schema && contents.data {
        options::preflight_data(source, target.ok_or("请选择目标连接")?, options, c)?;
    }
    Ok(())
}

impl Job {
    pub fn run(
        mut self,
        control: Arc<Control>,
        tx: &async_channel::Sender<Event>,
    ) -> Result<(), String> {
        supported(&self.source.profile)?;
        self.options
            .validate(self.mode, self.contents, &self.source.profile)?;
        control.check()?;
        preflight(
            &self.source,
            self.target.as_ref(),
            self.mode,
            &self.database,
            &control,
        )?;
        if !self.contents.schema && !self.contents.data {
            return Err("请选择表结构或数据".into());
        }
        if self.mode == Mode::Export {
            if self.file.exists() {
                return Err("导出文件已存在，请选择新文件，避免覆盖".into());
            }
            let parent = self.file.parent().ok_or("请选择有效导出位置")?;
            let staged = tempfile::NamedTempFile::new_in(parent).map_err(io_error)?;
            if self.source.profile.kind == DatabaseKind::Sqlite
                && self
                    .file
                    .extension()
                    .is_some_and(|ext| !ext.eq_ignore_ascii_case("sql"))
            {
                if !self.contents.schema || !self.contents.data || !self.options.full_sqlite() {
                    return Err("SQLite 文件备份包含结构与数据，单独导出请选 .sql 文件".into());
                }
                sqlite_backup(
                    Path::new(&self.source.profile.sqlite_path),
                    staged.path(),
                    false,
                    &control,
                    tx,
                )?;
            } else {
                export(
                    &self.source,
                    staged.path(),
                    self.contents,
                    &self.options,
                    false,
                    &control,
                    tx,
                )?;
            }
            control.check()?;
            if self.source.profile.kind != DatabaseKind::Sqlite
                || self
                    .file
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("sql"))
            {
                let mut normalized = tempfile::NamedTempFile::new_in(parent).map_err(io_error)?;
                export_text::write_sql(
                    File::open(staged.path()).map_err(io_error)?,
                    BufWriter::new(normalized.as_file_mut()),
                    self.options.crlf,
                    self.source.profile.kind,
                    || control.check().map_err(std::io::Error::other),
                )
                .map_err(io_error)?;
                control.check()?;
                normalized.persist_noclobber(&self.file).map_err(io_error)?;
            } else {
                staged.persist_noclobber(&self.file).map_err(io_error)?;
            }
            return Ok(());
        }
        let target = self.target.as_mut().ok_or("请选择目标连接")?;
        supported(&target.profile)?;
        if target.profile.kind != self.source.profile.kind {
            return Err("仅支持相同数据库类型之间传输，不自动转换 SQL 方言".into());
        }
        if self.mode != Mode::Copy
            && target.profile.kind == DatabaseKind::Sqlite
            && target.profile.sqlite_read_only
        {
            return Err("目标连接为只读，不能导入".into());
        }
        if self.mode == Mode::ImportConnection {
            let from = &self.source.profile;
            let to = &target.profile;
            let same = if from.kind == DatabaseKind::Sqlite {
                std::fs::canonicalize(&from.sqlite_path)
                    .ok()
                    .zip(std::fs::canonicalize(&to.sqlite_path).ok())
                    .is_some_and(|(a, b)| a == b)
            } else {
                from.host.eq_ignore_ascii_case(&to.host)
                    && from.port == to.port
                    && from.database == to.database
            };
            if same {
                return Err("来源与目标是同一个数据库，已阻止重复导入".into());
            }
        }
        if self.mode == Mode::Copy && !self.contents.schema {
            return Err("复制到新库必须包含表结构".into());
        }
        if self.mode == Mode::Copy
            && self.source.profile.kind == DatabaseKind::MySql
            && crate::mysql::uses_legacy(&self.source.profile)
            && self.source.profile.id == target.profile.id
            && self.options.tables.is_none()
        {
            let kind = crate::database::OperationKind::CopyDatabase {
                source: self.source.profile.database.clone(),
                target: self.database.clone(),
                charset: String::new(),
                include_data: self.contents.data,
            };
            service_operation(&self.source, kind, &control)?;
            return Ok(());
        }
        if self.mode == Mode::ImportConnection && !self.contents.schema {
            return options::import_data(&self.source, target, &self.options, &control, tx);
        }
        let staged;
        let input = if self.mode == Mode::ImportFile {
            self.file.clone()
        } else {
            staged = tempfile::NamedTempFile::new().map_err(io_error)?;
            if self.source.profile.kind == DatabaseKind::Sqlite
                && self.contents.schema
                && self.contents.data
                && self.options.full_sqlite()
            {
                sqlite_backup(
                    Path::new(&self.source.profile.sqlite_path),
                    staged.path(),
                    false,
                    &control,
                    tx,
                )?;
            } else {
                export(
                    &self.source,
                    staged.path(),
                    self.contents,
                    &self.options,
                    false,
                    &control,
                    tx,
                )?;
            }
            staged.path().to_path_buf()
        };
        control.check()?;
        if self.mode == Mode::Copy {
            if self.database.trim().is_empty() {
                return Err("请填写新数据库名称或 SQLite 新文件路径".into());
            }
            if target.profile.kind == DatabaseKind::Sqlite {
                let path = PathBuf::from(&self.database);
                if !path.is_absolute() || path.exists() {
                    return Err("SQLite 目标须为不存在的新文件绝对路径".into());
                }
                // Publish a new file only when all statements have succeeded.
                let target_file =
                    tempfile::NamedTempFile::new_in(path.parent().ok_or("目标目录无效")?)
                        .map_err(io_error)?;
                restore_sqlite(&input, target_file.path(), &control, tx)?;
                control.check()?;
                target_file.persist_noclobber(path).map_err(io_error)?;
                return Ok(());
            }
            crate::database::validate_name(target.profile.kind, &self.database)?;
            if target.profile.kind == DatabaseKind::PostgreSql {
                let mut cmd = pg_command(target, "createdb")?;
                cmd.stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::piped());
                let settings = query(
                    &self.source,
                    "SELECT pg_encoding_to_char(encoding),datcollate,datctype FROM pg_database WHERE datname=current_database();",
                )?;
                let values = settings.trim().split('\t').collect::<Vec<_>>();
                if values.len() != 3 {
                    return Err("无法读取源库编码及区域设置".into());
                }
                cmd.arg(format!("--encoding={}", values[0]))
                    .arg(format!("--lc-collate={}", values[1]))
                    .arg(format!("--lc-ctype={}", values[2]))
                    .arg("--template=template0")
                    .arg("--")
                    .arg(&self.database);
                wait_child(cmd.spawn().map_err(io_error)?, &control, true, || {})?;
            } else {
                crate::database::validate_name(DatabaseKind::MySql, &self.database)?;
                let create = tempfile::NamedTempFile::new().map_err(io_error)?;
                let source_name = crate::database::workbench::literal_for(
                    DatabaseKind::MySql,
                    &self.source.profile.database,
                );
                let settings = query(
                    &self.source,
                    &format!(
                        "SELECT DEFAULT_CHARACTER_SET_NAME,DEFAULT_COLLATION_NAME FROM information_schema.SCHEMATA WHERE SCHEMA_NAME={source_name};"
                    ),
                )?;
                let values = settings.trim().split('\t').collect::<Vec<_>>();
                if values.len() != 2
                    || !values.iter().all(|v| {
                        !v.is_empty() && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                    })
                {
                    return Err("无法读取源库字符集与排序规则".into());
                }
                if let Some(collation) = &self.options.collation {
                    let name =
                        crate::database::workbench::literal_for(DatabaseKind::MySql, collation);
                    let support = query(
                        target,
                        &format!(
                            "SELECT CHARACTER_SET_NAME FROM information_schema.COLLATIONS WHERE COLLATION_NAME={name};"
                        ),
                    )?;
                    if support.trim() != values[0] {
                        return Err(
                            "目标服务器不支持所选排序规则，或与来源字符集不匹配；尚未创建目标库"
                                .into(),
                        );
                    }
                }
                std::fs::write(
                    create.path(),
                    format!(
                        "CREATE DATABASE `{}` CHARACTER SET `{}` COLLATE `{}`;",
                        self.database.replace('`', "``"),
                        values[0],
                        self.options.collation.as_deref().unwrap_or(values[1])
                    ),
                )
                .map_err(io_error)?;
                mysql_import(target, create.path(), &control, tx)?;
            }
            target.profile.database = self.database.clone();
        } else if self.mode == Mode::ImportConnection && self.contents.schema {
            // A generated schema must never replace existing target objects.
            if !inventory(target)?.is_empty() {
                return Err("导入结构需要空目标库；目标已有对象，可改选仅数据或复制到新库".into());
            }
        }
        if target.profile.kind == DatabaseKind::Sqlite {
            restore_sqlite(&input, Path::new(&target.profile.sqlite_path), &control, tx)
        } else if target.profile.kind == DatabaseKind::PostgreSql {
            pg_import(target, &input, &control, tx)
        } else {
            mysql_import(target, &input, &control, tx)
        }
    }
}

fn export(
    source: &Endpoint,
    path: &Path,
    contents: Contents,
    options: &Options,
    tables_only: bool,
    c: &Control,
    tx: &async_channel::Sender<Event>,
) -> Result<(), String> {
    if source.profile.kind == DatabaseKind::Sqlite {
        sqlite_export(
            Path::new(&source.profile.sqlite_path),
            path,
            contents,
            options,
            c,
            tx,
        )
    } else if source.profile.kind == DatabaseKind::PostgreSql {
        pg_export(source, path, contents, options, c, tx)
    } else {
        mysql_export(source, path, contents, options, tables_only, c, tx)
    }
}
fn sqlite_open(path: &Path, write: bool, c: &Control) -> Result<rusqlite::Connection, String> {
    let flags = if write {
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
    } else {
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
    };
    let db = rusqlite::Connection::open_with_flags(path, flags).map_err(db_error)?;
    db.busy_timeout(Duration::from_secs(5)).map_err(db_error)?;
    *c.sqlite.lock().unwrap() = Some(db.get_interrupt_handle());
    Ok(db)
}
fn sqlite_objects(db: &rusqlite::Connection) -> Result<Vec<(String, String, String)>, String> {
    let mut query=db.prepare("SELECT type,name,sql FROM sqlite_schema WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' ORDER BY CASE type WHEN 'table' THEN 0 WHEN 'index' THEN 1 WHEN 'view' THEN 2 ELSE 3 END,name").map_err(db_error)?;
    query
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)
}
fn write_value(out: &mut impl Write, value: rusqlite::types::ValueRef<'_>) -> std::io::Result<()> {
    use rusqlite::types::ValueRef;
    match value {
        ValueRef::Null => out.write_all(b"NULL"),
        ValueRef::Integer(n) => write!(out, "{n}"),
        ValueRef::Real(n) => {
            if n.is_finite() {
                write!(out, "{n:?}")
            } else {
                out.write_all(if n.is_sign_negative() {
                    b"-9e999"
                } else {
                    b"9e999"
                })
            }
        }
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => {
            let text = matches!(value, ValueRef::Text(_));
            out.write_all(if text { b"CAST(X'" } else { b"X'" })?;
            const HEX: &[u8] = b"0123456789abcdef";
            for part in bytes.chunks(8192) {
                let mut buffer = Vec::with_capacity(part.len() * 2);
                for b in part {
                    buffer.push(HEX[(b >> 4) as usize]);
                    buffer.push(HEX[(b & 15) as usize]);
                }
                out.write_all(&buffer)?;
            }
            out.write_all(if text { b"' AS TEXT)" } else { b"'" })
        }
    }
}
fn sqlite_export(
    source: &Path,
    path: &Path,
    contents: Contents,
    options: &Options,
    c: &Control,
    tx: &async_channel::Sender<Event>,
) -> Result<(), String> {
    let db = sqlite_open(source, false, c)?;
    db.execute_batch("BEGIN").map_err(db_error)?;
    let objects = options::sqlite_objects(&db, options)?;
    if objects
        .iter()
        .any(|(_, _, sql)| sql.to_ascii_uppercase().contains("CREATE VIRTUAL TABLE"))
    {
        return Err("SQL 导出暂不支持 SQLite 虚拟表，请使用完整 SQLite 文件备份".into());
    }
    let mut out = BufWriter::with_capacity(64 * 1024, File::create(path).map_err(io_error)?);
    out.write_all(b"PRAGMA foreign_keys=ON;\nBEGIN;\nPRAGMA defer_foreign_keys=ON;\n")
        .map_err(io_error)?;
    let tables = objects
        .iter()
        .filter(|(kind, _, _)| kind == "table")
        .collect::<Vec<_>>();
    if contents.schema {
        for (_, _, sql) in &tables {
            c.check()?;
            writeln!(out, "{sql};").map_err(io_error)?;
        }
    }
    let mut progress = Progress {
        total: Some(tables.len() as u64),
        unit: Unit::Tables,
        ..Default::default()
    };
    for (_, name, _) in &tables {
        c.check()?;
        if contents.data {
            let mut columns = db
                .prepare(&format!("PRAGMA table_xinfo({})", quote(name)))
                .map_err(db_error)?;
            let names = columns
                .query_map([], |row| {
                    Ok((row.get::<_, String>(1)?, row.get::<_, i64>(6)?))
                })
                .map_err(db_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db_error)?
                .into_iter()
                .filter(|(_, hidden)| *hidden == 0)
                .map(|(name, _)| quote(&name))
                .collect::<Vec<_>>();
            let fields = names.join(",");
            let mut query = db
                .prepare(&format!("SELECT {fields} FROM {}", quote(name)))
                .map_err(db_error)?;
            let mut rows = query.query([]).map_err(db_error)?;
            let mut batch_rows = 0;
            let mut batch_bytes = 0;
            while let Some(row) = rows.next().map_err(db_error)? {
                c.check()?;
                let estimated = (0..names.len())
                    .map(|i| {
                        row.get_ref(i).map(|v| match v {
                            rusqlite::types::ValueRef::Text(v)
                            | rusqlite::types::ValueRef::Blob(v) => v.len().saturating_mul(2) + 32,
                            _ => 32,
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(db_error)?
                    .into_iter()
                    .sum::<usize>();
                if batch_rows > 0
                    && (batch_rows >= options.batch_size
                        || batch_bytes + estimated > 4 * 1024 * 1024)
                {
                    out.write_all(b";\n").map_err(io_error)?;
                    batch_rows = 0;
                    batch_bytes = 0;
                }
                if batch_rows == 0 {
                    write!(out, "INSERT INTO {} ({fields}) VALUES ", quote(name))
                        .map_err(io_error)?;
                } else {
                    out.write_all(b",").map_err(io_error)?;
                }
                out.write_all(b"(").map_err(io_error)?;
                batch_rows += 1;
                batch_bytes += estimated;
                for i in 0..names.len() {
                    if i > 0 {
                        out.write_all(b",").map_err(io_error)?;
                    }
                    write_value(&mut out, row.get_ref(i).map_err(db_error)?).map_err(io_error)?;
                }
                out.write_all(b")").map_err(io_error)?;
                progress.rows += 1;
                if progress.rows.is_multiple_of(200) {
                    out.flush().map_err(io_error)?;
                    progress.bytes = out.get_ref().metadata().map_err(io_error)?.len();
                    notify(tx, progress.clone());
                }
            }
            if batch_rows > 0 {
                out.write_all(b";\n").map_err(io_error)?;
            }
        }
        progress.completed += 1;
        notify(tx, progress.clone());
    }
    if contents.schema {
        for (kind, _, sql) in &objects {
            if kind != "table" {
                c.check()?;
                writeln!(out, "{sql};").map_err(io_error)?;
            }
        }
    }
    out.write_all(b"COMMIT;\n").map_err(io_error)?;
    out.flush().map_err(io_error)?;
    c.check()
}
fn sqlite_import(
    input: &Path,
    target: &Path,
    c: &Control,
    tx: &async_channel::Sender<Event>,
) -> Result<(), String> {
    let db = sqlite_open(target, true, c)?;
    db.authorizer(Some(|context: rusqlite::hooks::AuthContext<'_>| {
        use rusqlite::hooks::{AuthAction, Authorization};
        match context.action {
            AuthAction::Attach { .. } | AuthAction::Detach { .. } => Authorization::Deny,
            AuthAction::Function { function_name }
                if function_name.eq_ignore_ascii_case("load_extension") =>
            {
                Authorization::Deny
            }
            AuthAction::Pragma { pragma_name, .. }
                if [
                    "writable_schema",
                    "schema_version",
                    "temp_store_directory",
                    "data_store_directory",
                ]
                .iter()
                .any(|name| pragma_name.eq_ignore_ascii_case(name)) =>
            {
                Authorization::Deny
            }
            _ => Authorization::Allow,
        }
    }));
    let file = File::open(input).map_err(io_error)?;
    let total = file.metadata().map_err(io_error)?.len();
    let mut reader = BufReader::new(file);
    let mut statement = Vec::new();
    let mut read = 0;
    let mut last_progress = 0;
    loop {
        c.check()?;
        let n = reader
            .by_ref()
            .take((8 * 1024 * 1024 + 1 - statement.len()) as u64)
            .read_until(b'\n', &mut statement)
            .map_err(io_error)?;
        read += n as u64;
        if statement.len() > 8 * 1024 * 1024 {
            return Err("单条 SQLite SQL 超过 8 MiB，请使用数据库文件备份传输超大值".into());
        }
        if n == 0 {
            if !statement.iter().all(u8::is_ascii_whitespace) {
                db.execute_batch(std::str::from_utf8(&statement).map_err(io_error)?)
                    .map_err(db_error)?;
            }
            break;
        }
        let sql = std::ffi::CString::new(statement.as_slice()).map_err(io_error)?;
        if unsafe { rusqlite::ffi::sqlite3_complete(sql.as_ptr()) } != 0 {
            db.execute_batch(std::str::from_utf8(&statement).map_err(io_error)?)
                .map_err(db_error)?;
            statement.clear();
            if read - last_progress >= 65536 || read == total {
                notify(
                    tx,
                    Progress {
                        bytes: read,
                        completed: read,
                        total: Some(total),
                        importing: true,
                        unit: Unit::Bytes,
                        ..Default::default()
                    },
                );
                last_progress = read;
            }
        }
    }
    if !db.is_autocommit() {
        return Err("SQL 文件结束时仍有未提交事务，已回滚".into());
    }
    c.check()
}

fn tool(p: &Profile, name: &str) -> Result<PathBuf, String> {
    let mut dirs = Vec::new();
    if let Some(parent) = Path::new(&p.legacy_library).parent() {
        dirs.push(parent.to_path_buf());
        if let Some(root) = parent.parent() {
            dirs.push(root.join("bin"));
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&paths));
    }
    for dir in dirs {
        let path = dir.join(if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.into()
        });
        if path.is_file() {
            if crate::mysql::uses_legacy(p) {
                let output = command(&path)
                    .args(["--no-defaults", "--version"])
                    .output()
                    .map_err(io_error)?;
                let version = String::from_utf8_lossy(&output.stdout);
                let v51 = version.contains("Distrib 5.1");
                if !(v51 || version.contains("Distrib 5.6")) || (v51 && !p.allow_old_auth) {
                    continue;
                }
            }
            return Ok(path);
        }
    }
    Err(if crate::mysql::uses_legacy(p) {
        format!(
            "未找到匹配的 {name} 客户端；旧版 MySQL 需兼容 5.1/5.6 客户端；5.1 仅用于已明确允许旧认证的连接，不能使用现代客户端代替。请安装到所选兼容驱动旁或既有 PATH"
        )
    } else {
        format!("未找到 {name} 客户端，请安装对应数据库客户端工具")
    })
}
fn command(path: &Path) -> Command {
    let cmd = Command::new(path);
    #[cfg(windows)]
    let cmd = {
        use std::os::windows::process::CommandExt;
        let mut cmd = cmd;
        cmd.creation_flags(0x08000000);
        cmd
    };
    cmd
}
struct NativeCommand {
    command: Command,
    _credentials: tempfile::NamedTempFile,
}
impl std::ops::Deref for NativeCommand {
    type Target = Command;
    fn deref(&self) -> &Command {
        &self.command
    }
}
impl std::ops::DerefMut for NativeCommand {
    fn deref_mut(&mut self) -> &mut Command {
        &mut self.command
    }
}
fn credential_file(password: &str) -> Result<tempfile::NamedTempFile, String> {
    if password.contains('\0') {
        return Err("客户端不支持含零字符的密码".into());
    }
    let mut file = tempfile::Builder::new()
        .prefix("turbodbnote-client-")
        .suffix(".cnf")
        .tempfile()
        .map_err(io_error)?;
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::{
            Foundation::LocalFree,
            Security::{
                Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW,
                DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, SetFileSecurityW,
            },
        };
        let acl = "D:P(A;;FA;;;OW)(A;;FA;;;SY)"
            .encode_utf16()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let path = file
            .path()
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let mut descriptor = std::ptr::null_mut();
        unsafe {
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                acl.as_ptr(),
                1,
                &mut descriptor,
                std::ptr::null_mut(),
            ) == 0
            {
                return Err("无法保护临时客户端凭据文件".into());
            }
            let protected = SetFileSecurityW(
                path.as_ptr(),
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                descriptor,
            ) != 0;
            LocalFree(descriptor);
            if !protected {
                return Err("无法限制临时客户端凭据文件权限".into());
            }
        }
    }
    let escaped = password
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t");
    write!(file, "[client]\npassword=\"{escaped}\"\n").map_err(io_error)?;
    file.flush().map_err(io_error)?;
    Ok(file)
}
fn mysql_command(endpoint: &Endpoint, name: &str) -> Result<NativeCommand, String> {
    let path = tool(&endpoint.profile, name)?;
    let version = command(&path)
        .args(["--no-defaults", "--version"])
        .output()
        .map_err(io_error)?;
    let version = String::from_utf8_lossy(&version.stdout);
    let old = version.contains("Distrib 5.1") || version.contains("Distrib 5.6");
    let p = &endpoint.profile;
    let mut cmd = command(&path);
    let credentials = credential_file(&endpoint.password)?;
    cmd.arg(format!("--defaults-file={}", credentials.path().display()));
    // MySQL 8.0 still reads .mylogin.cnf with --no-defaults. Isolate that
    // documented client setting so unrelated saved credentials cannot win.
    cmd.env(
        "MYSQL_TEST_LOGIN_FILE",
        std::env::temp_dir().join(format!("turbodbnote-unused-login-{}", uuid::Uuid::new_v4())),
    );
    cmd.arg("--protocol=TCP")
        .arg(format!("--host={}", p.host))
        .arg(format!("--port={}", p.port))
        .arg(format!("--user={}", p.user));
    if name == "mysql" {
        cmd.arg(format!("--connect-timeout={}", p.timeout_seconds));
    }
    cmd.env_remove("MYSQL_PWD");
    if old {
        let secure_auth = if name == "mysql" {
            true
        } else if version.contains("Distrib 5.1") {
            false
        } else {
            let help = command(&path)
                .args(["--no-defaults", "--help"])
                .output()
                .map_err(io_error)?;
            String::from_utf8_lossy(&help.stdout).contains("secure-auth")
        };
        if secure_auth {
            cmd.arg(if p.allow_old_auth {
                "--skip-secure-auth"
            } else {
                "--secure-auth"
            });
        } else if !p.allow_old_auth {
            return Err("所选客户端不能保证禁用旧认证，请选择支持 secure-auth 的兼容客户端".into());
        }
        match p.tls {
            TlsMode::Disabled => {
                cmd.arg("--skip-ssl");
            }
            TlsMode::Required => {
                cmd.arg("--ssl");
            }
            TlsMode::VerifyIdentity => {
                cmd.args(["--ssl", "--ssl-verify-server-cert"]);
            }
        }
    } else {
        cmd.arg(match p.tls {
            TlsMode::Disabled => "--ssl-mode=DISABLED",
            TlsMode::Required => "--ssl-mode=REQUIRED",
            TlsMode::VerifyIdentity => "--ssl-mode=VERIFY_IDENTITY",
        });
    }
    if p.charset != "server-default" {
        cmd.arg(format!("--default-character-set={}", p.charset));
    }
    for (key, value) in [
        ("ssl-ca", &p.ca),
        ("ssl-cert", &p.client_cert),
        ("ssl-key", &p.client_key),
    ] {
        if !value.is_empty() {
            cmd.arg(format!("--{key}={value}"));
        }
    }
    Ok(NativeCommand {
        command: cmd,
        _credentials: credentials,
    })
}
fn wait_child(
    mut child: std::process::Child,
    c: &Control,
    writing: bool,
    mut progress: impl FnMut(),
) -> Result<(), String> {
    let error = child.stderr.take();
    let drain =
        std::thread::spawn(move || error.map(diagnostic::Diagnostic::read).unwrap_or_default());
    let mut exit = None;
    let result = loop {
        if c.cancelled.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            break c.check();
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                break if status.success() {
                    Ok(())
                } else {
                    exit = Some(status.code().unwrap_or(-1));
                    Err(String::new())
                };
            }
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(io_error(error));
            }
        }
        progress();
        std::thread::sleep(Duration::from_millis(150));
    };
    let diagnostic = drain.join().unwrap_or_default();
    if let Some(code) = exit {
        Err(diagnostic.failure(code, writing))
    } else {
        result
    }
}
fn dump_compatibility_args(help: &str) -> Vec<&'static str> {
    [
        ("column-statistics", "--column-statistics=0"),
        ("set-gtid-purged", "--set-gtid-purged=OFF"),
        ("no-tablespaces", "--no-tablespaces"),
    ]
    .into_iter()
    .filter_map(|(name, arg)| help.contains(name).then_some(arg))
    .collect()
}

fn mysql_export(
    source: &Endpoint,
    path: &Path,
    contents: Contents,
    options: &Options,
    tables_only: bool,
    c: &Control,
    tx: &async_channel::Sender<Event>,
) -> Result<(), String> {
    let mut cmd = mysql_command(source, "mysqldump")?;
    cmd.args([
        "--quick",
        "--single-transaction",
        "--hex-blob",
        "--skip-add-drop-table",
        "--skip-extended-insert",
        "--complete-insert",
    ]);
    let version = command(&tool(&source.profile, "mysqldump")?)
        .args(["--no-defaults", "--version"])
        .output()
        .map_err(io_error)?;
    let version = String::from_utf8_lossy(&version.stdout);
    // Probe the selected executable's supported options, not a substring of its version.
    let help = command(&tool(&source.profile, "mysqldump")?)
        .args(["--no-defaults", "--help"])
        .output()
        .map_err(io_error)?;
    if !help.status.success() {
        return Err("无法检测导出客户端支持的选项".into());
    }
    let help = String::from_utf8_lossy(&help.stdout);
    cmd.args(dump_compatibility_args(&help));
    cmd.arg("--skip-lock-tables");
    for (kind, name) in &options.excluded_objects {
        if kind == "view" {
            cmd.arg(format!(
                "--ignore-table={}.{}",
                source.profile.database, name
            ));
        } else {
            return Err("当前 MySQL 客户端仅支持按类型筛选触发器/例程/事件".into());
        }
    }
    if tables_only || !options.views {
        if !options.triggers {
            cmd.arg("--skip-triggers");
        }
        for line in inventory(source)?.lines() {
            if let Some((name, kind)) = line.split_once('\t')
                && kind == "VIEW"
            {
                cmd.arg(format!(
                    "--ignore-table={}.{}",
                    source.profile.database, name
                ));
            }
        }
    }
    if !options.triggers {
        cmd.arg("--skip-triggers");
    }
    if contents.schema
        && !version.contains("Distrib 5.1")
        && !source.profile.target_version.starts_with("4.")
    {
        if options.routines {
            cmd.arg("--routines");
        }
        if options.events {
            cmd.arg("--events");
        }
    }
    if !version.contains("Distrib 5.1") && !crate::mysql::uses_legacy(&source.profile) {
        cmd.arg("--default-character-set=utf8mb4");
    }
    if !contents.data {
        cmd.arg("--no-data");
    }
    if !contents.schema {
        cmd.args(["--no-create-info", "--skip-triggers"]);
    }
    if options.tables.as_ref().is_some_and(Vec::is_empty) {
        return Err("请选择至少一个表".into());
    }
    cmd.arg("--")
        .arg(&source.profile.database)
        .stdin(Stdio::null())
        .stdout(File::create(path).map_err(io_error)?)
        .stderr(Stdio::piped());
    if let Some(tables) = &options.tables {
        cmd.args(tables);
    }
    let child = cmd.spawn().map_err(io_error)?;
    let result = wait_child(child, c, false, || {
        notify(
            tx,
            Progress {
                bytes: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
                ..Default::default()
            },
        )
    });
    if result.is_ok() {
        options::rebatch_mysql(path, options.batch_size, c)?;
        notify(
            tx,
            Progress {
                bytes: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
                ..Default::default()
            },
        );
    }
    result
}
fn mysql_import(
    target: &Endpoint,
    path: &Path,
    c: &Control,
    tx: &async_channel::Sender<Event>,
) -> Result<(), String> {
    let mut cmd = mysql_command(target, "mysql")?;
    cmd.args(["--binary-mode", "--local-infile=0"])
        .arg(format!("--database={}", target.profile.database))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    feed_command(cmd, path, c, tx)
}
fn feed_command(
    mut cmd: NativeCommand,
    path: &Path,
    c: &Control,
    tx: &async_channel::Sender<Event>,
) -> Result<(), String> {
    let total = std::fs::metadata(path).map_err(io_error)?.len();
    let mut file = File::open(path).map_err(io_error)?;
    let mut child = cmd.spawn().map_err(io_error)?;
    let mut stdin = child.stdin.take().ok_or("无法打开客户端输入")?;
    let bytes = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let sent = bytes.clone();
    let writer = std::thread::spawn(move || -> std::io::Result<()> {
        let mut buffer = [0; 64 * 1024];
        loop {
            let n = file.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            stdin.write_all(&buffer[..n])?;
            sent.fetch_add(n as u64, Ordering::Relaxed);
        }
        Ok(())
    });
    let result = wait_child(child, c, true, || {
        let n = bytes.load(Ordering::Relaxed);
        notify(
            tx,
            Progress {
                bytes: n,
                completed: n,
                total: Some(total),
                importing: true,
                unit: Unit::Bytes,
                ..Default::default()
            },
        );
    });
    let wrote = writer
        .join()
        .map_err(|_| "客户端输入线程异常")?
        .map_err(io_error);
    let result = result.and(wrote);
    if result.is_ok() {
        let n = bytes.load(Ordering::Relaxed);
        notify(
            tx,
            Progress {
                bytes: n,
                completed: n,
                total: Some(total),
                importing: true,
                unit: Unit::Bytes,
                ..Default::default()
            },
        );
    }
    result
}
fn query(endpoint: &Endpoint, sql: &str) -> Result<String, String> {
    query_inner(endpoint, sql, false)
}
fn query_inner(endpoint: &Endpoint, sql: &str, sample: bool) -> Result<String, String> {
    let mut cmd = if endpoint.profile.kind == DatabaseKind::PostgreSql {
        let mut cmd = pg_command(endpoint, "psql")?;
        cmd.args([
            "--no-psqlrc",
            "--tuples-only",
            "--no-align",
            "--field-separator=\t",
            "--set=ON_ERROR_STOP=1",
        ]);
        cmd
    } else {
        let mut cmd = mysql_command(endpoint, "mysql")?;
        cmd.args(["--batch", "--skip-column-names", "--local-infile=0"])
            .arg(format!("--database={}", endpoint.profile.database));
        if crate::mysql::uses_legacy(&endpoint.profile) {
            if sql.contains('\\') {
                return Err("兼容客户端元数据查询不支持反斜杠标识符".into());
            }
        } else {
            cmd.arg("--binary-mode");
        }
        cmd
    };
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(io_error)?;
    child
        .stdin
        .take()
        .ok_or("客户端输入不可用")?
        .write_all(sql.as_bytes())
        .map_err(io_error)?;
    let stderr = child.stderr.take().ok_or("客户端诊断通道不可用")?;
    let diagnostic = std::thread::spawn(move || diagnostic::Diagnostic::read(stderr));
    let stdout = child.stdout.take().ok_or("客户端输出不可用")?;
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout
            .take(64 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = sender.send(result);
    });
    let deadline = std::time::Instant::now()
        + Duration::from_secs(endpoint.profile.timeout_seconds.clamp(1, 60));
    let mut output = None;
    let result = loop {
        if let Ok(read) = receiver.try_recv() {
            output = Some(read.map_err(io_error));
        }
        if output
            .as_ref()
            .is_some_and(|v| v.as_ref().is_ok_and(|b| b.len() > 64 * 1024))
        {
            let _ = child.kill();
            let _ = child.wait();
            if sample {
                let bytes = output.take().unwrap()?;
                break Ok(format!(
                    "{}\n[预览已截断]",
                    String::from_utf8_lossy(&bytes[..64 * 1024])
                ));
            }
            break Err(
                "对象目录超过 64 KiB，当前连接间传输无法完整核对对象；可改用文件导出".into(),
            );
        }
        if let Some(status) = child.try_wait().map_err(io_error)? {
            if !status.success() {
                break Err("无法读取来源预览，请检查客户端、连接及权限".into());
            }
            let bytes = match output {
                Some(v) => v,
                None => receiver
                    .recv_timeout(Duration::from_secs(1))
                    .map_err(io_error)?
                    .map_err(io_error),
            }?;
            break Ok(String::from_utf8_lossy(&bytes).into_owned());
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            break Err("来源预览超时，客户端已关闭".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let _ = reader.join();
    let diagnostic = diagnostic.join().unwrap_or_default();
    result.map_err(|error| diagnostic.detail().unwrap_or(error))
}
/// Read catalog metadata without sampling table rows or requiring native dump tools.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReadPermission {
    Available,
    Denied,
    #[default]
    Unknown,
}
#[derive(Clone, Default)]
pub struct ReadPermissions(pub std::collections::HashMap<String, ReadPermission>);
impl ReadPermissions {
    pub fn denied(&self, kind: &str) -> bool {
        self.0.get(kind) == Some(&ReadPermission::Denied)
    }
    pub fn blocks(&self, options: &Options) -> bool {
        [
            ("view", options.views),
            ("trigger", options.triggers),
            ("routine", options.routines),
            ("event", options.events),
        ]
        .into_iter()
        .any(|(kind, selected)| selected && self.denied(kind))
    }
}
fn permission_result(result: Result<String, String>) -> ReadPermission {
    match result {
        Ok(text) if !text.split('\t').any(|v| v.trim() == "NULL") => ReadPermission::Available,
        Err(error)
            if ["错误码 1044", "错误码 1142", "错误码 1227"]
                .iter()
                .any(|code| error.contains(code)) =>
        {
            ReadPermission::Denied
        }
        _ => ReadPermission::Unknown,
    }
}
/// Bounded read-only checks through the CLI channel used by transfers; never emit definition bodies.
pub fn read_permissions(
    endpoint: &Endpoint,
    preview: &Preview,
    control: &Control,
) -> ReadPermissions {
    let result = ReadPermissions::default();
    if endpoint.profile.kind != DatabaseKind::MySql || crate::mysql::uses_legacy(&endpoint.profile)
    {
        return result;
    }
    let db = crate::database::workbench::quote(DatabaseKind::MySql, &endpoint.profile.database);
    let old_dump = tool(&endpoint.profile, "mysqldump")
        .ok()
        .and_then(|path| {
            command(&path)
                .args(["--no-defaults", "--version"])
                .output()
                .ok()
        })
        .is_some_and(|output| {
            let version = String::from_utf8_lossy(&output.stdout);
            ["Distrib 5.1", "Distrib 5.5", "Distrib 5.6", "Distrib 5.7"]
                .iter()
                .any(|prefix| version.contains(prefix))
        });
    inspect_permissions(&db, preview, control, old_dump, |sql| query(endpoint, sql))
}
fn inspect_permissions(
    db: &str,
    preview: &Preview,
    control: &Control,
    old_dump: bool,
    mut read: impl FnMut(&str) -> Result<String, String>,
) -> ReadPermissions {
    let mut result = ReadPermissions::default();
    for kind in ["view", "trigger", "routine", "event"] {
        if control.is_cancelled() {
            break;
        }
        let objects = preview
            .objects
            .iter()
            .filter(|o| {
                o.kind == kind
                    || kind == "routine" && matches!(o.kind.as_str(), "procedure" | "function")
            })
            .collect::<Vec<_>>();
        let mut state = ReadPermission::Unknown;
        if kind == "event" {
            state = permission_result(read(&format!("SHOW EVENTS FROM {db};")));
        }
        // Older dump clients read mysql.proc even when information_schema hides routine definitions.
        if kind == "routine" && old_dump {
            state = permission_result(read("SELECT 1 FROM mysql.proc LIMIT 0;"));
        }
        if state != ReadPermission::Denied {
            let mut available = !objects.is_empty() && objects.len() <= 4;
            for object in objects.iter().take(4) {
                if control.is_cancelled() {
                    available = false;
                    break;
                }
                let keyword = match object.kind.as_str() {
                    "view" => "VIEW",
                    "trigger" => "TRIGGER",
                    "event" => "EVENT",
                    "function" => "FUNCTION",
                    _ => "PROCEDURE",
                };
                let name = crate::database::workbench::quote(DatabaseKind::MySql, &object.name);
                match permission_result(read(&format!("SHOW CREATE {keyword} {db}.{name};"))) {
                    ReadPermission::Denied => {
                        state = ReadPermission::Denied;
                        break;
                    }
                    ReadPermission::Unknown => available = false,
                    ReadPermission::Available => {}
                }
            }
            if state != ReadPermission::Denied {
                state = if available {
                    ReadPermission::Available
                } else {
                    ReadPermission::Unknown
                };
            }
        }
        result.0.insert(kind.into(), state);
    }
    result
}

pub fn catalog(endpoint: &Endpoint, control: &Control) -> Result<(Preview, String), String> {
    use crate::database::{
        OperationKind, OperationResult,
        workbench::{Action, Reply, Request},
    };
    let reply = service_operation(
        endpoint,
        OperationKind::Workbench(Box::new(Request {
            database: endpoint.profile.database.clone(),
            action: Action::Catalog,
        })),
        control,
    )?;
    let OperationResult::Workbench(reply) = reply else {
        return Err("无法读取库对象目录".into());
    };
    let Reply::Catalog { objects, notice } = *reply else {
        return Err("无法读取库对象目录".into());
    };
    Ok((
        Preview {
            objects: objects
                .into_iter()
                .take(2000)
                .map(|o| ObjectChoice {
                    name: if endpoint.profile.kind == DatabaseKind::PostgreSql
                        && !o.schema.is_empty()
                    {
                        format!(
                            "{}.{}",
                            crate::database::workbench::quote(DatabaseKind::PostgreSql, &o.schema),
                            crate::database::workbench::quote(DatabaseKind::PostgreSql, &o.name)
                        )
                    } else {
                        o.name
                    },
                    kind: o.kind,
                })
                .collect(),
            ..Default::default()
        },
        notice,
    ))
}

pub fn inventory(endpoint: &Endpoint) -> Result<String, String> {
    if endpoint.profile.kind == DatabaseKind::Sqlite {
        let c = Control::default();
        let db = sqlite_open(Path::new(&endpoint.profile.sqlite_path), false, &c)?;
        Ok(sqlite_objects(&db)?
            .iter()
            .map(|(kind, name, _)| format!("{kind}\t{name}"))
            .collect::<Vec<_>>()
            .join("\n"))
    } else if endpoint.profile.kind == DatabaseKind::PostgreSql {
        query(
            endpoint,
            "SELECT quote_ident(table_schema)||'.'||quote_ident(table_name),table_type FROM information_schema.tables WHERE table_schema NOT IN ('pg_catalog','information_schema') ORDER BY table_schema,table_name;",
        )
    } else if crate::mysql::uses_legacy(&endpoint.profile)
        && endpoint.profile.target_version.starts_with("4.")
    {
        Ok(query(endpoint, "SHOW TABLES;")?
            .lines()
            .map(|name| format!("{name}\tBASE TABLE"))
            .collect::<Vec<_>>()
            .join("\n"))
    } else {
        query(endpoint, "SHOW FULL TABLES;")
    }
}
#[cfg(test)]
fn preview(endpoint: &Endpoint) -> Result<String, String> {
    let preview = preview_grid(endpoint, None)?;
    Ok(std::iter::once(preview.columns.join("\t"))
        .chain(preview.rows.into_iter().map(|row| row.join("\t")))
        .collect::<Vec<_>>()
        .join("\n"))
}

fn sqlite_backup(
    source: &Path,
    target: &Path,
    importing: bool,
    c: &Control,
    tx: &async_channel::Sender<Event>,
) -> Result<(), String> {
    let from = sqlite_open(source, false, c)?;
    let mut to = sqlite_open(target, true, c)?;
    let backup = rusqlite::backup::Backup::new(&from, &mut to).map_err(db_error)?;
    let mut blocked = None;
    let mut last_notice = std::time::Instant::now() - Duration::from_secs(1);
    loop {
        c.check()?;
        let step = backup.step(256).map_err(db_error)?;
        let progress = backup.progress();
        if last_notice.elapsed() >= Duration::from_millis(100)
            || step == rusqlite::backup::StepResult::Done
        {
            notify(
                tx,
                Progress {
                    completed: (progress.pagecount - progress.remaining).max(0) as u64,
                    total: Some(progress.pagecount.max(0) as u64),
                    unit: Unit::Pages,
                    importing,
                    bytes: std::fs::metadata(target).map(|m| m.len()).unwrap_or(0),
                    ..Default::default()
                },
            );
            last_notice = std::time::Instant::now();
        }
        match step {
            rusqlite::backup::StepResult::Done => return Ok(()),
            rusqlite::backup::StepResult::More => blocked = None,
            _ => {
                let since = blocked.get_or_insert_with(std::time::Instant::now);
                if since.elapsed() > Duration::from_secs(30) {
                    return Err("SQLite 备份等待锁超时".into());
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }
}
fn restore_sqlite(
    input: &Path,
    target: &Path,
    c: &Control,
    tx: &async_channel::Sender<Event>,
) -> Result<(), String> {
    let mut header = [0; 16];
    let mut file = File::open(input).map_err(io_error)?;
    let n = file.read(&mut header).map_err(io_error)?;
    if n == 16 && &header == b"SQLite format 3\0" {
        if target.exists() {
            let db = sqlite_open(target, false, c)?;
            if !sqlite_objects(&db)?.is_empty() {
                return Err("SQLite 文件恢复要求空目标库，不覆盖现有对象".into());
            }
        }
        sqlite_backup(input, target, true, c, tx)
    } else {
        sqlite_import(input, target, c, tx)
    }
}

/// Run one explicit adapter operation with cancellation; drops its private session on return.
pub fn service_operation(
    endpoint: &Endpoint,
    kind: crate::database::OperationKind,
    c: &Control,
) -> Result<crate::database::OperationResult, String> {
    use crate::{
        database::Operation,
        mysql::{Event, Session},
    };
    let mut profile = endpoint.profile.clone();
    profile.timeout_seconds = 60;
    let (session, events) = Session::start(profile, endpoint.password.clone(), false);
    let mut kind = Some(kind);
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    loop {
        c.check()?;
        if std::time::Instant::now() > deadline {
            return Err("兼容任务超时，目标可能保留已写入内容".into());
        }
        match events.try_recv() {
            Ok(Event::Connected(_)) => {
                if let Some(kind) = kind.take() {
                    session.request(Operation { id: 1, kind })?;
                }
            }
            Ok(Event::Operation { result, .. }) => return result,
            Ok(Event::Failed(error)) => {
                return Err(if error.contains("1045")
                    || error.contains("认证")
                    || error.contains("密码")
                {
                    "兼容连接认证失败，请重新连接并核对凭据及旧密码认证选项"
                } else if error.contains("TLS") {
                    "兼容连接 TLS 策略不受支持，请核对已确认的连接策略"
                } else if error.contains("辅助") || error.contains("进程") {
                    "兼容辅助程序不可用，请使用包含兼容辅助程序的完整应用目录"
                } else if error.contains("位数") || error.contains("DLL") || error.contains("dll")
                {
                    "兼容驱动加载失败，请核对客户端位数及版本"
                } else {
                    "兼容连接操作失败，请核对连接、驱动和权限"
                }
                .into());
            }
            Ok(Event::Disconnected) => return Err("兼容连接已断开，任务未完成".into()),
            Err(async_channel::TryRecvError::Closed) => return Err("兼容任务已结束".into()),
            _ => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

fn pg_command(endpoint: &Endpoint, name: &str) -> Result<NativeCommand, String> {
    use std::io::{Seek, SeekFrom};
    let p = &endpoint.profile;
    if p.database.contains('=') || p.database.contains("://") {
        return Err("客户端任务暂不支持含连接串符号的数据库名称".into());
    }
    let mut credentials = credential_file("")?;
    let fields = [
        p.host.as_str(),
        &p.port.to_string(),
        "*",
        p.user.as_str(),
        endpoint.password.as_str(),
    ];
    if fields.iter().any(|s| s.contains(['\r', '\n', '\0'])) {
        return Err("PostgreSQL 凭据含客户端密码文件不支持的控制字符".into());
    }
    let line = fields
        .iter()
        .map(|s| s.replace('\\', "\\\\").replace(':', "\\:"))
        .collect::<Vec<_>>()
        .join(":");
    credentials.as_file_mut().set_len(0).map_err(io_error)?;
    credentials.seek(SeekFrom::Start(0)).map_err(io_error)?;
    writeln!(credentials, "{line}").map_err(io_error)?;
    credentials.flush().map_err(io_error)?;
    let mut cmd = command(&tool(p, name)?);
    cmd.arg(format!("--host={}", p.host))
        .arg(format!("--port={}", p.port))
        .arg(format!("--username={}", p.user))
        .arg("--no-password")
        .arg(format!(
            "--{}={}",
            if name == "createdb" {
                "maintenance-db"
            } else {
                "dbname"
            },
            p.database
        ))
        .env_remove("PGPASSWORD")
        .env_remove("PGSERVICE")
        .env_remove("PGSERVICEFILE")
        .env_remove("PGHOSTADDR")
        .env_remove("PGOPTIONS")
        .env("PGPASSFILE", credentials.path())
        .env("PGCONNECT_TIMEOUT", p.timeout_seconds.to_string())
        .env("PGCLIENTENCODING", "UTF8")
        .env(
            "PGSSLMODE",
            match p.tls {
                TlsMode::Disabled => "disable",
                TlsMode::Required => "require",
                TlsMode::VerifyIdentity => "verify-full",
            },
        );
    for (name, value) in [
        ("PGSSLROOTCERT", &p.ca),
        ("PGSSLCERT", &p.client_cert),
        ("PGSSLKEY", &p.client_key),
    ] {
        if value.is_empty() {
            cmd.env_remove(name);
        } else {
            cmd.env(name, value);
        }
    }
    Ok(NativeCommand {
        command: cmd,
        _credentials: credentials,
    })
}
fn pg_export(
    source: &Endpoint,
    path: &Path,
    contents: Contents,
    options: &Options,
    c: &Control,
    tx: &async_channel::Sender<Event>,
) -> Result<(), String> {
    let mut cmd = pg_command(source, "pg_dump")?;
    cmd.args([
        "--format=plain",
        "--no-owner",
        "--no-privileges",
        "--no-tablespaces",
        "--encoding=UTF8",
    ]);
    if contents.data {
        cmd.arg(format!("--rows-per-insert={}", options.batch_size));
    }
    if let Some(tables) = &options.tables {
        for table in tables {
            cmd.arg(format!("--table={table}"));
        }
    }
    if !contents.data {
        cmd.arg("--schema-only");
    }
    if !contents.schema {
        cmd.arg("--data-only");
    }
    cmd.stdin(Stdio::null())
        .stdout(File::create(path).map_err(io_error)?)
        .stderr(Stdio::piped());
    wait_child(cmd.spawn().map_err(io_error)?, c, false, || {
        notify(
            tx,
            Progress {
                bytes: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
                ..Default::default()
            },
        )
    })
}
// SQL file imports must never execute psql shell, include-file or reconnect commands.
fn pg_import(
    target: &Endpoint,
    path: &Path,
    c: &Control,
    tx: &async_channel::Sender<Event>,
) -> Result<(), String> {
    pg_sql_guard::validate(path, || c.cancelled.load(Ordering::Relaxed))?;
    c.check()?;
    let mut cmd = pg_command(target, "psql")?;
    cmd.args([
        "--no-psqlrc",
        "--set=ON_ERROR_STOP=1",
        "--single-transaction",
        "--file=-",
    ])
    .stdin(Stdio::piped())
    .stdout(Stdio::null())
    .stderr(Stdio::piped());
    feed_command(cmd, path, c, tx)
}

pub fn freeze_file(path: &Path, c: &Control) -> Result<tempfile::NamedTempFile, String> {
    let mut source = File::open(path).map_err(io_error)?;
    let before = source.metadata().map_err(io_error)?;
    let mut snapshot = tempfile::NamedTempFile::new().map_err(io_error)?;
    let mut buffer = [0; 65536];
    loop {
        c.check()?;
        let n = source.read(&mut buffer).map_err(io_error)?;
        if n == 0 {
            break;
        }
        snapshot.write_all(&buffer[..n]).map_err(io_error)?;
    }
    snapshot.flush().map_err(io_error)?;
    let after = source.metadata().map_err(io_error)?;
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return Err("文件在预览期间发生变化，请重新预览".into());
    }
    c.check()?;
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hidden_event_denial_does_not_disable_readable_views() {
        let preview = Preview {
            objects: vec![ObjectChoice {
                name: "visible".into(),
                kind: "view".into(),
            }],
            ..Default::default()
        };
        let checked =
            inspect_permissions("`fixture`", &preview, &Control::default(), true, |sql| {
                if sql.starts_with("SHOW EVENTS") {
                    Err("数据库权限不足（错误码 1044）".into())
                } else if sql.contains("mysql.proc") {
                    Err("对象不存在（错误码 1146）".into())
                } else if sql.starts_with("SHOW CREATE VIEW") {
                    Ok("visible\tCREATE VIEW visible AS SELECT 1".into())
                } else {
                    Err("未确认".into())
                }
            });
        assert_eq!(checked.0["event"], ReadPermission::Denied);
        assert_eq!(checked.0["view"], ReadPermission::Available);
        assert_eq!(checked.0["routine"], ReadPermission::Unknown);
        assert!(!checked.denied("trigger"));
    }
    #[test]
    fn permission_checks_only_disable_confirmed_denials() {
        assert_eq!(
            permission_result(Err("数据库权限不足（错误码 1044）".into())),
            ReadPermission::Denied
        );
        assert_eq!(
            permission_result(Err("网络失败；退出码 2".into())),
            ReadPermission::Unknown
        );
        assert_eq!(
            permission_result(Ok("name\tNULL".into())),
            ReadPermission::Unknown
        );
        let permissions = ReadPermissions(std::collections::HashMap::from([(
            "event".into(),
            ReadPermission::Denied,
        )]));
        let mut options = Options::default();
        assert!(permissions.blocks(&options));
        options.events = false;
        assert!(!permissions.blocks(&options));
    }
    #[test]
    fn client_capabilities_are_not_tied_to_version_substrings() {
        assert_eq!(
            dump_compatibility_args("no-tablespaces set-gtid-purged"),
            vec!["--set-gtid-purged=OFF", "--no-tablespaces"]
        );
        assert_eq!(
            dump_compatibility_args("no-tablespaces"),
            vec!["--no-tablespaces"]
        );
        assert!(dump_compatibility_args("legacy options").is_empty());
    }
    #[test]
    fn catalog_lists_objects_without_sampling_data() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("fixture.db");
        let db = rusqlite::Connection::open(&file).unwrap();
        db.execute_batch("CREATE TABLE items(id INTEGER); CREATE VIEW v AS SELECT * FROM items; CREATE TRIGGER t AFTER INSERT ON items BEGIN SELECT 1; END;").unwrap();
        drop(db);
        let endpoint = Endpoint {
            profile: Profile {
                kind: DatabaseKind::Sqlite,
                sqlite_path: file.display().to_string(),
                ..Default::default()
            },
            password: String::new(),
        };
        let (preview, _) = catalog(&endpoint, &Control::default()).unwrap();
        for kind in ["table", "view", "trigger"] {
            assert!(preview.objects.iter().any(|o| o.kind == kind));
        }
        assert!(preview.rows.is_empty() && preview.columns.is_empty());
    }
    #[test]
    fn preflight_reports_existing_new_database_without_writing() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.db");
        let db = rusqlite::Connection::open(&source).unwrap();
        db.execute_batch("CREATE TABLE unchanged(id INT);").unwrap();
        let before = std::fs::read(&source).unwrap();
        let result = preflight(
            &endpoint(&source),
            Some(&endpoint(&source)),
            Mode::Copy,
            source.to_str().unwrap(),
            &Control::default(),
        );
        assert!(result.is_err_and(|e| e.contains("已存在")));
        assert!(std::fs::read(source).unwrap() == before);
    }
    #[test]
    fn sqlite_file_import_cannot_attach_another_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("input.sql");
        let outside = dir.path().join("outside.db");
        std::fs::write(
            &file,
            format!(
                "ATTACH DATABASE '{}' AS external;\nCREATE TABLE external.items(id INT);",
                outside.display().to_string().replace('\'', "''")
            ),
        )
        .unwrap();
        let (tx, _) = async_channel::bounded(4);
        assert!(
            sqlite_import(
                &file,
                &dir.path().join("target.db"),
                &Control::default(),
                &tx
            )
            .is_err()
        );
        assert!(!outside.exists());
    }
    #[test]
    fn file_preview_freezes_execution_content() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("import.sql");
        std::fs::write(&original, "SELECT 1;").unwrap();
        let snapshot = freeze_file(&original, &Control::default()).unwrap();
        std::fs::write(&original, "SELECT 2;").unwrap();
        assert!(std::fs::read_to_string(snapshot.path()).unwrap() == "SELECT 1;");
    }
    #[test]
    fn temporary_client_credentials_are_private_and_removed() {
        let secret = uuid::Uuid::new_v4().to_string() + "\"\\\n";
        let file = credential_file(&secret).unwrap();
        let path = file.path().to_path_buf();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.starts_with("[client]\npassword=\""));
        assert!(content.contains("\\\"\\\\\\n"));
        drop(file);
        assert!(!path.exists());
    }
    #[test]
    #[ignore = "requires two explicitly supplied isolated MySQL fixture ports"]
    fn mysql_native_file_export_import_and_cross_connection_copy() {
        let port = std::env::var("TRANSFER_SOURCE_PORT")
            .unwrap()
            .parse()
            .unwrap();
        let target_port = std::env::var("TRANSFER_TARGET_PORT")
            .unwrap()
            .parse()
            .unwrap();
        let endpoint = |port, database: &str| Endpoint {
            profile: Profile {
                kind: DatabaseKind::MySql,
                host: "127.0.0.1".into(),
                port,
                user: "root".into(),
                database: database.into(),
                tls: TlsMode::Disabled,
                ..Default::default()
            },
            password: String::new(),
        };
        let fixture_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .canonicalize()
            .unwrap();
        for endpoint in [
            endpoint(port, "import_alpha"),
            endpoint(target_port, "import_beta"),
        ] {
            let data = query(&endpoint, "SELECT @@datadir;").unwrap();
            let data = Path::new(data.trim()).canonicalize().unwrap();
            assert!(data.starts_with(&fixture_root));
            assert!(
                data.parent()
                    .and_then(Path::file_name)
                    .is_some_and(|name| name.to_string_lossy().starts_with("transfer-mysql-"))
            );
        }
        let source = endpoint(port, "import_alpha");
        query(&source,"CREATE TABLE items(id INT PRIMARY KEY,value VARCHAR(32),payload LONGBLOB); INSERT INTO items VALUES(1,'fixture',REPEAT('x',1048576));").unwrap();
        let shown = preview(&source).unwrap();
        assert!(shown.len() < 32768);
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("database_export.sql");
        let (tx, _) = async_channel::bounded(16);
        Job {
            options: Options::default(),
            mode: Mode::Export,
            source: endpoint(port, "import_alpha"),
            target: None,
            database: String::new(),
            file: file.clone(),
            contents: Contents {
                schema: true,
                data: true,
            },
        }
        .run(Arc::default(), &tx)
        .unwrap();
        assert!(std::fs::metadata(&file).unwrap().len() > 768 * 1024);
        Job {
            options: Options::default(),
            mode: Mode::ImportFile,
            source: endpoint(target_port, "import_beta"),
            target: Some(endpoint(target_port, "import_beta")),
            database: String::new(),
            file: file.clone(),
            contents: Contents {
                schema: true,
                data: true,
            },
        }
        .run(Arc::default(), &tx)
        .unwrap();
        assert!(
            query(
                &endpoint(target_port, "import_beta"),
                "SELECT LENGTH(payload) FROM items;"
            )
            .unwrap()
            .trim()
                == "1048576"
        );
        query(&endpoint(port,"import_alpha"),"UPDATE items SET value='updated'; CREATE TABLE keep(id INT); INSERT INTO keep VALUES(7);").unwrap();
        query(&endpoint(target_port,"import_beta"),"INSERT INTO items VALUES(3,'retained',NULL); CREATE TABLE keep(id INT); INSERT INTO keep VALUES(9);").unwrap();
        for port in [port, target_port] {
            let db = if port == target_port {
                "import_beta"
            } else {
                "import_alpha"
            };
            query(&endpoint(port,db),"CREATE TABLE typed(id INT PRIMARY KEY,v JSON,stamp TIMESTAMP DEFAULT CURRENT_TIMESTAMP,raw VARBINARY(8));").unwrap();
        }
        query(&endpoint(port,"import_alpha"),"INSERT INTO typed(id,v,raw) VALUES(1,JSON_OBJECT('text',CONVERT(X'E4B8AD' USING utf8mb4)),X'0009FF');").unwrap();
        for write_mode in [WriteMode::Upsert, WriteMode::Replace] {
            Job {
                mode: Mode::ImportConnection,
                source: endpoint(port, "import_alpha"),
                target: Some(endpoint(target_port, "import_beta")),
                database: String::new(),
                file: PathBuf::new(),
                contents: Contents {
                    schema: false,
                    data: true,
                },
                options: Options {
                    tables: Some(vec!["items".into(), "typed".into()]),
                    batch_size: 1,
                    write_mode,
                    ..Default::default()
                },
            }
            .run(Arc::default(), &tx)
            .unwrap();
            let count = if write_mode == WriteMode::Upsert {
                "2"
            } else {
                "1"
            };
            assert!(
                query(
                    &endpoint(target_port, "import_beta"),
                    "SELECT COUNT(*) FROM items;"
                )
                .unwrap()
                .trim()
                    == count
            );
            assert!(
                query(
                    &endpoint(target_port, "import_beta"),
                    "SELECT value,LENGTH(payload) FROM items WHERE id=1;"
                )
                .unwrap()
                .trim()
                    == "updated\t1048576"
            );
            assert!(
                query(
                    &endpoint(target_port, "import_beta"),
                    "SELECT id FROM keep;"
                )
                .unwrap()
                .trim()
                    == "9"
            );
        }
        query(
            &endpoint(target_port, "import_beta"),
            "CREATE UNIQUE INDEX other_unique ON items(value);",
        )
        .unwrap();
        assert!(
            Job {
                mode: Mode::ImportConnection,
                source: endpoint(port, "import_alpha"),
                target: Some(endpoint(target_port, "import_beta")),
                database: String::new(),
                file: PathBuf::new(),
                contents: Contents {
                    schema: false,
                    data: true
                },
                options: Options {
                    tables: Some(vec!["items".into()]),
                    write_mode: WriteMode::Upsert,
                    ..Default::default()
                },
            }
            .run(Arc::default(), &tx)
            .is_err()
        );
        assert!(query(&endpoint(target_port,"import_beta"),"SELECT HEX(JSON_UNQUOTE(JSON_EXTRACT(v,'$.text'))),HEX(raw),stamp IS NOT NULL FROM typed;").unwrap().trim()=="E4B8AD\t0009FF\t1");
        let mut rejected = endpoint(target_port, "import_beta");
        rejected.password = uuid::Uuid::new_v4().to_string();
        assert!(
            preflight(
                &endpoint(port, "import_alpha"),
                Some(&rejected),
                Mode::ImportConnection,
                "",
                &Control::default()
            )
            .is_err_and(|e| e.contains("目标执行客户端连接失败"))
        );
        let target_db = format!("transfer_{}", uuid::Uuid::new_v4().simple());
        Job {
            options: Options::default(),
            mode: Mode::Copy,
            source: endpoint(port, "import_alpha"),
            target: Some(endpoint(target_port, "import_beta")),
            database: target_db.clone(),
            file: PathBuf::new(),
            contents: Contents {
                schema: true,
                data: true,
            },
        }
        .run(Arc::default(), &tx)
        .unwrap();
        assert!(
            query(
                &endpoint(target_port, &target_db),
                "SELECT LENGTH(payload) FROM items;"
            )
            .unwrap()
            .trim()
                == "1048576"
        );
        assert!(
            Job {
                options: Options::default(),
                mode: Mode::Copy,
                source: endpoint(port, "import_alpha"),
                target: Some(endpoint(target_port, "import_beta")),
                database: target_db,
                file: PathBuf::new(),
                contents: Contents {
                    schema: true,
                    data: false
                }
            }
            .run(Arc::default(), &tx)
            .is_err()
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
    fn sqlite_streams_complete_typed_data_and_backups_without_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.db");
        let restored = dir.path().join("restored.db");
        let dump = dir.path().join("export.sql");
        let db = rusqlite::Connection::open(&source).unwrap();
        db.execute_batch("CREATE TABLE items(id INTEGER PRIMARY KEY, value TEXT, payload BLOB, derived TEXT GENERATED ALWAYS AS (value||'!') VIRTUAL); CREATE INDEX item_value ON items(value); CREATE VIEW item_view AS SELECT id,value FROM items;").unwrap();
        let payload = vec![0x9a; 1024 * 1024];
        db.execute(
            "INSERT INTO items(id,value,payload) VALUES(1,?1,?2)",
            rusqlite::params!["a\0b", payload],
        )
        .unwrap();
        drop(db);
        let control = Arc::new(Control::default());
        let (tx, _) = async_channel::bounded(16);
        Job {
            options: Options::default(),
            mode: Mode::Export,
            source: endpoint(&source),
            target: None,
            database: String::new(),
            file: dump.clone(),
            contents: Contents {
                schema: true,
                data: true,
            },
        }
        .run(control.clone(), &tx)
        .unwrap();
        assert!(std::fs::metadata(&dump).unwrap().len() > 768 * 1024);
        assert!(
            std::fs::read_to_string(&dump)
                .unwrap()
                .contains("CREATE TABLE IF NOT EXISTS items")
        );
        sqlite_import(&dump, &restored, &control, &tx).unwrap();
        let copied = rusqlite::Connection::open(&restored).unwrap();
        let (text, blob, derived): (String, Vec<u8>, String) = copied
            .query_row("SELECT value,payload,derived FROM items", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!(text, "a\0b");
        assert_eq!(blob, payload);
        assert_eq!(derived, "a\0b!");
        drop(copied);
        let new = dir.path().join("copy.db");
        Job {
            options: Options::default(),
            mode: Mode::Copy,
            source: endpoint(&source),
            target: Some(endpoint(&source)),
            database: new.display().to_string(),
            file: PathBuf::new(),
            contents: Contents {
                schema: true,
                data: true,
            },
        }
        .run(control.clone(), &tx)
        .unwrap();
        assert!(new.exists());
        let original = std::fs::read(&dump).unwrap();
        assert!(
            Job {
                options: Options::default(),
                mode: Mode::Export,
                source: endpoint(&source),
                target: None,
                database: String::new(),
                file: dump.clone(),
                contents: Contents {
                    schema: true,
                    data: true
                }
            }
            .run(control.clone(), &tx)
            .is_err()
        );
        assert_eq!(std::fs::read(&dump).unwrap(), original);
        control.cancel();
        let cancelled = dir.path().join("cancelled.sql");
        assert!(
            Job {
                options: Options::default(),
                mode: Mode::Export,
                source: endpoint(&source),
                target: None,
                database: String::new(),
                file: cancelled.clone(),
                contents: Contents {
                    schema: true,
                    data: true
                }
            }
            .run(control, &tx)
            .is_err()
        );
        assert!(!cancelled.exists());
    }
    #[test]
    fn sqlite_schema_only_cross_connection_and_self_import_guard() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.db");
        let target = dir.path().join("target.db");
        let db = rusqlite::Connection::open(&source).unwrap();
        db.execute_batch("CREATE TABLE items(id INTEGER);INSERT INTO items VALUES(1);")
            .unwrap();
        drop(db);
        let db = rusqlite::Connection::open(&target).unwrap();
        db.execute_batch("PRAGMA user_version=1;").unwrap();
        drop(db);
        let (tx, _) = async_channel::bounded(16);
        Job {
            options: Options::default(),
            mode: Mode::ImportConnection,
            source: endpoint(&source),
            target: Some(endpoint(&target)),
            database: String::new(),
            file: PathBuf::new(),
            contents: Contents {
                schema: true,
                data: false,
            },
        }
        .run(Arc::default(), &tx)
        .unwrap();
        let db = rusqlite::Connection::open(&target).unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM items", [], |r| r.get::<_, u64>(0))
                .unwrap(),
            0
        );
        drop(db);
        assert!(
            Job {
                options: Options::default(),
                mode: Mode::ImportConnection,
                source: endpoint(&source),
                target: Some(endpoint(&source)),
                database: String::new(),
                file: PathBuf::new(),
                contents: Contents {
                    schema: false,
                    data: true
                }
            }
            .run(Arc::default(), &tx)
            .is_err()
        );
    }
}

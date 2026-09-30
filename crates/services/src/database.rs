//! Common connection operations. Adapters own protocol-specific capabilities.
use crate::{
    DatabaseKind,
    mysql::{self, Event, Profile, ServerInfo, Session, TlsMode},
};
use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use sqlx::{ConnectOptions, Connection, Row};
use std::{
    path::Path,
    time::{Duration, Instant},
};
pub mod connection_import;
pub mod copy;
pub mod notebook_query;
pub mod redis_value;
mod remote;
pub mod sql_assist;
pub mod transfer;
pub mod workbench;
use remote::{HttpDatabase, RedisDatabase};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ObjectItem {
    pub schema: String,
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub key: Option<Vec<u8>>,
}
impl ObjectItem {
    pub fn new(schema: String, name: String, kind: String) -> Self {
        Self {
            schema,
            name,
            kind,
            key: None,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum OperationKind {
    Workbench(Box<workbench::Request>),
    Databases,
    Objects {
        database: String,
        cursor: Option<String>,
    },
    Details {
        database: String,
        object: ObjectItem,
    },
    CopyDatabase {
        source: String,
        target: String,
        #[serde(default)]
        charset: String,
        #[serde(default)]
        include_data: bool,
    },
    CreateDatabase {
        name: String,
        #[serde(default)]
        charset: String,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Operation {
    pub id: u64,
    pub kind: OperationKind,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum OperationResult {
    Workbench(Box<workbench::Reply>),
    Databases(Vec<String>),
    Objects {
        items: Vec<ObjectItem>,
        cursor: Option<String>,
        notice: Option<String>,
    },
    Details(Vec<(String, String)>),
    Created(String),
    Copied {
        name: String,
        tables: usize,
        include_data: bool,
    },
}
pub fn objects(items: Vec<ObjectItem>) -> OperationResult {
    let truncated = items.len() > 500;
    OperationResult::Objects {
        items: items.into_iter().take(500).collect(),
        cursor: None,
        notice: truncated.then(|| "对象超过 500 个，请缩小范围".into()),
    }
}
pub fn defaults(kind: DatabaseKind) -> Profile {
    Profile {
        kind,
        port: kind.port(),
        user: match kind {
            DatabaseKind::PostgreSql => "postgres",
            DatabaseKind::ClickHouse => "default",
            DatabaseKind::Redis | DatabaseKind::Sqlite => "",
            _ => "root",
        }
        .into(),
        name: format!("新建 {} 连接", kind.label()),
        ..Default::default()
    }
}
pub fn validate_profile(p: &Profile) -> Result<(), String> {
    if p.kind == DatabaseKind::Sqlite
        && (p.sqlite_path.is_empty() || !Path::new(&p.sqlite_path).is_absolute())
    {
        return Err("请选择 SQLite 数据库文件的绝对路径".into());
    }
    if p.kind == DatabaseKind::Redis && !p.database.is_empty() {
        let n = p
            .database
            .parse::<u32>()
            .map_err(|_| "Redis 数据库编号必须为非负整数")?;
        if p.redis_cluster && n != 0 {
            return Err("Redis 集群仅支持 DB 0".into());
        }
    }
    Ok(())
}
pub fn validate_name(kind: DatabaseKind, name: &str) -> Result<(), String> {
    let limit = if kind == DatabaseKind::PostgreSql {
        63
    } else {
        192
    };
    if name.trim().is_empty() || name.len() > limit || name.chars().any(char::is_control) {
        return Err("数据库名称为空、过长或含控制字符".into());
    }
    if !kind.can_create_database() {
        return Err("该类型不支持服务器建库".into());
    }
    Ok(())
}
pub fn create_database_sql(
    kind: DatabaseKind,
    name: &str,
    charset: &str,
) -> Result<String, String> {
    validate_name(kind, name)?;
    if !charset.is_empty()
        && (charset.len() > 40
            || !charset
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))
    {
        return Err("字符集名称无效".into());
    }
    let quoted = if kind == DatabaseKind::MySql {
        quote_mysql(name)
    } else {
        quote_pg(name)
    };
    let suffix = if charset.is_empty() {
        String::new()
    } else {
        match kind {
            DatabaseKind::MySql if !charset.contains('-') => {
                format!(" CHARACTER SET {}", quote_mysql(charset))
            }
            DatabaseKind::PostgreSql => format!(" ENCODING '{charset}' TEMPLATE template0"),
            _ => return Err("当前数据库类型不支持此字符集设置".into()),
        }
    };
    Ok(format!("CREATE DATABASE {quoted}{suffix}"))
}
pub fn quote_mysql(name: &str) -> String {
    format!("`{}`", name.replace('`', "``"))
}
fn quote_pg(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}
fn info(
    version: String,
    charset: &str,
    database: Option<String>,
    databases: Vec<String>,
    tls: &str,
    elapsed: u128,
) -> ServerInfo {
    ServerInfo {
        version,
        session_id: 0,
        charset: charset.into(),
        database,
        databases,
        tls_cipher: tls.into(),
        elapsed_ms: elapsed,
        database_list_error: None,
    }
}
fn db_error(error: sqlx::Error) -> String {
    if let sqlx::Error::Database(db) = &error
        && let Some(error) = db.try_downcast_ref::<sqlx::postgres::PgDatabaseError>()
    {
        match error.code() {
            "42501" => return "数据库权限不足，当前账号无法执行此操作".into(),
            "42P04" => return "数据库已存在，未重复创建".into(),
            "3D000" => return "数据库不存在或不可访问".into(),
            "28P01" | "28000" => return "认证失败，请检查用户名、密码和来源授权".into(),
            _ => {}
        }
    }
    mysql::error_message(error)
}

pub fn start(
    profile: Profile,
    password: String,
    test: bool,
) -> (Session, async_channel::Receiver<Event>) {
    let (stop, rx) = async_channel::bounded(1);
    let (commands, requests) = async_channel::bounded::<Operation>(8);
    let (events, out) = async_channel::bounded(8);
    std::thread::spawn(move || {
        let failure_events = events.clone();
        let task_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            let Ok(runtime) = runtime else {
                let _ = events.send_blocking(Event::Failed("无法启动数据库后台任务".into()));
                return;
            };
            runtime.block_on(async move {
            let timeout=Duration::from_secs(profile.timeout_seconds.clamp(1,60));
            let opened=tokio::select!{_=rx.recv()=>return,result=tokio::time::timeout(timeout,Backend::open(&profile,&password))=>result};
            drop(password);
            let (mut backend,mut info)=match opened {Ok(Ok(result))=>result,Ok(Err(e))=>{let _=events.send(Event::Failed(e)).await;return;},Err(_)=>{let _=events.send(Event::Failed("连接超时，已取消".into())).await;return;}};
            if info.databases.len()>=512 {info.database_list_error=Some("数据库列表最多显示 512 项，可在连接配置中直接指定库名".into());}
            if test{let _=events.send(Event::Tested(info)).await;return;}
            if events.send(Event::Connected(info)).await.is_err(){return;}
            let mut ping=tokio::time::interval(Duration::from_secs(30));ping.tick().await;
            loop {tokio::select! {
                _=rx.recv()=>break,
                request=requests.recv()=>{
                    let Ok(request)=request else{break;};let id=request.id;
                    let result=tokio::select!{_=rx.recv()=>break,result=tokio::time::timeout(timeout,backend.operation(&profile,&request))=>result};
                    match result {Ok(result)=>{let _=events.send(Event::Operation{id,result}).await;},Err(_)=>{let _=events.send(Event::Operation{id,result:Err("操作超时；建库结果可能已生效，请重连确认，勿重复提交".into())}).await;break;}}
                }
                _=ping.tick()=>{let ok=tokio::select!{_=rx.recv()=>break,result=tokio::time::timeout(Duration::from_secs(5),backend.ping())=>matches!(result,Ok(Ok(())))};if !ok {let _=events.send(Event::Failed("连接已失效，请重新连接".into())).await;break;}}
            }}
            let _=events.send(Event::Disconnected).await;
        });
        }));
        if task_result.is_err() {
            let _ = failure_events.send_blocking(Event::Failed(
                "数据库适配器异常，连接已关闭；请检查驱动与服务端兼容性".into(),
            ));
        }
    });
    (Session::from_channels(stop, commands), out)
}
enum Backend {
    Postgres {
        connection: sqlx::PgConnection,
        options: Box<sqlx::postgres::PgConnectOptions>,
        current: String,
    },
    Sqlite(sqlx::SqliteConnection),
    ClickHouse(HttpDatabase),
    Redis(RedisDatabase),
}
impl Backend {
    async fn open(p: &Profile, password: &str) -> Result<(Self, ServerInfo), String> {
        p.validate()?;
        validate_profile(p)?;
        let begin = Instant::now();
        match p.kind {
            DatabaseKind::PostgreSql => {
                use sqlx::postgres::{PgConnectOptions, PgSslMode};
                let mut options = PgConnectOptions::new()
                    .host(&p.host)
                    .port(p.port)
                    .username(&p.user)
                    .password(password)
                    .database(if p.database.is_empty() {
                        "postgres"
                    } else {
                        &p.database
                    })
                    .ssl_mode(match p.tls {
                        TlsMode::VerifyIdentity => PgSslMode::VerifyFull,
                        TlsMode::Required => PgSslMode::Require,
                        TlsMode::Disabled => PgSslMode::Disable,
                    })
                    .disable_statement_logging();
                if !p.ca.is_empty() {
                    options = options.ssl_root_cert(&p.ca);
                }
                if !p.client_cert.is_empty() {
                    options = options
                        .ssl_client_cert(&p.client_cert)
                        .ssl_client_key(&p.client_key);
                }
                let mut connection = sqlx::PgConnection::connect_with(&options)
                    .await
                    .map_err(db_error)?;
                let row =
                    sqlx::query("SELECT current_setting('server_version'), current_database()")
                        .fetch_one(&mut connection)
                        .await
                        .map_err(db_error)?;
                let version: String = row.try_get(0).map_err(db_error)?;
                let current: String = row.try_get(1).map_err(db_error)?;
                let databases = if p.database.is_empty() || p.list_all_databases {
                    pg_databases(&mut connection).await?
                } else {
                    vec![p.database.clone()]
                };
                let info = info(
                    version,
                    "UTF8",
                    Some(current.clone()),
                    databases,
                    if p.tls == TlsMode::Disabled {
                        ""
                    } else {
                        "TLS"
                    },
                    begin.elapsed().as_millis(),
                );
                Ok((
                    Self::Postgres {
                        connection,
                        options: Box::new(options),
                        current,
                    },
                    info,
                ))
            }
            DatabaseKind::Sqlite => {
                if !Path::new(&p.sqlite_path).is_file() {
                    return Err("SQLite 文件不存在；请选择文件，或明确新建文件".into());
                }
                let options = sqlx::sqlite::SqliteConnectOptions::new()
                    .filename(&p.sqlite_path)
                    .create_if_missing(false)
                    .read_only(p.sqlite_read_only)
                    .disable_statement_logging();
                let mut connection = sqlx::SqliteConnection::connect_with(&options)
                    .await
                    .map_err(db_error)?;
                let version: String = sqlx::query_scalar("SELECT sqlite_version()")
                    .fetch_one(&mut connection)
                    .await
                    .map_err(db_error)?;
                // Read schema too: opening an arbitrary file alone does not validate SQLite format.
                sqlx::query("SELECT name FROM sqlite_master LIMIT 1")
                    .fetch_optional(&mut connection)
                    .await
                    .map_err(db_error)?;
                let name = Path::new(&p.sqlite_path)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                Ok((
                    Self::Sqlite(connection),
                    info(
                        version,
                        "UTF8",
                        Some(name.clone()),
                        vec![name],
                        "",
                        begin.elapsed().as_millis(),
                    ),
                ))
            }
            DatabaseKind::ClickHouse => {
                let (client, info) = HttpDatabase::open(p, password).await?;
                Ok((Self::ClickHouse(client), info))
            }
            DatabaseKind::Redis => {
                let (client, info) = RedisDatabase::open(p, password).await?;
                Ok((Self::Redis(client), info))
            }
            DatabaseKind::MySql => Err("MySQL 应使用专属适配器".into()),
        }
    }
    async fn ping(&mut self) -> Result<(), String> {
        match self {
            Self::Postgres { connection, .. } => connection.ping().await.map_err(db_error),
            Self::Sqlite(c) => c.ping().await.map_err(db_error),
            Self::ClickHouse(c) => c.ping().await,
            Self::Redis(c) => c.ping().await,
        }
    }
    async fn operation(&mut self, p: &Profile, op: &Operation) -> Result<OperationResult, String> {
        match self {
            Self::ClickHouse(c) => c.operation(p, op).await,
            Self::Redis(c) => c.operation(p, op).await,
            Self::Sqlite(c) => sqlite_operation(c, p, op).await,
            Self::Postgres {
                connection,
                options,
                current,
            } => {
                let database = match &op.kind {
                    OperationKind::Workbench(r) => Some(&r.database),
                    OperationKind::Objects { database, .. }
                    | OperationKind::Details { database, .. } => Some(database),
                    _ => None,
                };
                if let Some(database) = database
                    && database != current
                {
                    let next = sqlx::PgConnection::connect_with(
                        &options.as_ref().clone().database(database),
                    )
                    .await
                    .map_err(db_error)?;
                    *connection = next;
                    *current = database.clone();
                }
                pg_operation(connection, p, op).await
            }
        }
    }
}
async fn pg_databases(c: &mut sqlx::PgConnection) -> Result<Vec<String>, String> {
    sqlx::query_scalar("SELECT datname FROM pg_database WHERE datallowconn AND NOT datistemplate AND has_database_privilege(datname,'CONNECT') ORDER BY datname LIMIT 512").fetch_all(c).await.map_err(db_error)
}
async fn pg_operation(
    c: &mut sqlx::PgConnection,
    p: &Profile,
    op: &Operation,
) -> Result<OperationResult, String> {
    match &op.kind {
        OperationKind::CopyDatabase { .. } => {
            Err("PostgreSQL 请使用原生导出/导入工具复制数据库".into())
        }
        OperationKind::Workbench(r) => workbench::run(c, p.kind, "", r)
            .await
            .map(|r| OperationResult::Workbench(Box::new(r))),
        OperationKind::Databases => Ok(OperationResult::Databases(
            if p.database.is_empty() || p.list_all_databases {
                pg_databases(c).await?
            } else {
                vec![p.database.clone()]
            },
        )),
        OperationKind::CreateDatabase { name, charset } => {
            sqlx::query(&create_database_sql(p.kind, name, charset)?)
                .execute(c)
                .await
                .map_err(db_error)?;
            Ok(OperationResult::Created(name.clone()))
        }
        OperationKind::Objects { .. } => {
            let rows=sqlx::query("SELECT n.nspname,c.relname,CASE WHEN c.relkind IN ('v','m') THEN '视图' ELSE '表' END FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE c.relkind IN ('r','p','v','m','f') AND n.nspname NOT IN ('pg_catalog','information_schema') AND has_schema_privilege(n.oid,'USAGE') AND has_table_privilege(c.oid,'SELECT,INSERT,UPDATE,DELETE,REFERENCES') ORDER BY n.nspname,c.relname LIMIT 501").fetch_all(c).await.map_err(db_error)?;
            Ok(objects(
                rows.into_iter()
                    .map(|r| ObjectItem::new(r.get(0), r.get(1), r.get(2)))
                    .collect(),
            ))
        }
        OperationKind::Details { object, .. } => {
            let mut out = Vec::new();
            for r in sqlx::query("SELECT column_name,data_type,is_nullable,COALESCE(column_default,'') FROM information_schema.columns WHERE table_schema=$1 AND table_name=$2 ORDER BY ordinal_position LIMIT 500").bind(&object.schema).bind(&object.name).fetch_all(&mut *c).await.map_err(db_error)? {
                out.push((format!("列 · {}",r.get::<String,_>(0)),format!("{} · 可空 {} · {}",r.get::<String,_>(1),r.get::<String,_>(2),r.get::<String,_>(3))));
            }
            for r in sqlx::query("SELECT indexname,indexdef FROM pg_indexes WHERE schemaname=$1 AND tablename=$2 LIMIT 500").bind(&object.schema).bind(&object.name).fetch_all(c).await.map_err(db_error)? {out.push((format!("索引 · {}",r.get::<String,_>(0)),r.get(1)));}
            Ok(OperationResult::Details(out))
        }
    }
}
async fn sqlite_operation(
    c: &mut sqlx::SqliteConnection,
    p: &Profile,
    op: &Operation,
) -> Result<OperationResult, String> {
    match &op.kind {
        OperationKind::CopyDatabase {
            target,
            include_data,
            ..
        } => copy::sqlite(c, target, *include_data).await,
        OperationKind::Workbench(r) => workbench::run(c, p.kind, "", r)
            .await
            .map(|r| OperationResult::Workbench(Box::new(r))),
        OperationKind::Databases => Ok(OperationResult::Databases(vec![
            Path::new(&p.sqlite_path)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
        ])),
        OperationKind::CreateDatabase { .. } => Err("SQLite 请使用新建数据库文件".into()),
        OperationKind::Objects { .. } => {
            let rows=sqlx::query("SELECT name,type FROM sqlite_master WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY name LIMIT 501").fetch_all(c).await.map_err(db_error)?;
            Ok(objects(
                rows.into_iter()
                    .map(|r| {
                        ObjectItem::new(
                            String::new(),
                            r.get(0),
                            if r.get::<String, _>(1) == "view" {
                                "视图"
                            } else {
                                "表"
                            }
                            .into(),
                        )
                    })
                    .collect(),
            ))
        }
        OperationKind::Details { object, .. } => {
            let mut out = Vec::new();
            for r in sqlx::query("SELECT name,type,\"notnull\",COALESCE(dflt_value,''),pk FROM pragma_table_xinfo(?)").bind(&object.name).fetch_all(&mut *c).await.map_err(db_error)? {out.push((format!("列 · {}",r.get::<String,_>(0)),format!("{} · 非空 {} · 默认 {} · 主键 {}",r.get::<String,_>(1),r.get::<i64,_>(2),r.get::<String,_>(3),r.get::<i64,_>(4))));}
            for r in sqlx::query("SELECT name,\"unique\" FROM pragma_index_list(?)")
                .bind(&object.name)
                .fetch_all(c)
                .await
                .map_err(db_error)?
            {
                out.push((
                    format!("索引 · {}", r.get::<String, _>(0)),
                    format!("唯一 {}", r.get::<i64, _>(1)),
                ));
            }
            Ok(OperationResult::Details(out))
        }
    }
}
pub async fn create_sqlite_file(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("请指定绝对路径".into());
    }
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| "文件已存在或无法创建，未覆盖原文件")?;
    drop(file);
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false);
    let mut c = sqlx::SqliteConnection::connect_with(&options)
        .await
        .map_err(db_error)?;
    sqlx::query("PRAGMA user_version=0")
        .execute(&mut c)
        .await
        .map_err(db_error)?;
    c.close().await.map_err(db_error)
}
pub async fn mysql_operation(
    c: &mut sqlx::MySqlConnection,
    p: &Profile,
    op: &Operation,
) -> Result<OperationResult, String> {
    sqlx::query(if matches!(op.kind, OperationKind::Workbench(_)) {
        "SET NAMES utf8mb4"
    } else {
        "SET NAMES utf8"
    })
    .execute(&mut *c)
    .await
    .map_err(db_error)?;
    let result = mysql_inner(c, p, op).await;
    sqlx::query(&format!("SET NAMES {}", p.charset))
        .execute(c)
        .await
        .map_err(db_error)?;
    result
}
fn mysql_text(row: &sqlx::mysql::MySqlRow, index: usize) -> Result<String, String> {
    // MySQL marks some textual INFORMATION_SCHEMA expressions as BINARY.
    // The connection requests UTF-8, so decode the bytes explicitly and report failures.
    row.try_get_unchecked::<Option<String>, _>(index)
        .map(|v| v.unwrap_or_default())
        .map_err(db_error)
}
async fn mysql_inner(
    c: &mut sqlx::MySqlConnection,
    p: &Profile,
    op: &Operation,
) -> Result<OperationResult, String> {
    match &op.kind {
        OperationKind::CopyDatabase {
            source,
            target,
            charset,
            include_data,
        } => copy::mysql(c, source, target, charset, *include_data).await,
        OperationKind::Workbench(r) => workbench::run(c, p.kind, "5.6+", r)
            .await
            .map(|r| OperationResult::Workbench(Box::new(r))),
        OperationKind::Databases => {
            if !p.database.is_empty() {
                return Ok(OperationResult::Databases(vec![p.database.clone()]));
            }
            let mut out = Vec::new();
            let mut rows = sqlx::query("SHOW DATABASES").fetch(c);
            while let Some(row) = rows.try_next().await.map_err(db_error)? {
                out.push(mysql_text(&row, 0)?);
                if out.len() == 512 {
                    break;
                }
            }
            Ok(OperationResult::Databases(out))
        }
        OperationKind::CreateDatabase { name, charset } => {
            sqlx::query(&create_database_sql(p.kind, name, charset)?)
                .execute(c)
                .await
                .map_err(db_error)?;
            Ok(OperationResult::Created(name.clone()))
        }
        OperationKind::Objects { database, .. } => {
            let rows=sqlx::query("SELECT TABLE_NAME,TABLE_TYPE FROM information_schema.TABLES WHERE TABLE_SCHEMA=? ORDER BY TABLE_NAME LIMIT 501").bind(database).fetch_all(c).await.map_err(db_error)?;
            let mut items = Vec::new();
            for row in rows {
                items.push(ObjectItem::new(
                    String::new(),
                    mysql_text(&row, 0)?,
                    if mysql_text(&row, 1)? == "VIEW" {
                        "视图"
                    } else {
                        "表"
                    }
                    .into(),
                ));
            }
            Ok(objects(items))
        }
        OperationKind::Details { database, object } => {
            let mut out = Vec::new();
            for r in sqlx::query("SELECT COLUMN_NAME,COLUMN_TYPE,IS_NULLABLE,COALESCE(COLUMN_DEFAULT,''),EXTRA FROM information_schema.COLUMNS WHERE TABLE_SCHEMA=? AND TABLE_NAME=? ORDER BY ORDINAL_POSITION LIMIT 500").bind(database).bind(&object.name).fetch_all(&mut *c).await.map_err(db_error)? {
                out.push((format!("列 · {}",mysql_text(&r,0)?),format!("{} · 可空 {} · {} · {}",mysql_text(&r,1)?,mysql_text(&r,2)?,mysql_text(&r,3)?,mysql_text(&r,4)?)));
            }
            for r in sqlx::query("SELECT INDEX_NAME,COLUMN_NAME,CAST(NON_UNIQUE AS CHAR) FROM information_schema.STATISTICS WHERE TABLE_SCHEMA=? AND TABLE_NAME=? ORDER BY INDEX_NAME,SEQ_IN_INDEX LIMIT 500").bind(database).bind(&object.name).fetch_all(c).await.map_err(db_error)? {
                out.push((format!("索引 · {}",mysql_text(&r,0)?),format!("{} · 非唯一 {}",mysql_text(&r,1)?,mysql_text(&r,2)?)));
            }
            Ok(OperationResult::Details(out))
        }
    }
}
pub fn create_sqlite_file_sync(path: &Path) -> Result<(), String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?
        .block_on(create_sqlite_file(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_profiles_default_to_mysql_and_types_have_distinct_capabilities() {
        let profile: Profile =
            serde_json::from_value(serde_json::json!({"id":uuid::Uuid::new_v4(),"name":"old"}))
                .unwrap();
        assert_eq!(profile.kind, DatabaseKind::MySql);
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("connections.json");
        let sqlite = defaults(DatabaseKind::Sqlite);
        mysql::save_profiles(&path, vec![sqlite.clone()]).unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["version"], 2);
        assert_eq!(mysql::load_profiles(&path).unwrap(), vec![sqlite]);

        assert!(!DatabaseKind::Redis.can_create_database());
        assert!(!DatabaseKind::Sqlite.can_create_database());
        for kind in DatabaseKind::ALL {
            let p = defaults(kind);
            assert_eq!(p.port, kind.port());
            assert!(p.validate().is_ok());
        }
        assert!(!mysql::uses_legacy(&Profile {
            kind: DatabaseKind::PostgreSql,
            driver: mysql::Driver::Legacy,
            ..Default::default()
        }));
    }
    #[test]
    fn identifiers_cannot_break_out_of_quotes() {
        assert_eq!(quote_mysql("a`b"), "`a``b`");
        assert_eq!(quote_pg("a\"b"), "\"a\"\"b\"");
        assert!(validate_name(DatabaseKind::PostgreSql, "bad\nname").is_err());
        assert!(validate_name(DatabaseKind::Redis, "new").is_err());
    }
    #[tokio::test]
    async fn sqlite_connection_operations_and_no_overwrite() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("fixture.db");
        create_sqlite_file(&path).await.unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(create_sqlite_file(&path).await.is_err());
        assert_eq!(before, std::fs::read(&path).unwrap());
        let mut fixture = sqlx::SqliteConnection::connect_with(
            &sqlx::sqlite::SqliteConnectOptions::new().filename(&path),
        )
        .await
        .unwrap();
        sqlx::raw_sql("CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT); CREATE INDEX item_name ON items(name);").execute(&mut fixture).await.unwrap();
        fixture.close().await.unwrap();
        let (session, events) = Session::start(
            Profile {
                kind: DatabaseKind::Sqlite,
                sqlite_path: path.display().to_string(),
                ..defaults(DatabaseKind::Sqlite)
            },
            String::new(),
            false,
        );
        let first = events.recv().await.unwrap();
        assert!(matches!(first, Event::Connected(_)));
        session
            .request(Operation {
                id: 1,
                kind: OperationKind::Objects {
                    database: "fixture.db".into(),
                    cursor: None,
                },
            })
            .unwrap();
        let object = match events.recv().await.unwrap() {
            Event::Operation {
                id: 1,
                result: Ok(OperationResult::Objects { items, .. }),
            } => {
                assert_eq!(items.len(), 1);
                items[0].clone()
            }
            _ => panic!("objects missing"),
        };
        session
            .request(Operation {
                id: 2,
                kind: OperationKind::Details {
                    database: "fixture.db".into(),
                    object,
                },
            })
            .unwrap();
        match events.recv().await.unwrap() {
            Event::Operation {
                id: 2,
                result: Ok(OperationResult::Details(rows)),
            } => {
                assert!(rows.iter().any(|r| r.0.contains("item_name")));
                assert!(rows.iter().any(|r| r.0 == "列 · id"));
            }
            _ => panic!("details missing"),
        }
        drop(session);
        assert!(matches!(events.recv().await.unwrap(), Event::Disconnected));
    }
}

#[cfg(test)]
mod create_charset_tests {
    use super::*;
    #[test]
    fn charset_is_explicit_and_quoted() {
        assert_eq!(
            create_database_sql(DatabaseKind::MySql, "new`db", "gbk").unwrap(),
            "CREATE DATABASE `new``db` CHARACTER SET `gbk`"
        );
        assert!(create_database_sql(DatabaseKind::MySql, "demo", "utf8;DROP").is_err());
        assert_eq!(
            create_database_sql(DatabaseKind::PostgreSql, "demo", "UTF8").unwrap(),
            "CREATE DATABASE \"demo\" ENCODING 'UTF8' TEMPLATE template0"
        );
        assert!(create_database_sql(DatabaseKind::ClickHouse, "demo", "UTF8").is_err());
    }
}

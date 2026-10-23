use crate::DatabaseKind;
#[path = "mysql_auto.rs"]
mod automatic;
use async_channel::{Receiver, Sender};
use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use sqlx::{
    ConnectOptions, Connection as _, Row,
    mysql::{MySqlConnectOptions, MySqlConnection, MySqlSslMode},
};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum Driver {
    #[default]
    Auto,
    Modern,
    Legacy,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum TlsMode {
    #[default]
    VerifyIdentity,
    Required,
    Disabled,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Profile {
    pub driver_id: String,
    pub id: Uuid,
    pub kind: DatabaseKind,
    #[serde(default)]
    pub list_all_databases: bool,
    pub sqlite_path: String,
    pub sqlite_read_only: bool,
    pub redis_cluster: bool,
    pub name: String,
    pub group: String,
    pub environment: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub database: String,
    pub mongo_auth_source: String,
    pub mongo_read_only: bool,
    pub odbc: turbodbn_core::odbc::Options,
    pub driver: Driver,
    pub target_version: String,
    pub charset: String,
    pub mysql_raw_gbk: bool,
    pub tls: TlsMode,
    pub ca: String,
    pub client_cert: String,
    pub client_key: String,
    pub inherit_timeouts: bool,
    pub timeout_seconds: u64,
    pub execution_timeout_seconds: u64,
    pub remember_password: bool,
    pub legacy_library: String,
    pub allow_old_auth: bool,
}
impl Default for Profile {
    fn default() -> Self {
        Self {
            driver_id: String::new(),
            id: Uuid::new_v4(),
            kind: DatabaseKind::MySql,
            list_all_databases: false,
            sqlite_path: String::new(),
            sqlite_read_only: true,
            redis_cluster: false,
            name: "新建 MySQL 连接".into(),
            group: "开发环境".into(),
            environment: "开发".into(),
            host: "127.0.0.1".into(),
            port: 3306,
            user: "root".into(),
            database: String::new(),
            mongo_auth_source: "admin".into(),
            mongo_read_only: true,
            odbc: Default::default(),
            driver: Driver::Auto,
            target_version: "自动检测".into(),
            charset: "utf8mb4".into(),
            mysql_raw_gbk: false,
            tls: TlsMode::VerifyIdentity,
            ca: String::new(),
            client_cert: String::new(),
            client_key: String::new(),
            inherit_timeouts: false,
            timeout_seconds: 5,
            execution_timeout_seconds: turbodbn_core::timeouts::DEFAULT_EXECUTION_SECONDS,
            remember_password: false,
            legacy_library: String::new(),
            allow_old_auth: false,
        }
    }
}
pub(crate) async fn database_names(
    connection: &mut MySqlConnection,
) -> (Vec<String>, Option<String>) {
    let mut databases = Vec::new();
    let mut error = None;
    let mut bytes = 0usize;
    let mut rows = sqlx::raw_sql("SHOW DATABASES").fetch(connection);
    loop {
        match rows.try_next().await {
            Ok(Some(row)) => match crate::database::mysql_text(&row, 0) {
                Ok(name) if !name.is_empty() => {
                    bytes += name.len();
                    if bytes > 16 * 1024 * 1024 {
                        error = Some("browser.response_failed".into());
                        break;
                    }
                    databases.push(name);
                }
                _ => {
                    error = Some("dbwork.detect.name_decode_failed".into());
                    break;
                }
            },
            Ok(None) => break,
            Err(cause) => {
                error = Some(error_message(cause));
                break;
            }
        }
    }
    (databases, error)
}
pub fn valid_charset(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_')
}
impl Profile {
    pub fn execution_timeout(&self) -> Duration {
        Duration::from_secs(turbodbn_core::timeouts::execution_seconds(Some(
            self.execution_timeout_seconds,
        )))
    }
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=turbodbn_core::timeouts::MAX_EXECUTION_SECONDS)
            .contains(&self.execution_timeout_seconds)
        {
            return Err("database.timeout.execution_invalid".into());
        }
        if self.driver_id == "odbc" {
            self.odbc.attributes().map_err(str::to_owned)?;
        }
        if !self.driver_id.is_empty() && self.driver_spec().is_none_or(|d| d.kind != self.kind) {
            return Err("drivers.invalid_identity".into());
        }
        if self.kind == DatabaseKind::External && self.driver_spec().is_none() {
            return Err("drivers.invalid_identity".into());
        }
        if self.name.trim().is_empty()
            || (!self.is_file_database() && self.host.trim().is_empty())
            || (matches!(self.kind, DatabaseKind::MySql | DatabaseKind::PostgreSql)
                && self.user.trim().is_empty())
        {
            return Err("名称、主机和用户名不能为空".into());
        }
        if self.port == 0 || !(1..=60).contains(&self.timeout_seconds) {
            return Err("端口须为 1–65535，超时须为 1–60 秒".into());
        }
        if self.mysql_raw_gbk
            && (self.kind != DatabaseKind::MySql
                || !self.charset.eq_ignore_ascii_case("gbk")
                || (self
                    .target_version
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_digit)
                    && !(self.target_version == "4.1"
                        || self.target_version.starts_with("4.1.")
                        || self.target_version.starts_with("5."))))
        {
            return Err("mysql.gbk.invalid".into());
        }
        if self.charset != "server-default" && !valid_charset(&self.charset) {
            return Err("字符集名称须为 1–64 位英文字母、数字或下划线".into());
        }
        if self.client_cert.is_empty() != self.client_key.is_empty() {
            return Err("客户端证书和私钥必须成对填写".into());
        }
        for value in [
            &self.sqlite_path,
            &self.name,
            &self.group,
            &self.environment,
            &self.host,
            &self.user,
            &self.database,
            &self.mongo_auth_source,
            &self.target_version,
            &self.ca,
            &self.client_cert,
            &self.client_key,
            &self.legacy_library,
        ] {
            if value.len() > 4096 || value.contains('\0') {
                return Err("配置字段过长或包含无效字符".into());
            }
        }
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
struct Profiles {
    version: u32,
    connections: Vec<Profile>,
}
pub fn load_profiles(path: &Path) -> Result<Vec<Profile>, String> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e.to_string()),
    };
    let mut data = vec![];
    file.take(1024 * 1024 + 1)
        .read_to_end(&mut data)
        .map_err(|e| e.to_string())?;
    if data.len() > 1024 * 1024 {
        return Err("连接配置文件超过 1 MiB".into());
    }
    let profiles: Profiles = serde_json::from_slice(&data).map_err(|_| "连接配置文件损坏")?;
    if ![1, 2].contains(&profiles.version) || profiles.connections.len() > 200 {
        return Err("不支持的连接配置版本或数量".into());
    }
    let mut ids = std::collections::HashSet::new();
    for profile in &profiles.connections {
        profile.validate()?;
        if !ids.insert(profile.id) {
            return Err("连接 ID 重复".into());
        }
    }
    Ok(profiles.connections)
}
pub fn save_profiles(path: &Path, connections: Vec<Profile>) -> Result<(), String> {
    let mut ids = std::collections::HashSet::new();
    for profile in &connections {
        profile.validate()?;
        if !ids.insert(profile.id) {
            return Err("连接 ID 重复".into());
        }
    }
    if connections.len() > 200 {
        return Err("最多保存 200 个连接".into());
    }
    let data = serde_json::to_vec_pretty(&Profiles {
        version: if connections.iter().any(|p| p.kind != DatabaseKind::MySql) {
            2
        } else {
            1
        },
        connections,
    })
    .map_err(|e| e.to_string())?;
    if data.len() > 1024 * 1024 {
        return Err("连接配置文件超过 1 MiB".into());
    }
    let parent = path.parent().ok_or("配置目录无效")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    file.write_all(&data).map_err(|e| e.to_string())?;
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    file.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}
pub fn modern_version(version: &str) -> bool {
    if version.to_ascii_lowercase().contains("mariadb") {
        return false;
    }
    let mut parts = version.split('.');
    let major = parts.next().and_then(|s| s.parse::<u32>().ok());
    let minor = parts.next().and_then(|s| s.parse::<u32>().ok());
    matches!(
        (major, minor),
        (Some(5), Some(6 | 7)) | (Some(8 | 9), Some(_))
    )
}
pub fn uses_legacy(profile: &Profile) -> bool {
    profile.kind == DatabaseKind::MySql
        && (profile.mysql_raw_gbk
            || profile.driver == Driver::Legacy
            || (profile.driver == Driver::Auto
                && ["4.0", "4.1", "5.0", "5.1", "5.5", "5.0–5.5"]
                    .contains(&profile.target_version.as_str())))
}
pub fn detected_profile(mut profile: Profile, version: &str) -> Profile {
    if profile.kind == DatabaseKind::MySql && profile.driver == Driver::Auto {
        if profile.mysql_raw_gbk {
            profile.driver = Driver::Legacy;
            profile.target_version = version.to_owned();
            return profile;
        }

        if let Some(target) = automatic::legacy_target(version) {
            profile.driver = Driver::Legacy;
            profile.target_version = target.into();
            if target == "4.0" {
                profile.charset = "server-default".into();
            } else if profile.charset == "utf8mb4" {
                profile.charset = "utf8".into();
            }
        } else {
            profile.driver = Driver::Modern;
        }
    }
    profile
}
pub fn route(profile: &Profile) -> Result<(), String> {
    if profile.kind != DatabaseKind::MySql {
        return crate::database::validate_profile(profile);
    }
    if uses_legacy(profile) {
        return crate::mysql_legacy::validate(profile);
    }
    if profile.charset == "server-default" {
        return Err("现代驱动请选择具体字符集（如 utf8mb4、gbk）".into());
    }
    if ["4.0", "4.1", "5.0", "5.1", "5.5", "5.0–5.5"].contains(&profile.target_version.as_str()) {
        return Err("该目标版本请使用兼容驱动或自动选择".into());
    }
    Ok(())
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TlsIssue {
    Certificate,
    Handshake,
    Unsupported,
    LegacyUnsupported,
    Required,
}
impl TlsIssue {
    pub fn message(self) -> &'static str {
        match self {
            Self::Certificate => "无法验证服务器证书，请检查证书信任、有效期及连接地址",
            Self::Handshake => "TLS 握手失败，可能是协议版本或加密配置不兼容，具体原因尚未确定",
            Self::Unsupported => "服务器未提供 TLS 加密能力",
            Self::LegacyUnsupported => "当前兼容驱动暂不支持 TLS",
            Self::Required => "服务器要求加密连接，不能使用不加密模式",
        }
    }
    pub fn from_message(message: &str) -> Option<Self> {
        [
            Self::Certificate,
            Self::Handshake,
            Self::Unsupported,
            Self::LegacyUnsupported,
            Self::Required,
        ]
        .into_iter()
        .find(|issue| issue.message() == message)
    }
}
pub(crate) fn tls_cause(error: &(dyn std::error::Error + 'static)) -> Option<TlsIssue> {
    if let Some(error) = error.downcast_ref::<rustls::Error>() {
        return Some(if matches!(error, rustls::Error::InvalidCertificate(_)) {
            TlsIssue::Certificate
        } else {
            TlsIssue::Handshake
        });
    }
    if let Some(error) = error.downcast_ref::<std::io::Error>()
        && let Some(inner) = error.get_ref()
    {
        return tls_cause(inner);
    }
    error.source().and_then(tls_cause)
}
fn tls_issue(error: &sqlx::Error) -> Option<TlsIssue> {
    match error {
        sqlx::Error::Tls(inner) if inner.to_string() == "server does not support TLS" => {
            Some(TlsIssue::Unsupported)
        }
        sqlx::Error::Tls(inner) => tls_cause(inner.as_ref()).or(Some(TlsIssue::Handshake)),
        sqlx::Error::Io(inner) => tls_cause(inner),
        _ => None,
    }
}
pub(crate) const SQL_SYNTAX_ERROR: &str = "mysql.error.syntax_1064";
pub(crate) const TABLE_MISSING: &str = "mysql.error.table_missing_1146";

pub(crate) fn statement_error(number: u32) -> Option<&'static str> {
    match number {
        1064 => Some(SQL_SYNTAX_ERROR),
        1146 => Some(TABLE_MISSING),
        1142 | 1143 => Some("mysql.error.command_denied"),
        1227 => Some("mysql.error.privilege_1227"),
        1360 => Some("mysql.error.trigger_missing_1360"),
        1419 => Some("mysql.error.trigger_privilege_1419"),
        _ => None,
    }
}

pub(crate) fn error_message(error: sqlx::Error) -> String {
    if let Some(issue) = tls_issue(&error) {
        return issue.message().into();
    }
    match error {
        sqlx::Error::Database(error) => match error
            .try_downcast_ref::<sqlx::mysql::MySqlDatabaseError>()
            .map(|e| e.number())
        {
            Some(code) if statement_error(u32::from(code)).is_some() => {
                statement_error(u32::from(code)).unwrap().into()
            }
            Some(3159) => TlsIssue::Required.message().into(),
            Some(1045) => "认证失败：请检查用户名、密码与来源主机授权".into(),
            Some(1049) => "默认数据库不存在或不可访问".into(),
            Some(1130) => "服务器拒绝此来源主机，请检查账号的主机授权".into(),
            Some(1044) => "当前账号无权访问默认数据库".into(),
            Some(code) => format!("数据库拒绝请求（错误码 {code}），请检查服务器配置/权限"),
            None => "数据库拒绝请求".into(),
        },
        sqlx::Error::Tls(_) => TlsIssue::Handshake.message().into(),
        sqlx::Error::Io(error) if error.kind() == std::io::ErrorKind::InvalidData => {
            "服务器响应格式或协议异常；不能据此判断为 TLS 问题".into()
        }
        sqlx::Error::Io(_) => "网络连接失败或中断，请检查地址、端口和防火墙".into(),
        sqlx::Error::Protocol(_) => "协议或认证方式不兼容，请核对服务器版本与驱动".into(),
        _ => "连接或能力检测失败，请核对数据库版本、字符集及权限".into(),
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerInfo {
    pub version: String,
    pub session_id: u64,
    pub charset: String,
    pub database: Option<String>,
    pub tls_cipher: String,
    pub elapsed_ms: u128,
    #[serde(default)]
    pub databases: Vec<String>,
    #[serde(default)]
    pub database_list_error: Option<String>,
}
async fn open(profile: &Profile, password: &str) -> Result<(MySqlConnection, ServerInfo), String> {
    profile.validate()?;
    route(profile)?;
    let begin = Instant::now();
    let mut options = MySqlConnectOptions::new()
        .host(&profile.host)
        .port(profile.port)
        .username(&profile.user)
        .charset("utf8")
        .ssl_mode(match profile.tls {
            TlsMode::VerifyIdentity => MySqlSslMode::VerifyIdentity,
            TlsMode::Required => MySqlSslMode::Required,
            TlsMode::Disabled => MySqlSslMode::Disabled,
        })
        .disable_statement_logging();
    if !password.is_empty() {
        options = options.password(password);
    }
    if !profile.database.is_empty() {
        options = options.database(&profile.database);
    }
    if !profile.ca.is_empty() {
        options = options.ssl_ca(PathBuf::from(&profile.ca));
    }
    if !profile.client_cert.is_empty() {
        options = options
            .ssl_client_cert(&profile.client_cert)
            .ssl_client_key(&profile.client_key);
    }
    let mut connection = MySqlConnection::connect_with(&options)
        .await
        .map_err(error_message)?;
    // Select Unicode database names during the UTF-8 handshake, then apply the requested
    // client charset. Metadata temporarily returns UTF-8; restore result charset before use.
    sqlx::query(&format!("SET NAMES {}", profile.charset))
        .execute(&mut connection)
        .await
        .map_err(|_| format!("服务器不支持字符集 {}，请检查字符集名称", profile.charset))?;
    sqlx::query("SET character_set_results = utf8")
        .execute(&mut connection)
        .await
        .map_err(error_message)?;
    let row=sqlx::query("SELECT VERSION() AS version, CONNECTION_ID() AS session_id, @@character_set_client AS charset, DATABASE() AS db")
        .fetch_one(&mut connection).await.map_err(error_message)?;
    let version: String = row.try_get("version").map_err(error_message)?;
    if !modern_version(&version) {
        return Err(format!(
            "检测到 {version}，现代适配器支持的版本系列为 MySQL 5.6/5.7/8.x/9.x；此服务器版本需使用对应适配器或另行兼容验证"
        ));
    }
    let tls = sqlx::query("SHOW SESSION STATUS LIKE 'Ssl_cipher'")
        .fetch_optional(&mut connection)
        .await
        .map_err(error_message)?;
    let tls_cipher = tls
        .map(|row| row.try_get::<String, _>(1))
        .transpose()
        .map_err(error_message)?
        .unwrap_or_default();
    if profile.tls != TlsMode::Disabled && tls_cipher.is_empty() {
        return Err(TlsIssue::Handshake.message().into());
    }
    let (databases, database_list_error) =
        if profile.database.is_empty() || profile.list_all_databases {
            database_names(&mut connection).await
        } else {
            (vec![profile.database.clone()], None)
        };
    sqlx::query(&format!("SET character_set_results = {}", profile.charset))
        .execute(&mut connection)
        .await
        .map_err(error_message)?;
    let info = ServerInfo {
        version,
        databases,
        database_list_error,
        session_id: row.try_get("session_id").map_err(error_message)?,
        charset: row.try_get("charset").map_err(error_message)?,
        database: row.try_get("db").map_err(error_message)?,
        tls_cipher,
        elapsed_ms: begin.elapsed().as_millis(),
    };
    Ok((connection, info))
}
pub enum Event {
    Connected(ServerInfo),
    Tested(ServerInfo),
    Failed(String),
    Disconnected,
    Operation {
        id: u64,
        result: Result<crate::database::OperationResult, String>,
    },
}
pub struct Session {
    stop: Sender<()>,
    commands: Sender<crate::database::Operation>,
}
impl Drop for Session {
    fn drop(&mut self) {
        self.stop.close();
    }
}
impl Session {
    pub fn start(profile: Profile, password: String, test_only: bool) -> (Self, Receiver<Event>) {
        let profile = profile.resolve_timeouts();
        if profile
            .driver_spec()
            .is_some_and(|d| !crate::driver_packages::enabled(d.id))
        {
            let (stop, _) = async_channel::bounded(1);
            let (commands, _) = async_channel::bounded(1);
            let (send, output) = async_channel::bounded(1);
            let _ = send.try_send(Event::Failed("components.disabled".into()));
            return (Self { stop, commands }, output);
        }
        // Workspace builds may enable another provider through isolated drivers.
        // Keep existing connection TLS deterministic instead of relying on feature inference.
        let _ = rustls::crypto::ring::default_provider().install_default();
        if profile.kind == DatabaseKind::External {
            return crate::external_driver::start(profile, password, test_only);
        }
        if profile.kind != DatabaseKind::MySql {
            return crate::database::start(profile, password, test_only);
        }
        if profile.driver == Driver::Auto && profile.target_version == "自动检测" {
            return automatic::start(profile, password, test_only);
        }
        if uses_legacy(&profile) {
            return crate::mysql_legacy::start(profile, password, test_only);
        }
        let (stop, rx) = async_channel::bounded(1);
        let (commands, requests) = async_channel::bounded::<crate::database::Operation>(8);
        let (events, output) = async_channel::bounded(8);
        std::thread::spawn(move || {
            let mut profile = profile;
            let failure_events = events.clone();
            let task_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                match runtime {
                Ok(runtime)=>runtime.block_on(async{
                    let duration=Duration::from_secs(profile.timeout_seconds.clamp(1,60));
                    let execution_timeout=profile.execution_timeout();
                    let result=tokio::select!{
                        _=rx.recv()=>return,
                        result=tokio::time::timeout(duration,open(&profile,&password))=>result.unwrap_or_else(|_|Err("连接测试超时，已取消".into())),
                    };
                    drop(password);
                    match result{
                        Err(error)=>{let _=events.send(Event::Failed(error)).await;}
                        Ok((connection,info)) if test_only=>{drop(connection);let _=events.send(Event::Tested(info)).await;}
                        Ok((mut connection,info))=>{
                            profile.target_version=info.version.clone();
                            if events.send(Event::Connected(info)).await.is_err(){return;}
                            let mut script_session=false;
                            let mut heartbeat=tokio::time::interval(Duration::from_secs(30));heartbeat.tick().await;
                            loop {tokio::select!{
                                _=rx.recv()=>break,
                                request=requests.recv()=>{
                                    let Ok(request)=request else {break;};let id=request.id;
                                    script_session|=matches!(&request.kind,crate::database::OperationKind::Workbench(r) if matches!(r.action,crate::database::workbench::Action::ScriptPrepare{..}|crate::database::workbench::Action::ScriptExecute{..}));
                                    let result=tokio::select!{
                                        _=rx.recv()=>break,
                                        result=tokio::time::timeout(execution_timeout,crate::database::mysql_operation(&mut connection,&profile,&request))=>result,
                                    };
                                    match result {Ok(result)=>{let _=events.send(Event::Operation{id,result}).await;},Err(_)=>{
                                        let message=if script_session{"script.error.timeout"}else{"操作超时；建库结果可能已生效，请重连后确认，勿重复提交"};
                                        let _=events.send(Event::Operation{id,result:Err(message.into())}).await;break;
                                    }}
                                }
                                _=heartbeat.tick()=>{
                                    let result=tokio::select!{_=rx.recv()=>break,result=tokio::time::timeout(Duration::from_secs(5),connection.ping())=>result};
                                    if !matches!(result,Ok(Ok(()))){let _=events.send(Event::Failed("连接已失效，请重新连接".into())).await;break;}
                                }
                            }}
                            if script_session {
                                let _=tokio::time::timeout(Duration::from_secs(3),async{
                                    if sqlx::raw_sql("ROLLBACK").execute(&mut connection).await.is_ok(){
                                        let _=sqlx::raw_sql("UNLOCK TABLES").execute(&mut connection).await;
                                    }
                                }).await;
                            }
                            drop(connection);let _=events.send(Event::Disconnected).await;
                        }
                    }
                }),
                Err(_)=>{let _=events.send_blocking(Event::Failed("无法启动数据库后台任务".into()));}
            }
            }));
            if task_result.is_err() {
                let _ = failure_events.send_blocking(Event::Failed(
                    "数据库适配器异常，连接已关闭；请检查驱动与服务端兼容性".into(),
                ));
            }
        });
        (Self { stop, commands }, output)
    }
}
impl Session {
    pub(crate) fn from_channels(
        stop: Sender<()>,
        commands: Sender<crate::database::Operation>,
    ) -> Self {
        Self { stop, commands }
    }
    pub fn request(&self, request: crate::database::Operation) -> Result<(), String> {
        self.commands
            .try_send(request)
            .map_err(|_| "连接正忙或已断开，请稍后重试".into())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_profiles_keep_connect_timeout_and_receive_separate_execution_default() {
        let p: Profile = serde_json::from_str(r#"{"timeout_seconds":7}"#).unwrap();
        assert_eq!(p.timeout_seconds, 7);
        assert_eq!(p.execution_timeout_seconds, 300);
        let mut p = p;
        p.execution_timeout_seconds = 3600;
        p.validate().unwrap();
        assert_eq!(
            serde_json::from_str::<Profile>(&serde_json::to_string(&p).unwrap())
                .unwrap()
                .execution_timeout_seconds,
            3600
        );
        p.execution_timeout_seconds = 0;
        assert!(p.validate().is_err());
    }
    #[test]
    fn trigger_server_codes_are_safe_actionable_categories() {
        assert_eq!(statement_error(1142), Some("mysql.error.command_denied"));
        assert_eq!(statement_error(1227), Some("mysql.error.privilege_1227"));
        assert_eq!(
            statement_error(1360),
            Some("mysql.error.trigger_missing_1360")
        );
        assert_eq!(
            statement_error(1419),
            Some("mysql.error.trigger_privilege_1419")
        );
    }
    #[test]
    fn mongodb_auth_source_retains_legacy_default_and_round_trips() {
        let old: Profile = serde_json::from_str(
            r#"{"driver_id":"mongodb","kind":"External","database":"business"}"#,
        )
        .unwrap();
        assert_eq!(old.mongo_auth_source, "admin");
        let profile = Profile {
            mongo_auth_source: "account_auth".into(),
            ..old
        };
        let restored: Profile =
            serde_json::from_str(&serde_json::to_string(&profile).unwrap()).unwrap();
        assert_eq!(restored.mongo_auth_source, "account_auth");
        assert_eq!(restored.database, "business");
    }
    #[test]
    fn metadata_text_decodes_binary_flagged_utf8_without_lossy_fallback() {
        fn packet(socket: &mut std::net::TcpStream, seq: u8, bytes: &[u8]) {
            let length = (bytes.len() as u32).to_le_bytes();
            socket
                .write_all(&[length[0], length[1], length[2], seq])
                .unwrap();
            socket.write_all(bytes).unwrap();
        }
        fn read_packet(socket: &mut std::net::TcpStream) -> std::io::Result<Vec<u8>> {
            let mut header = [0; 4];
            socket.read_exact(&mut header)?;
            let length = u32::from_le_bytes([header[0], header[1], header[2], 0]) as usize;
            assert!(length < 65536);
            let mut bytes = vec![0; length];
            socket.read_exact(&mut bytes)?;
            Ok(bytes)
        }
        for value in ["测试库".as_bytes().to_vec(), vec![0xff]] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let sent = value.clone();
            let server = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let capabilities = 0x0008_a201u32;
                let mut greeting = b"\x0a8.4.5\0".to_vec();
                greeting.extend_from_slice(&1u32.to_le_bytes());
                greeting.extend_from_slice(b"12345678\0");
                greeting.extend_from_slice(&(capabilities as u16).to_le_bytes());
                greeting.push(33);
                greeting.extend_from_slice(&2u16.to_le_bytes());
                greeting.extend_from_slice(&((capabilities >> 16) as u16).to_le_bytes());
                greeting.push(21);
                greeting.extend_from_slice(&[0; 10]);
                greeting.extend_from_slice(b"abcdefghijkl\0mysql_native_password\0");
                packet(&mut socket, 0, &greeting);
                read_packet(&mut socket).unwrap();
                packet(&mut socket, 2, &[0, 0, 0, 2, 0, 0, 0]);
                while let Ok(command) = read_packet(&mut socket) {
                    if command.first() == Some(&1) {
                        break;
                    }
                    if command.get(1..) == Some(b"SHOW DATABASES".as_slice()) {
                        packet(&mut socket, 1, &[1]);
                        let mut column = Vec::new();
                        for field in ["def", "", "", "", "Database", "Database"] {
                            column.push(field.len() as u8);
                            column.extend_from_slice(field.as_bytes());
                        }
                        column.push(12);
                        column.extend_from_slice(&33u16.to_le_bytes());
                        column.extend_from_slice(&256u32.to_le_bytes());
                        column.push(0xfd); // VAR_STRING with the BINARY metadata flag.
                        column.extend_from_slice(&0x80u16.to_le_bytes());
                        column.extend_from_slice(&[0; 3]);
                        packet(&mut socket, 2, &column);
                        packet(&mut socket, 3, &[0xfe, 0, 0, 2, 0]);
                        let mut row = vec![sent.len() as u8];
                        row.extend_from_slice(&sent);
                        let count = if std::str::from_utf8(&sent).is_ok() {
                            640
                        } else {
                            1
                        };
                        for i in 0..count {
                            packet(&mut socket, (4 + i) as u8, &row);
                        }
                        packet(&mut socket, (count + 4) as u8, &[0xfe, 0, 0, 2, 0]);
                    } else {
                        // SQLx connection initialization only changes this test session.
                        packet(&mut socket, 1, &[0, 0, 0, 2, 0, 0, 0]);
                    }
                }
            });
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    let options = MySqlConnectOptions::new()
                        .host("127.0.0.1")
                        .port(port)
                        .username("fixture")
                        .ssl_mode(MySqlSslMode::Disabled)
                        .charset("utf8")
                        .disable_statement_logging();
                    let mut connection = MySqlConnection::connect_with(&options).await.unwrap();
                    let row = sqlx::raw_sql("SHOW DATABASES")
                        .fetch_one(&mut connection)
                        .await
                        .unwrap();
                    assert!(
                        row.try_get::<String, _>(0).is_err(),
                        "reproduce the original type mismatch"
                    );
                    let decoded = crate::database::mysql_text(&row, 0);
                    if let Ok(expected) = std::str::from_utf8(&value) {
                        assert_eq!(decoded.unwrap(), expected);
                    } else {
                        assert!(decoded.is_err(), "invalid UTF-8 must still fail");
                    }
                    // Drain the first response, then exercise the same reader used by connect and refresh.
                    let (names, error) = database_names(&mut connection).await;
                    if std::str::from_utf8(&value).is_ok() {
                        assert!(error.is_none());
                        assert_eq!(names.len(), 640);
                    } else {
                        assert!(error.is_some());
                    }
                    connection.close().await.unwrap();
                });
            server.join().unwrap();
        }
    }
    #[test]
    fn tls_failures_are_typed_without_exposing_certificate_contents() {
        let error = sqlx::Error::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            rustls::Error::InvalidCertificate(rustls::CertificateError::UnknownIssuer),
        ));
        assert_eq!(
            TlsIssue::from_message(&error_message(error)),
            Some(TlsIssue::Certificate)
        );
        assert_eq!(
            tls_issue(&sqlx::Error::Tls("server does not support TLS".into())),
            Some(TlsIssue::Unsupported)
        );
        assert_eq!(tls_issue(&sqlx::Error::Protocol("bad packet".into())), None);
    }
    #[test]
    fn charset_names_accept_custom_values_but_never_sql() {
        for name in ["gbk", "GB2312", "utf8mb4", "latin1", "custom_charset"] {
            assert!(
                Profile {
                    charset: name.into(),
                    ..Default::default()
                }
                .validate()
                .is_ok()
            );
        }
        for name in ["", "gbk; DROP DATABASE x", "gbk'", "gbk --", "utf-8"] {
            assert!(
                Profile {
                    charset: name.into(),
                    ..Default::default()
                }
                .validate()
                .is_err()
            );
        }
    }
    #[test]
    fn invalid_data_is_not_misclassified_as_tls() {
        let message = error_message(sqlx::Error::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "private detail",
        )));
        assert!(TlsIssue::from_message(&message).is_none());
        assert!(!message.contains("private detail"));
    }
    #[test]
    fn cancellation_stops_waiting_for_server_handshake() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (session, events) = Session::start(
            Profile {
                port,
                tls: TlsMode::Disabled,
                timeout_seconds: 30,
                ..Default::default()
            },
            String::new(),
            false,
        );
        drop(session);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        assert!(
            runtime
                .block_on(async {
                    tokio::time::timeout(Duration::from_secs(2), async move { events.recv().await })
                        .await
                })
                .is_ok()
        );
        drop(listener);
    }
    #[test]
    fn version_routing_does_not_claim_legacy_support() {
        for v in [
            "5.6.51",
            "5.7.44-log",
            "8.0.32",
            "8.4.5",
            "8.4.5-commercial",
            "8.9.0",
            "9.0.1",
            "9.5.0",
        ] {
            assert!(modern_version(v));
        }
        for v in [
            "4.0.30",
            "4.1.22",
            "5.5.62",
            "10.6.0-MariaDB",
            "10.0.0",
            "8.4.5-MariaDB",
            "8.invalid",
            "garbage",
        ] {
            assert!(!modern_version(v));
        }
        let p = Profile {
            target_version: "4.1".into(),
            ..Default::default()
        };
        assert!(route(&p).is_err());
    }
    #[test]
    fn profiles_round_trip_without_password() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.json");
        let profile = Profile::default();
        save_profiles(&path, vec![profile.clone()]).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("\"password\""));
        assert_eq!(load_profiles(&path).unwrap(), vec![profile]);
    }
    #[test]
    fn validation_rejects_invalid_configuration() {
        let mut p = Profile {
            port: 0,
            ..Default::default()
        };
        assert!(p.validate().is_err());
        p.port = 3306;
        p.client_cert = "cert.pem".into();
        assert!(p.validate().is_err());
    }
}

//! Database contracts and the concrete Codex App Server adapter.
pub mod agent;
pub mod claude;
pub mod codex;
pub mod codex_discovery;
pub mod codex_install;
pub mod connection_url;
pub mod credentials;
pub mod database;
pub mod driver_catalog;
pub mod driver_packages;
pub mod external_driver;
pub mod mysql;
pub mod mysql_drivers;
pub mod mysql_legacy;
pub mod oracle_bundle;
pub mod password_protection;
pub mod updates;
use std::{future::Future, pin::Pin, sync::Arc};
use turbodbn_core::QueryToken;
use uuid::Uuid;

pub type ServiceFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, ServiceError>> + Send + 'a>>;

#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error("服务尚未配置")]
    NotConfigured,
    #[error("请求已取消")]
    Cancelled,
    #[error("请求超时")]
    Timeout,
    #[error("服务异常：{0}")]
    Backend(String),
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum DatabaseKind {
    PostgreSql,
    #[default]
    MySql,
    Sqlite,
    ClickHouse,
    Redis,
    External,
}
impl DatabaseKind {
    pub const ALL: [Self; 5] = [
        Self::MySql,
        Self::Sqlite,
        Self::PostgreSql,
        Self::ClickHouse,
        Self::Redis,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::MySql => "MySQL",
            Self::Sqlite => "SQLite",
            Self::PostgreSql => "PostgreSQL",
            Self::ClickHouse => "ClickHouse",
            Self::Redis => "Redis",
            Self::External => "External",
        }
    }
    pub fn port(self) -> u16 {
        match self {
            Self::MySql => 3306,
            Self::Sqlite => 1,
            Self::PostgreSql => 5432,
            Self::ClickHouse => 8443,
            Self::Redis => 6379,
            Self::External => 1,
        }
    }
    pub fn can_create_database(self) -> bool {
        matches!(self, Self::MySql | Self::PostgreSql | Self::ClickHouse)
    }
}

/// No credentials: resolve a connection ID inside the adapter, never inside a prompt.
#[derive(Debug, Clone)]
pub struct QueryRequest {
    pub token: QueryToken,
    pub connection_id: Uuid,
    pub database: Option<String>,
    pub sql: Arc<str>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Text(Arc<str>),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    /// Preserve decimal precision; never convert money to f64.
    Decimal(Arc<str>),
    Bytes(Arc<[u8]>),
}

#[derive(Debug, Clone)]
pub struct RowBatch {
    pub token: QueryToken,
    pub columns: Arc<[String]>,
    pub rows: Arc<[Vec<Value>]>,
}

/// Pull-based boundary supplies natural backpressure. Implementations must enforce byte budgets.
pub trait QueryCursor: Send {
    fn next_batch(&mut self) -> ServiceFuture<'_, Option<RowBatch>>;
    fn cancel(&mut self) -> ServiceFuture<'_, ()>;
}

pub trait DatabaseService: Send + Sync {
    fn execute(&self, request: QueryRequest) -> ServiceFuture<'_, Box<dyn QueryCursor>>;
}

/// Explicitly selected, redacted material only. No full connection configuration.
#[derive(Debug, Clone, Default)]
pub struct AiContext {
    pub database_kind: Option<DatabaseKind>,
    pub selected_sql: Option<Arc<str>>,
    pub selected_ddl: Option<Arc<str>>,
    pub result_summary: Option<Arc<str>>,
}

#[derive(Debug, Clone)]
pub enum AiEvent {
    TextDelta { item_id: String, text: String },
    TextSnapshot { item_id: String, text: String },
    Completed,
    Failed(String),
}

pub trait AiSession: Send {
    fn next_event(&mut self) -> ServiceFuture<'_, Option<AiEvent>>;
    fn cancel(&mut self) -> ServiceFuture<'_, ()>;
}

pub trait AiService: Send + Sync {
    fn start(&self, prompt: String, context: AiContext) -> ServiceFuture<'_, Box<dyn AiSession>>;
}

pub mod note_sql_export;
pub mod note_store;
pub mod sql_script;

pub mod sql_mcp;

pub mod db2_bundle;

pub mod access_mode;

pub mod timeout_policy;

mod bundle_download;

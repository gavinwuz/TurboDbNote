//! Database contracts and the concrete Codex App Server adapter.
pub mod agent;
pub mod claude;
pub mod codex;
pub mod codex_discovery;
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

#[derive(Debug, Clone, Copy)]
pub enum DatabaseKind {
    PostgreSql,
    MySql,
    Sqlite,
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

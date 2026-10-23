//! Notebook SQL runs through the existing adapters, with a frozen preparation and bounded results.
use super::workbench::{self, Access, Object, Rows};
use crate::DatabaseKind;
use serde::{Deserialize, Serialize};
use sqlparser::ast::Spanned;
use sqlparser::{
    ast::{self, Expr, LimitClause, Statement, Value, Visit, Visitor},
    dialect::{Dialect, MySqlDialect, PostgreSqlDialect, SQLiteDialect},
    parser::Parser,
};
use std::{collections::BTreeSet, ops::ControlFlow, time::Instant};

#[path = "notebook_grid.rs"]
mod result_grid;
pub use result_grid::{GridAction, GridSource, grid_request, query_table_name};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScriptDiagnostics {
    pub version: String,
    pub account: String,
    pub database: String,
    pub binary_log: String,
    pub trust_creators: String,
    pub grants_available: bool,
    pub direct_privileges: Vec<String>,
}
pub async fn script_diagnostics(
    c: &mut impl Access,
    kind: DatabaseKind,
) -> Result<ScriptDiagnostics, String> {
    if kind != DatabaseKind::MySql {
        return Err("script.diagnostics.unsupported".into());
    }
    let rows = c
        .query("SELECT VERSION(), CURRENT_USER(), DATABASE()")
        .await?;
    let row = rows
        .values
        .first()
        .filter(|r| r.len() == 3)
        .ok_or("script.error.execution")?;
    let value = |i: usize| {
        row[i]
            .clone()
            .unwrap_or_default()
            .chars()
            .filter(|c| !c.is_control())
            .take(512)
            .collect::<String>()
    };
    let variables = c
        .query("SELECT @@GLOBAL.log_bin, @@GLOBAL.log_bin_trust_function_creators")
        .await
        .ok();
    let variable = |i: usize| {
        variables
            .as_ref()
            .and_then(|r| r.values.first())
            .and_then(|r| r.get(i))
            .and_then(Option::as_ref)
            .filter(|v| v.len() <= 8 && v.bytes().all(|b| b.is_ascii_alphanumeric()))
            .cloned()
            .unwrap_or_default()
    };
    let grants = c.query("SHOW GRANTS").await;
    let mut direct_privileges = Vec::new();
    if let Ok(grants) = &grants {
        for row in &grants.values {
            if let Some(text) = row.first().and_then(Option::as_deref) {
                // Never return grant text: older servers can include authentication material.
                if let Some((privileges, _)) = text.split_once(" ON ")
                    && let Some(privileges) = privileges.strip_prefix("GRANT ")
                {
                    for privilege in privileges.split(',').map(str::trim) {
                        if [
                            "ALL PRIVILEGES",
                            "TRIGGER",
                            "SUPER",
                            "SYSTEM_USER",
                            "SET_USER_ID",
                            "SYSTEM_VARIABLES_ADMIN",
                        ]
                        .contains(&privilege)
                            && !direct_privileges.iter().any(|v| v == privilege)
                        {
                            direct_privileges.push(privilege.to_owned());
                        }
                    }
                }
            }
        }
    }
    Ok(ScriptDiagnostics {
        version: value(0),
        account: value(1),
        database: value(2),
        binary_log: variable(0),
        trust_creators: variable(1),
        grants_available: grants.is_ok(),
        direct_privileges,
    })
}
/// In-memory identity only; never persisted or used to authorize different SQL.
pub fn script_source_fingerprint(sql: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(sql.trim().as_bytes()).into()
}
pub const DEFAULT_PAGE_SIZE: usize = 20;
pub const SCRIPT_MAX_BYTES: usize = workbench::script_context::MAX_BYTES;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Plan {
    pub sql: String,
    pub read_only: bool,
    pub confirm: bool,
    pub guards: Vec<Guard>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Guard {
    pub object: Object,
    pub stamp: Option<String>,
}
pub fn guard_hash(guard: &Guard) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    digest.update([u8::from(guard.stamp.is_some())]);
    if let Some(stamp) = &guard.stamp {
        digest.update(stamp.as_bytes());
    }
    digest.finalize().into()
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResultPage {
    #[serde(default)]
    pub execution: Option<Box<ExecutionSummary>>,
    #[serde(default)]
    pub affected_rows: Option<u64>,
    #[serde(default)]
    pub grid: Option<GridSource>,
    pub rows: Rows,
    pub more: bool,
    pub offset: usize,
    pub elapsed_ms: u64,
    pub read_only: bool,
}
/// Execution metadata only: never include SQL, credentials or raw server errors.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ExecutionSummary {
    pub operation: String,
    pub status: String,
    pub affected_rows: Option<u64>,
    pub matched_rows: Option<u64>,
    pub changed_rows: Option<u64>,
    pub elapsed_ms: u64,
    pub count_source: Option<String>,
    #[serde(default)]
    pub objects: Vec<Object>,
    #[serde(default)]
    pub created_tables: Option<u64>,
    #[serde(default)]
    pub skipped_tables: Option<u64>,
    #[serde(default)]
    pub mixed_write: bool,
    #[serde(default)]
    pub table_create: bool,
}
impl ExecutionSummary {
    pub fn json_rows(&self) -> Rows {
        Rows {
            columns: vec!["execution".into()],
            values: vec![vec![Some(serde_json::to_string(self).unwrap_or_default())]],
        }
    }
}
fn parse(kind: DatabaseKind, sql: &str) -> Result<Statement, String> {
    parse_for(kind, sql, false)
}
fn parse_for(kind: DatabaseKind, sql: &str, script: bool) -> Result<Statement, String> {
    if sql.trim().is_empty()
        || sql.len()
            > if script {
                workbench::script_context::MAX_BYTES
            } else {
                60_000
            }
        || sql.contains('\0')
        || !script && sql.contains("/*!")
        || sql.contains("/*M!")
    {
        return Err("SQL 为空、超过 60 KB 或包含不支持的执行注释".into());
    }
    let dialect: Box<dyn Dialect> = match kind {
        DatabaseKind::MySql => Box::new(MySqlDialect {}),
        DatabaseKind::PostgreSql => Box::new(PostgreSqlDialect {}),
        DatabaseKind::Sqlite => Box::new(SQLiteDialect {}),
        _ => return Err("笔记 SQL 当前支持 MySQL、PostgreSQL 与 SQLite".into()),
    };
    let mut statements = Parser::new(dialect.as_ref())
        .with_recursion_limit(64)
        .try_with_sql(sql)
        .and_then(|mut p| p.parse_statements())
        .map_err(|_| "SQL 语法或复杂度暂不受支持，请检查当前数据库方言")?;
    if statements.len() != 1 {
        return Err("每个 SQL 子区域一次执行一条语句，请将多条命令拆分到不同区域".into());
    }
    Ok(statements.remove(0))
}
#[derive(Default)]
struct Inspection {
    postgres: bool,
    functions: bool,
    writes: bool,
    blocked: bool,
    tables: BTreeSet<Vec<String>>,
}
impl Visitor for Inspection {
    type Break = ();
    fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<()> {
        if let Expr::Function(function) = expr {
            // These built-ins cannot write data. Unknown/stored/UDF calls on legacy
            // servers still require confirmation because no read-only transaction exists.
            self.functions |= !matches!(
                function.name.to_string().to_ascii_uppercase().as_str(),
                "COUNT"
                    | "SUM"
                    | "AVG"
                    | "MIN"
                    | "MAX"
                    | "ABS"
                    | "ROUND"
                    | "COALESCE"
                    | "IFNULL"
                    | "NULLIF"
                    | "LOWER"
                    | "UPPER"
                    | "LENGTH"
                    | "CONCAT"
                    | "SUBSTRING"
                    | "TRIM"
                    | "NOW"
                    | "CURDATE"
                    | "DATE_FORMAT"
            );
        }
        ControlFlow::Continue(())
    }
    fn pre_visit_statement(&mut self, s: &Statement) -> ControlFlow<()> {
        if !matches!(s, Statement::Query(_)) {
            self.writes = true;
        }
        ControlFlow::Continue(())
    }
    fn pre_visit_select(&mut self, s: &ast::Select) -> ControlFlow<()> {
        if s.into.is_some() {
            self.blocked = true;
        }
        ControlFlow::Continue(())
    }
    fn pre_visit_query(&mut self, q: &ast::Query) -> ControlFlow<()> {
        if !q.locks.is_empty()
            || q.settings.is_some()
            || q.format_clause.is_some()
            || q.for_clause.is_some()
        {
            self.blocked = true;
        }
        ControlFlow::Continue(())
    }
    fn pre_visit_relation(&mut self, name: &ast::ObjectName) -> ControlFlow<()> {
        let parts = name
            .0
            .iter()
            .map(|p| match p {
                ast::ObjectNamePart::Identifier(i) => {
                    Some(if self.postgres && i.quote_style.is_none() {
                        i.value.to_lowercase()
                    } else {
                        i.value.clone()
                    })
                }
                _ => None,
            })
            .collect::<Option<Vec<_>>>();
        if let Some(parts) = parts {
            self.tables.insert(parts);
        } else {
            self.blocked = true;
        }
        ControlFlow::Continue(())
    }
}
fn inspect(s: &Statement, kind: DatabaseKind) -> Result<Inspection, String> {
    if let Statement::Explain {
        analyze,
        statement,
        options,
        ..
    } = s
    {
        if *analyze
            || options.as_ref().is_some_and(|options| {
                options
                    .iter()
                    .any(|option| option.name.value.eq_ignore_ascii_case("ANALYZE"))
            })
        {
            return Err("notes.explain.analyze_unsupported".into());
        }
        if !matches!(statement.as_ref(), Statement::Query(_)) {
            return Err("notes.explain.query_only".into());
        }
        if options.as_ref().is_some_and(|options| {
            options.iter().any(|option| {
                !matches!(
                    option.name.value.to_ascii_uppercase().as_str(),
                    "FORMAT" | "VERBOSE" | "COSTS" | "SETTINGS" | "GENERIC_PLAN"
                )
            })
        }) {
            return Err("notes.explain.options_unsupported".into());
        }
        return inspect(statement, kind);
    }
    let mut info = Inspection {
        postgres: kind == DatabaseKind::PostgreSql,
        ..Default::default()
    };
    let _ = s.visit(&mut info);
    let supported = matches!(
        s,
        Statement::Query(_)
            | Statement::Insert(_)
            | Statement::Update(_)
            | Statement::Delete(_)
            | Statement::CreateTable(_)
            | Statement::AlterTable(_)
            | Statement::Truncate(_)
            | Statement::Drop {
                object_type: ast::ObjectType::Table,
                ..
            }
    );
    if !supported || info.blocked || info.tables.len() > 16 {
        return Err("notes.sql_scope_unsupported".into());
    }
    if matches!(s, Statement::Query(_)) && info.writes {
        return Err("分页查询不能包含写入型 CTE".into());
    }
    Ok(info)
}
pub fn validate_script_statement(kind: DatabaseKind, sql: &str) -> Result<bool, String> {
    let expanded;
    let sql = if kind == DatabaseKind::MySql {
        expanded = workbench::script_context::expand(sql, None)?;
        &expanded
    } else {
        sql
    };
    if kind == DatabaseKind::MySql
        && (workbench::script_context::keys_target(sql)?.is_some()
            || workbench::script_context::drop_trigger_target(sql)?.is_some())
    {
        return Ok(false);
    }
    if kind == DatabaseKind::MySql
        && let Some(control) = workbench::script_context::control(sql)?
    {
        return Ok(!control.confirm());
    }
    if kind == DatabaseKind::MySql
        && let Some((inspection, _)) = workbench::script_context::variable_select(sql)?
    {
        let info = inspect(&parse_for(kind, &inspection, true)?, kind)?;
        if info.functions || info.writes {
            return Err("script.error.control".into());
        }
        return Ok(false);
    }
    if mysql_metadata(kind, sql) {
        return Ok(true);
    }
    let statement = parse_for(kind, sql, true)?;
    inspect(&statement, kind).map(|inspection| !inspection.writes)
}
/// Explicit table-view entry may refresh a plain read, never a changed write,
/// locking read, multi-statement script or unreviewed function call.
pub fn automatic_table_read(kind: DatabaseKind, sql: &str) -> bool {
    command_required(kind, sql) == Some("select")
        && script_requires_confirmation(kind, sql, true) == Ok(false)
}

pub fn script_requires_confirmation(
    kind: DatabaseKind,
    sql: &str,
    legacy: bool,
) -> Result<bool, String> {
    if !validate_script_statement(kind, sql)? {
        return Ok(true);
    }
    if (legacy || kind == DatabaseKind::MySql) && !mysql_metadata(kind, sql) {
        let expanded = workbench::script_context::expand(sql, None)?;
        if workbench::script_context::control(&expanded)?.is_some() {
            return Ok(false);
        }
        return inspect(&parse_for(kind, &expanded, true)?, kind).map(|info| info.functions);
    }
    Ok(false)
}
pub fn validate_script_target(kind: DatabaseKind, sql: &str, database: &str) -> Result<(), String> {
    workbench::script_context::validate_target(kind, sql, database)?;
    if kind == DatabaseKind::MySql {
        let expanded = workbench::script_context::expand(sql, None)?;
        if let Some(parts) = workbench::script_context::drop_trigger_target(&expanded)?
            .or(workbench::script_context::keys_target(&expanded)?)
        {
            object(kind, database, &parts).map_err(|_| "script.error.database_mismatch")?;
            return Ok(());
        }
        if let Some((inspection, _)) = workbench::script_context::variable_select(&expanded)? {
            let info = inspect(&parse_for(kind, &inspection, true)?, kind)?;
            if info.functions || info.writes {
                return Err("script.error.control".into());
            }
            for parts in info.tables {
                object(kind, database, &parts).map_err(|_| "script.error.database_mismatch")?;
            }
            return Ok(());
        }
        if workbench::script_context::control(&expanded)?.is_none() {
            if mysql_metadata(kind, &expanded) {
                return Ok(());
            }
            let statement = parse_for(kind, &expanded, true)?;
            for parts in inspect(&statement, kind)?.tables {
                object(kind, database, &parts).map_err(|_| "script.error.database_mismatch")?;
            }
        }
    }
    Ok(())
}
fn object(kind: DatabaseKind, database: &str, parts: &[String]) -> Result<Object, String> {
    let (schema, name) = match parts {
        [name] => (
            if kind == DatabaseKind::PostgreSql {
                "public"
            } else {
                ""
            }
            .to_owned(),
            name.clone(),
        ),
        [schema, name] if kind == DatabaseKind::PostgreSql => (schema.clone(), name.clone()),
        [schema, name]
            if (kind == DatabaseKind::MySql && schema == database)
                || (kind == DatabaseKind::Sqlite && schema == "main") =>
        {
            (String::new(), name.clone())
        }
        _ => return Err("写入范围必须在选定数据库内，请使用明确的表名或 schema.table".into()),
    };
    Ok(Object {
        schema,
        name,
        kind: "table".into(),
        comment: String::new(),
        parent: String::new(),
    })
}
pub(super) async fn resolved_object(
    c: &mut impl Access,
    kind: DatabaseKind,
    database: &str,
    parts: &[String],
) -> Result<Object, String> {
    let mut o = object(kind, database, parts)?;
    if kind == DatabaseKind::PostgreSql && parts.len() == 1 {
        let name = workbench::literal_for(kind, &workbench::quote(kind, &o.name));
        let rows=c.query(&format!("SELECT COALESCE((SELECT n.nspname FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE c.oid=to_regclass({name})),current_schema())")).await?;
        o.schema = rows
            .values
            .first()
            .and_then(|r| r.first())
            .and_then(Clone::clone)
            .ok_or("无法确定目标 schema")?;
    }
    Ok(o)
}
async fn stamp(
    c: &mut impl Access,
    kind: DatabaseKind,
    database: &str,
    o: &Object,
) -> Result<Option<String>, String> {
    if kind == DatabaseKind::MySql {
        // SHOW TABLES ... LIKE accepts a string literal, not CONVERT(...).
        // Read the exact object's DDL directly; never interpret wildcards or
        // depend on sql_mode, database size, or client-side name case folding.
        let target = workbench::qualified(kind, database, o);
        return match c.query(&format!("SHOW CREATE TABLE {target}")).await {
            Ok(rows) => rows
                .values
                .first()
                .and_then(|row| row.get(1))
                .and_then(Clone::clone)
                .filter(|ddl| !ddl.is_empty())
                .map(Some)
                .ok_or_else(|| "notes.schema_check_invalid".into()),
            Err(error) if error == crate::mysql::TABLE_MISSING => Ok(None),
            Err(error) => Err(error),
        };
    }
    let literal = workbench::literal_for(kind, &o.name);
    let sql = match kind {
        DatabaseKind::Sqlite => format!(
            "SELECT sql FROM sqlite_schema WHERE name={literal} COLLATE NOCASE AND type IN ('table','view')"
        ),
        DatabaseKind::PostgreSql => format!(
            "SELECT table_name FROM information_schema.tables WHERE table_schema={} AND table_name={literal}",
            workbench::literal_for(kind, &o.schema)
        ),
        _ => return Err("不支持该数据库类型".into()),
    };
    let rows = c.query(&sql).await?;
    if rows.values.is_empty() {
        return Ok(None);
    }
    if kind == DatabaseKind::Sqlite {
        return Ok(rows.values[0][0].clone());
    }
    Ok(Some(
        workbench::schema(c, kind, database, o, true).await?.ddl,
    ))
}
/// Application command categories are not server grants; preparation remains mandatory.
pub const COMMANDS: &[(&str, &str, bool)] = &[
    ("select", "SELECT", true),
    ("explain", "EXPLAIN", true),
    ("insert", "INSERT", false),
    ("update", "UPDATE", false),
    ("delete", "DELETE", false),
    ("create", "CREATE TABLE", false),
    ("alter", "ALTER TABLE", false),
    ("drop", "DROP", false),
    ("truncate", "TRUNCATE", false),
    ("show_create", "SHOW CREATE TABLE / VIEW", true),
    (
        "show_columns",
        "SHOW INDEX / INDEXES / KEYS / COLUMNS / FIELDS",
        true,
    ),
    ("show_tables", "SHOW TABLES", true),
    ("show_status", "SHOW STATUS", true),
    ("show_variables", "SHOW VARIABLES", true),
    ("show_databases", "SHOW DATABASES", true),
];
pub const SCRIPT_COMMANDS: &[(&str, &str, bool)] = &[
    ("script_session", "USE / SET SESSION", true),
    (
        "script_transaction",
        "BEGIN / COMMIT / ROLLBACK / SET TRANSACTION",
        true,
    ),
    (
        "script_lock",
        "LOCK / UNLOCK TABLES / SELECT INTO FOR UPDATE",
        false,
    ),
    ("script_drop_trigger", "DROP TRIGGER", false),
    ("script_variables", "SELECT INTO @variable", false),
];

/// Stable, non-sensitive operation label, ignoring leading SQL comments.
pub fn script_operation_label(sql: &str) -> &'static str {
    // UI labels must not tokenize an entire multi-megabyte INSERT payload.
    let sql = sql.get(..sql.len().min(8192)).unwrap_or("");
    use sqlparser::tokenizer::{Token, Tokenizer};
    let Some(Token::Word(word)) = Tokenizer::new(&MySqlDialect {}, sql)
        .tokenize()
        .ok()
        .and_then(|ts| ts.into_iter().find(|t| !matches!(t, Token::Whitespace(_))))
    else {
        return "COMMAND";
    };
    if word.quote_style.is_some() {
        return "COMMAND";
    }
    match word.value.to_ascii_uppercase().as_str() {
        "DROP"
            if workbench::script_context::drop_trigger_target(sql)
                .ok()
                .flatten()
                .is_some() =>
        {
            "DROP TRIGGER"
        }
        "INSERT" => "INSERT",
        "UPDATE" => "UPDATE",
        "DELETE" => "DELETE",
        "CREATE" => "CREATE",
        "ALTER" => "ALTER",
        "DROP" => "DROP",
        "TRUNCATE" => "TRUNCATE",
        "SELECT" => "SELECT",
        "EXPLAIN" => "EXPLAIN",
        "SET" => "SET",
        "USE" => "USE",
        "BEGIN" => "BEGIN",
        "COMMIT" => "COMMIT",
        "ROLLBACK" => "ROLLBACK",
        _ => "COMMAND",
    }
}
pub fn script_command_required(kind: DatabaseKind, sql: &str) -> Option<&'static str> {
    if kind == DatabaseKind::MySql {
        let expanded = workbench::script_context::expand(sql, None).ok()?;
        if let Some((_, locked)) = workbench::script_context::variable_select(&expanded).ok()? {
            return validate_script_statement(kind, sql).ok().map(|_| {
                if locked {
                    "script_lock"
                } else {
                    "script_variables"
                }
            });
        }
        if workbench::script_context::drop_trigger_target(&expanded)
            .ok()?
            .is_some()
        {
            return Some("script_drop_trigger");
        }
        if workbench::script_context::keys_target(&expanded)
            .ok()?
            .is_some()
        {
            return Some("alter");
        }
        if let Some(control) = workbench::script_context::control(&expanded).ok()? {
            return Some(control.permission());
        }
        return script_sql_command(kind, &expanded);
    }
    script_sql_command(kind, sql)
}
fn script_sql_command(kind: DatabaseKind, sql: &str) -> Option<&'static str> {
    if sql.len() <= 60_000 {
        return command_required(kind, sql);
    }
    match parse_for(kind, sql, true).ok()? {
        Statement::Insert(_) => Some("insert"),
        _ => None,
    }
}

/// REPLACE can remove existing rows; Note read/write must not use it to bypass DELETE restrictions.
pub fn note_mode_allows(
    mode: crate::access_mode::AccessMode,
    kind: DatabaseKind,
    sql: &str,
    script: bool,
) -> bool {
    let command = if script {
        script_command_required(kind, sql)
    } else {
        command_required(kind, sql)
    };
    let Some(command) = command else {
        return false;
    };
    if !mode.allows_note(command) {
        return false;
    }
    if command == "insert" && mode != crate::access_mode::AccessMode::Admin {
        let expanded = if script && kind == DatabaseKind::MySql {
            match workbench::script_context::expand(sql, None) {
                Ok(s) => s,
                Err(_) => return false,
            }
        } else {
            sql.into()
        };
        return matches!(parse_for(kind,&expanded,script),Ok(Statement::Insert(insert)) if !insert.replace_into && insert.or!=Some(ast::SqliteOnConflict::Replace));
    }
    true
}
pub fn command_required(kind: DatabaseKind, sql: &str) -> Option<&'static str> {
    if let Some(key) = extension_required(kind, sql) {
        return Some(key);
    }
    if mysql_metadata(kind, sql) {
        use sqlparser::tokenizer::{Token, Tokenizer};
        let tokens = Tokenizer::new(&MySqlDialect {}, sql).tokenize().ok()?;
        return Some(if tokens.iter().filter(|t| !matches!(t, Token::Whitespace(_))).nth(1).is_some_and(|token| matches!(token, Token::Word(w) if w.quote_style.is_none() && w.value.eq_ignore_ascii_case("CREATE"))) {
            "show_create"
        } else { "show_columns" });
    }
    match parse(kind, sql).ok()? {
        Statement::Query(_) => Some("select"),
        Statement::Explain { .. } => Some("explain"),
        Statement::Insert(_) => Some("insert"),
        Statement::Update { .. } => Some("update"),
        Statement::Delete(_) => Some("delete"),
        Statement::CreateTable(_) => Some("create"),
        Statement::AlterTable { .. } => Some("alter"),
        Statement::Drop { .. } => Some("drop"),
        Statement::Truncate { .. } => Some("truncate"),
        _ => None,
    }
}

/// Vetted optional read-only categories. No arbitrary SQL-prefix escape hatch.
pub fn extension_required(kind: DatabaseKind, sql: &str) -> Option<&'static str> {
    if kind != DatabaseKind::MySql
        || sql.len() > 60_000
        || sql.contains('\0')
        || sql.contains("/*!")
        || sql.contains("/*M!")
    {
        return None;
    }
    use sqlparser::tokenizer::{Token, Tokenizer};
    let mut tokens = Tokenizer::new(&MySqlDialect {}, sql)
        .tokenize()
        .ok()?
        .into_iter()
        .filter(|t| !matches!(t, Token::Whitespace(_)))
        .collect::<Vec<_>>();
    if tokens.last() == Some(&Token::SemiColon) {
        tokens.pop();
    }
    if tokens.len() < 2 {
        return None;
    }
    let word = |t: &Token| match t {
        Token::Word(w) if w.quote_style.is_none() => Some(w.value.to_ascii_uppercase()),
        _ => None,
    };
    if word(&tokens[0]).as_deref() != Some("SHOW") {
        return None;
    }
    let scoped = matches!(word(&tokens[1]).as_deref(), Some("SESSION" | "GLOBAL"));
    let index = if scoped { 2 } else { 1 };
    let category = match word(tokens.get(index)?).as_deref() {
        Some("TABLES") if !scoped => "show_tables",
        Some("STATUS") => "show_status",
        Some("VARIABLES") => "show_variables",
        Some("DATABASES" | "SCHEMAS") if !scoped => "show_databases",
        _ => return None,
    };
    let suffix = &tokens[index + 1..];
    if suffix.is_empty()
        || matches!(suffix, [like, Token::SingleQuotedString(_)] if word(like).as_deref() == Some("LIKE") && !sql.contains('\\'))
    {
        Some(category)
    } else {
        None
    }
}
// MySQL metadata reads have their own bounded result path, never SELECT pagination.
fn mysql_metadata(kind: DatabaseKind, sql: &str) -> bool {
    use sqlparser::tokenizer::{Token, Tokenizer};
    if kind != DatabaseKind::MySql
        || sql.len() > 60_000
        || sql.contains(['\0'])
        || sql.contains("/*!")
        || sql.contains("/*M!")
    {
        return false;
    }
    let Ok(tokens) = Tokenizer::new(&MySqlDialect {}, sql).tokenize() else {
        return false;
    };
    let mut tokens = tokens
        .into_iter()
        .filter(|t| !matches!(t, Token::Whitespace(_)))
        .collect::<Vec<_>>();
    if tokens.last() == Some(&Token::SemiColon) {
        tokens.pop();
    }
    fn keyword(token: Option<&Token>, word: &str) -> bool {
        matches!(token, Some(Token::Word(w)) if w.quote_style.is_none() && w.value.eq_ignore_ascii_case(word))
    }
    if extension_required(kind, sql).is_some() {
        return true;
    }
    if !keyword(tokens.first(), "SHOW") {
        return false;
    }
    let mut i = 1;
    let create = keyword(tokens.get(i), "CREATE");
    if create {
        i += 1;
        if !["TABLE", "VIEW"].iter().any(|w| keyword(tokens.get(i), w)) {
            return false;
        }
        i += 1;
    } else {
        if keyword(tokens.get(i), "FULL") {
            i += 1;
        }
        if !["INDEX", "INDEXES", "KEYS", "COLUMNS", "FIELDS"]
            .iter()
            .any(|w| keyword(tokens.get(i), w))
        {
            return false;
        }
        i += 1;
        if !["FROM", "IN"].iter().any(|w| keyword(tokens.get(i), w)) {
            return false;
        }
        i += 1;
    }
    if !matches!(tokens.get(i), Some(Token::Word(_))) {
        return false;
    }
    i += 1;
    if tokens.get(i) == Some(&Token::Period) {
        i += 1;
        if !matches!(tokens.get(i), Some(Token::Word(_))) {
            return false;
        }
        i += 1;
    }
    if !create && ["FROM", "IN"].iter().any(|w| keyword(tokens.get(i), w)) {
        i += 1;
        if !matches!(tokens.get(i), Some(Token::Word(_))) {
            return false;
        }
        i += 1;
    }
    i == tokens.len()
}
async fn metadata_result(c: &mut impl Access, sql: &str) -> Result<ResultPage, String> {
    let start = Instant::now();
    let rows = c.query(sql).await?;
    if rows.columns.len() > 512 || rows.values.len() > 1000 {
        return Err("notes.metadata.limit".into());
    }
    Ok(ResultPage {
        execution: None,
        affected_rows: None,
        grid: None,
        rows,
        more: false,
        offset: 0,
        elapsed_ms: start.elapsed().as_millis() as u64,
        read_only: true,
    })
}
pub async fn prepare(
    c: &mut impl Access,
    kind: DatabaseKind,
    database: &str,
    version: &str,
    sql: &str,
) -> Result<Plan, String> {
    prepare_inner(c, kind, database, version, sql, false).await
}
async fn prepare_inner(
    c: &mut impl Access,
    kind: DatabaseKind,
    database: &str,
    version: &str,
    sql: &str,
    script: bool,
) -> Result<Plan, String> {
    if mysql_metadata(kind, sql) {
        return Ok(Plan {
            sql: sql.trim().into(),
            read_only: true,
            confirm: false,
            guards: vec![],
        });
    }
    let stmt = parse_for(kind, sql, script)?;
    let info = inspect(&stmt, kind)?;
    let legacy = kind == DatabaseKind::MySql && !crate::mysql::modern_version(version);
    let mut guards = Vec::new();
    if info.writes {
        for parts in info.tables {
            let object = resolved_object(c, kind, database, &parts).await?;
            let stamp = stamp(c, kind, database, &object).await?;
            guards.push(Guard { object, stamp });
        }
        if guards.is_empty() {
            return Err("无法识别写入目标，未生成执行预览".into());
        }
    }
    Ok(Plan {
        sql: sql.trim().to_owned(),
        read_only: !info.writes,
        confirm: info.writes
            || ((legacy || script && kind == DatabaseKind::MySql) && info.functions),
        guards,
    })
}
pub async fn prepare_script(
    c: &mut impl Access,
    kind: DatabaseKind,
    database: &str,
    version: &str,
    sql: &str,
) -> Result<Plan, String> {
    validate_script_target(kind, sql, database)?;
    let expanded = if kind == DatabaseKind::MySql {
        workbench::script_context::expand(
            sql,
            Some(workbench::script_context::version_number(version)?),
        )?
    } else {
        sql.into()
    };
    let empty = expanded.trim().trim_matches(';').trim().is_empty();
    if empty
        || kind == DatabaseKind::MySql && workbench::script_context::control(&expanded)?.is_some()
    {
        if !empty && !crate::mysql::modern_version(version) {
            return Err("script.error.legacy".into());
        }
        let confirm = if empty {
            false
        } else {
            workbench::script_context::control(&expanded)?
                .unwrap()
                .confirm()
        };
        return Ok(Plan {
            sql: sql.trim().into(),
            read_only: !confirm,
            confirm,
            guards: vec![],
        });
    }
    if kind == DatabaseKind::MySql && crate::mysql::modern_version(version) {
        let mode = c.query("SELECT @@SESSION.sql_mode").await?;
        let mode = mode
            .values
            .first()
            .and_then(|r| r.first())
            .and_then(Option::as_ref)
            .ok_or("script.error.mode")?;
        if mode.split(',').any(|m| {
            matches!(
                m.trim().to_ascii_uppercase().as_str(),
                "NO_BACKSLASH_ESCAPES" | "ANSI_QUOTES"
            )
        }) {
            return Err("script.error.mode".into());
        }
    }
    if kind == DatabaseKind::MySql
        && let Some(parts) = workbench::script_context::keys_target(&expanded)?
    {
        let object = resolved_object(c, kind, database, &parts).await?;
        let stamp = stamp(c, kind, database, &object).await?;
        return Ok(Plan {
            sql: sql.trim().into(),
            read_only: false,
            confirm: true,
            guards: vec![Guard { object, stamp }],
        });
    }
    if kind == DatabaseKind::MySql
        && let Some(parts) = workbench::script_context::drop_trigger_target(&expanded)?
    {
        if !crate::mysql::modern_version(version) {
            return Err("script.error.legacy".into());
        }
        let mut target = object(kind, database, &parts)?;
        target.kind = "trigger".into();
        let rows = c.query(&format!("SELECT ACTION_STATEMENT,EVENT_MANIPULATION,EVENT_OBJECT_TABLE,ACTION_TIMING,DEFINER,SQL_MODE,CHARACTER_SET_CLIENT,COLLATION_CONNECTION,DATABASE_COLLATION FROM information_schema.TRIGGERS WHERE TRIGGER_SCHEMA={} AND TRIGGER_NAME={}", workbench::literal_for(kind,database), workbench::literal_for(kind,&target.name))).await?;
        let stamp = if rows.values.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&rows.values).map_err(|_| "script.error.syntax")?)
        };
        return Ok(Plan {
            sql: sql.trim().into(),
            read_only: false,
            confirm: true,
            guards: vec![Guard {
                object: target,
                stamp,
            }],
        });
    }
    if kind == DatabaseKind::MySql
        && let Some((inspection, _)) = workbench::script_context::variable_select(&expanded)?
    {
        validate_script_statement(kind, &expanded)?;
        let mut plan = prepare_inner(c, kind, database, version, &inspection, true).await?;
        plan.sql = sql.trim().into();
        plan.read_only = false;
        plan.confirm = true;
        return Ok(plan);
    }
    let mut plan = prepare_inner(c, kind, database, version, &expanded, true).await?;
    plan.sql = sql.trim().into();
    Ok(plan)
}
/// Plain EXPLAIN only: ANALYZE is never appended and the original statement must be a query.
pub async fn explain(
    c: &mut impl Access,
    kind: DatabaseKind,
    sql: &str,
) -> Result<ResultPage, String> {
    let statement = parse(kind, sql)?;
    let info = inspect(&statement, kind)?;
    if info.writes {
        return Err("notes.explain.query_only".into());
    }
    let start = Instant::now();
    let prefix = if kind == DatabaseKind::Sqlite {
        "EXPLAIN QUERY PLAN"
    } else {
        "EXPLAIN"
    };
    let sql = if matches!(statement, Statement::Explain { .. }) {
        sql.trim().to_owned()
    } else {
        format!("{prefix} {sql}")
    };
    let mut rows = c.query(&sql).await?;
    if rows.columns.len() > 512 || rows.values.len() > 1000 {
        return Err("执行计划超过显示上限，请简化查询".into());
    }
    rows.values.shrink_to_fit();
    Ok(ResultPage {
        execution: None,
        affected_rows: None,
        grid: None,
        rows,
        more: false,
        offset: 0,
        elapsed_ms: start.elapsed().as_millis() as u64,
        read_only: true,
    })
}

fn number(expr: &Expr) -> Result<usize, String> {
    match expr {
        Expr::Value(v) => match &v.value {
            Value::Number(n, _) => n
                .parse()
                .map_err(|_| "分页 LIMIT/OFFSET 需要非负整数".into()),
            _ => Err("分页 LIMIT/OFFSET 需要整数".into()),
        },
        _ => Err("分页 LIMIT/OFFSET 暂不支持表达式".into()),
    }
}
pub fn page_sql(
    kind: DatabaseKind,
    sql: &str,
    offset: usize,
    size: usize,
) -> Result<String, String> {
    if !(1..=1000).contains(&size) || offset > 10_000_000 {
        return Err("分页范围无效".into());
    }
    let mut stmt = parse(kind, sql)?;
    if inspect(&stmt, kind)?.writes {
        return Err("写入语句不能分页重放".into());
    }
    let Statement::Query(ref mut q) = stmt else {
        return Err("仅查询支持分页".into());
    };
    if q.fetch.is_some() {
        return Err("请使用 LIMIT 代替 FETCH 以进行分页".into());
    }
    let (limit, base) = match &q.limit_clause {
        None => (None, 0),
        Some(LimitClause::LimitOffset {
            limit,
            offset,
            limit_by,
        }) if limit_by.is_empty() => (
            limit.as_ref().map(number).transpose()?,
            offset
                .as_ref()
                .map(|o| number(&o.value))
                .transpose()?
                .unwrap_or(0),
        ),
        Some(LimitClause::OffsetCommaLimit { offset, limit }) => {
            (Some(number(limit)?), number(offset)?)
        }
        _ => return Err("当前 LIMIT 形式不支持分页".into()),
    };
    let take = limit.map_or(size + 1, |n| n.saturating_sub(offset).min(size + 1));
    use sqlparser::tokenizer::{Token, Tokenizer};
    let dialect: Box<dyn Dialect> = match kind {
        DatabaseKind::MySql => Box::new(MySqlDialect {}),
        DatabaseKind::PostgreSql => Box::new(PostgreSqlDialect {}),
        _ => Box::new(SQLiteDialect {}),
    };
    let tokens = Tokenizer::new(dialect.as_ref(), sql)
        .tokenize_with_location()
        .map_err(|_| "无法定位 SQL 分页范围")?;
    let end = if let Some(clause) = &q.limit_clause {
        let start = clause.span().start;
        tokens
            .iter()
            .rfind(|t| {
                !matches!(t.token, Token::Whitespace(_))
                    && (t.span.start.line, t.span.start.column) < (start.line, start.column)
            })
            .map(|t| t.span.start)
    } else {
        tokens
            .iter()
            .find(|t| matches!(t.token, Token::SemiColon))
            .map(|t| t.span.start)
    };
    let cutoff = if let Some(end) = end {
        let mut line = 1;
        let mut column = 1;
        let mut found = None;
        for (index, ch) in sql.char_indices() {
            if line == end.line && column == end.column {
                found = Some(index);
                break;
            }
            if ch == '\n' {
                line += 1;
                column = 1;
            } else {
                column += 1;
            }
        }
        found.ok_or("无法定位 SQL 分页范围")?
    } else {
        sql.len()
    };
    Ok(format!(
        "{}\nLIMIT {take} OFFSET {}",
        &sql[..cutoff],
        base.checked_add(offset).ok_or("分页偏移超出范围")?
    ))
}
fn statement_body(kind: DatabaseKind, sql: &str) -> Result<String, String> {
    use sqlparser::tokenizer::{Token, Tokenizer};
    let dialect: Box<dyn Dialect> = match kind {
        DatabaseKind::MySql => Box::new(MySqlDialect {}),
        DatabaseKind::PostgreSql => Box::new(PostgreSqlDialect {}),
        _ => Box::new(SQLiteDialect {}),
    };
    let tokens = Tokenizer::new(dialect.as_ref(), sql)
        .tokenize_with_location()
        .map_err(|_| "无法读取 SQL 语句边界")?;
    if let Some(token) = tokens.iter().find(|t| matches!(t.token, Token::SemiColon)) {
        let mut line = 1;
        let mut column = 1;
        for (i, ch) in sql.char_indices() {
            if (line, column) == (token.span.start.line, token.span.start.column) {
                return Ok(sql[..i].into());
            }
            if ch == '\n' {
                line += 1;
                column = 1;
            } else {
                column += 1;
            }
        }
        return Err("SQL 语句边界无效".into());
    }
    Ok(sql.into())
}
pub async fn execute(
    c: &mut impl Access,
    kind: DatabaseKind,
    database: &str,
    version: &str,
    plan: &Plan,
    confirmed: bool,
    page: (usize, usize),
) -> Result<ResultPage, String> {
    execute_inner(c, kind, database, version, plan, confirmed, page, false).await
}
pub async fn execute_script(
    c: &mut impl Access,
    kind: DatabaseKind,
    database: &str,
    version: &str,
    plan: &Plan,
    confirmed: bool,
) -> Result<ResultPage, String> {
    let expected = prepare_script(c, kind, database, version, &plan.sql).await?;
    if expected.read_only != plan.read_only
        || expected.confirm != plan.confirm
        || expected.guards.len() != plan.guards.len()
        || expected
            .guards
            .iter()
            .zip(&plan.guards)
            .any(|(a, b)| a.object != b.object || a.stamp != b.stamp)
    {
        return Err("notes.preview_changed".into());
    }
    if expected.confirm && !confirmed {
        return Err("notes.preview_changed".into());
    }
    let expanded = if kind == DatabaseKind::MySql {
        workbench::script_context::expand(
            &plan.sql,
            Some(workbench::script_context::version_number(version)?),
        )?
    } else {
        plan.sql.clone()
    };
    let empty = expanded.trim().trim_matches(';').trim().is_empty();
    if empty
        || kind == DatabaseKind::MySql && workbench::script_context::control(&expanded)?.is_some()
    {
        let started = Instant::now();
        if plan.confirm && !confirmed {
            return Err("notes.preview_changed".into());
        }
        if !empty {
            c.query(&expanded).await?;
        }
        return Ok(ResultPage {
            execution: Some(Box::new(ExecutionSummary {
                operation: "SESSION".into(),
                status: "succeeded".into(),
                elapsed_ms: started.elapsed().as_millis() as u64,
                ..Default::default()
            })),
            affected_rows: None,
            grid: None,
            rows: Rows::default(),
            more: false,
            offset: 0,
            elapsed_ms: started.elapsed().as_millis() as u64,
            read_only: plan.read_only,
        });
    }
    if kind == DatabaseKind::MySql
        && (workbench::script_context::drop_trigger_target(&expanded)?.is_some()
            || workbench::script_context::variable_select(&expanded)?.is_some())
    {
        let start = Instant::now();
        c.query(&expanded).await?;
        return Ok(ResultPage {
            execution: Some(Box::new(ExecutionSummary {
                operation:
                    if workbench::script_context::drop_trigger_target(&expanded)?.is_some() {
                        "DROP TRIGGER"
                    } else {
                        "SELECT INTO"
                    }
                    .into(),
                status: "succeeded".into(),
                objects: plan.guards.iter().map(|g| g.object.clone()).collect(),
                elapsed_ms: start.elapsed().as_millis() as u64,
                ..Default::default()
            })),
            affected_rows: None,
            grid: None,
            rows: Rows::default(),
            more: false,
            offset: 0,
            elapsed_ms: start.elapsed().as_millis() as u64,
            read_only: false,
        });
    }
    let mut expanded_plan = plan.clone();
    expanded_plan.sql = expanded;
    if kind == DatabaseKind::MySql
        && workbench::script_context::keys_target(&expanded_plan.sql)?.is_some()
    {
        let start = Instant::now();
        c.query(&expanded_plan.sql).await?;
        return Ok(ResultPage {
            execution: Some(Box::new(ExecutionSummary {
                operation: "ALTER".into(),
                status: "succeeded".into(),
                affected_rows: None,
                matched_rows: None,
                changed_rows: None,
                elapsed_ms: start.elapsed().as_millis() as u64,
                count_source: None,
                objects: plan
                    .guards
                    .iter()
                    .take(32)
                    .map(|g| g.object.clone())
                    .collect(),
                ..Default::default()
            })),
            affected_rows: None,
            grid: None,
            rows: Rows::default(),
            more: false,
            offset: 0,
            elapsed_ms: start.elapsed().as_millis() as u64,
            read_only: false,
        });
    }
    execute_inner(
        c,
        kind,
        database,
        version,
        &expanded_plan,
        confirmed,
        (0, 20),
        true,
    )
    .await
}
#[allow(clippy::too_many_arguments)]
async fn execute_inner(
    c: &mut impl Access,
    kind: DatabaseKind,
    database: &str,
    version: &str,
    plan: &Plan,
    confirmed: bool,
    page: (usize, usize),
    script: bool,
) -> Result<ResultPage, String> {
    let (offset, size) = page;
    if mysql_metadata(kind, &plan.sql) {
        if !plan.read_only || !plan.guards.is_empty() || offset != 0 {
            return Err("notes.metadata.invalid".into());
        }
        let mut result = metadata_result(c, &plan.sql).await?;
        if script {
            result.more = result.rows.values.len() > 20;
            result.rows.values.truncate(20);
        }
        return Ok(result);
    }
    let stmt = parse_for(kind, &plan.sql, script)?;
    let info = inspect(&stmt, kind)?;
    if info.writes == plan.read_only {
        return Err("SQL 预览状态无效，请重新执行".into());
    }
    let legacy = kind == DatabaseKind::MySql && !crate::mysql::modern_version(version);
    if (info.writes || (legacy && info.functions)) && !confirmed {
        return Err("此 SQL 必须先预览并确认".into());
    }
    if matches!(stmt, Statement::Explain { .. }) {
        if offset != 0 {
            return Err("notes.explain.no_paging".into());
        }
        return explain(c, kind, &plan.sql).await;
    }
    if !plan.read_only && offset != 0 {
        return Err("写入语句不能分页重放".into());
    }
    if info.writes {
        let mut expected = Vec::new();
        for parts in &info.tables {
            expected.push(resolved_object(c, kind, database, parts).await?);
        }
        if expected.len() != plan.guards.len()
            || expected
                .iter()
                .zip(&plan.guards)
                .any(|(a, b)| a != &b.object)
        {
            return Err("SQL 目标与预览不一致".into());
        }
        for guard in &plan.guards {
            if stamp(c, kind, database, &guard.object).await? != guard.stamp {
                return Err("表结构已变化，请重新预览 SQL".into());
            }
        }
    }
    let start = Instant::now();
    let sql = if plan.read_only && !script {
        page_sql(kind, &plan.sql, offset, size)?
    } else {
        plan.sql.clone()
    };
    if plan.read_only && !legacy && (!script || kind != DatabaseKind::MySql) {
        c.query(match kind {
            DatabaseKind::Sqlite => "PRAGMA query_only=ON",
            DatabaseKind::PostgreSql => "BEGIN READ ONLY",
            _ => "START TRANSACTION READ ONLY",
        })
        .await?;
    }
    let result = if plan.read_only {
        c.query_data_counted(&sql).await
    } else {
        c.query_counted(&sql).await
    };
    if plan.read_only && !legacy && (!script || kind != DatabaseKind::MySql) {
        let _ = c
            .query(if kind == DatabaseKind::Sqlite {
                "PRAGMA query_only=OFF"
            } else {
                "ROLLBACK"
            })
            .await;
    }
    let (mut rows, driver_affected) = result.map_err(|error| {
        if kind == DatabaseKind::MySql
            && matches!(
                error.as_str(),
                crate::mysql::SQL_SYNTAX_ERROR | crate::mysql::TABLE_MISSING
            )
        {
            error
        } else {
            "SQL 执行失败，请检查语法、权限及对象是否存在；写入可能已生效，请勿盲目重试".to_string()
        }
    })?;
    let (created_tables, skipped_tables) = if let Statement::CreateTable(create) = &stmt {
        if !create.if_not_exists {
            (Some(1), Some(0))
        } else if kind == DatabaseKind::MySql {
            // Read diagnostics before any other statement can clear the CREATE result.
            match c.query("SHOW WARNINGS LIMIT 32").await {
                Ok(warnings)
                    if warnings
                        .values
                        .iter()
                        .any(|r| r.get(1).and_then(|v| v.as_deref()) == Some("1050")) =>
                {
                    (Some(0), Some(1))
                }
                Ok(warnings) if warnings.values.len() < 32 => (Some(1), Some(0)),
                _ => (None, None),
            }
        } else {
            // Success alone cannot distinguish conditional creation from a skipped table.
            (None, None)
        }
    } else {
        (None, None)
    };
    let mixed_write =
        matches!(&stmt, Statement::Insert(insert) if insert.on.is_some() || insert.replace_into);
    // Read the session counter immediately, before metadata queries can replace it.
    // SQLite changes() retains old DML counts after DDL, so only inspect DML.
    let affected_rows = if !plan.read_only {
        let dml = matches!(
            parse_for(kind, &plan.sql, script)?,
            Statement::Insert(_) | Statement::Update { .. } | Statement::Delete(_)
        );
        let counter = match kind {
            DatabaseKind::MySql if dml => Some("SELECT ROW_COUNT()"),
            DatabaseKind::Sqlite if dml => Some("SELECT changes()"),
            _ => None,
        };
        if dml && driver_affected.is_some() {
            driver_affected
        } else if let Some(counter) = counter {
            c.query(counter)
                .await
                .ok()
                .and_then(|r| r.values.first()?.first()?.as_ref()?.parse().ok())
        } else {
            None
        }
    } else {
        None
    };
    if rows.columns.len() > 512 {
        return Err("查询结果超过 512 列，请减少选择列".into());
    }
    let more = plan.read_only && rows.values.len() > size;
    rows.values.truncate(size);
    let grid = if plan.read_only && !script {
        let info = result_grid::source_info(c, kind, database, &plan.sql, &rows.columns)
            .await
            .unwrap_or_else(|_| result_grid::readonly(&rows.columns));
        if rows.columns.is_empty() {
            rows.columns = info.schema.columns.iter().map(|c| c.name.clone()).collect();
        }
        Some(info)
    } else {
        None
    };
    let elapsed_ms = start.elapsed().as_millis() as u64;
    let execution = (!plan.read_only).then(|| {
        Box::new(ExecutionSummary {
            operation: (if script {
                script_command_required(kind, &plan.sql)
            } else {
                command_required(kind, &plan.sql)
            })
            .unwrap_or("command")
            .to_ascii_uppercase(),
            status: "succeeded".into(),
            affected_rows,
            matched_rows: None,
            changed_rows: None,
            elapsed_ms,
            count_source: affected_rows.map(|_| {
                if driver_affected.is_some() {
                    "driver_result".into()
                } else {
                    "session_counter".into()
                }
            }),
            objects: plan
                .guards
                .iter()
                .take(32)
                .map(|g| g.object.clone())
                .collect(),
            created_tables,
            skipped_tables,
            mixed_write,
            table_create: matches!(&stmt, Statement::CreateTable(_)),
        })
    });
    Ok(ResultPage {
        execution,
        affected_rows,
        grid,
        rows,
        more,
        offset,
        elapsed_ms,
        read_only: plan.read_only,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mysql_script_create_counts_distinguish_created_and_skipped_tables() {
        struct Mock {
            exists: bool,
            skipped: bool,
            warning_reads: u64,
            row_counter_reads: u64,
        }
        impl Access for Mock {
            fn query<'a>(&'a mut self, sql: &'a str) -> workbench::Query<'a> {
                Box::pin(async move {
                    let rows = |columns: &[&str], values: &[&str]| Rows {
                        columns: columns.iter().map(|v| v.to_string()).collect(),
                        values: vec![values.iter().map(|v| Some(v.to_string())).collect()],
                    };
                    if sql == "SELECT @@SESSION.sql_mode" {
                        return Ok(rows(&["mode"], &[""]));
                    }
                    if sql.starts_with("SHOW CREATE TABLE") {
                        return if self.exists {
                            Ok(rows(
                                &["Table", "Create Table"],
                                &["items", "CREATE TABLE items(id INT)"],
                            ))
                        } else {
                            Err(crate::mysql::TABLE_MISSING.into())
                        };
                    }
                    if sql.starts_with("CREATE TABLE IF NOT EXISTS") {
                        self.skipped = self.exists;
                        self.exists = true;
                        return Ok(Rows::default());
                    }
                    if sql == "SHOW WARNINGS LIMIT 32" {
                        self.warning_reads += 1;
                        return Ok(if self.skipped {
                            rows(&["Level", "Code", "Message"], &["Note", "1050", "omitted"])
                        } else {
                            Rows::default()
                        });
                    }
                    if sql == "SELECT ROW_COUNT()" {
                        self.row_counter_reads += 1;
                        return Ok(rows(&["count"], &["0"]));
                    }
                    Err("unexpected fixture request".into())
                })
            }
        }
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                for exists in [false, true] {
                    let mut db = Mock {
                        exists,
                        skipped: false,
                        warning_reads: 0,
                        row_counter_reads: 0,
                    };
                    let plan = prepare_script(
                        &mut db,
                        DatabaseKind::MySql,
                        "fixture",
                        "8.0.32",
                        "CREATE TABLE IF NOT EXISTS items(id INT)",
                    )
                    .await
                    .unwrap();
                    let result = execute_script(
                        &mut db,
                        DatabaseKind::MySql,
                        "fixture",
                        "8.0.32",
                        &plan,
                        true,
                    )
                    .await
                    .unwrap();
                    let summary = result.execution.unwrap();
                    assert_eq!(summary.created_tables, Some(u64::from(!exists)));
                    assert_eq!(summary.skipped_tables, Some(u64::from(exists)));
                    assert!(summary.table_create);
                    assert_eq!(summary.objects[0].name, "items");
                    assert!(summary.affected_rows.is_none());
                    assert_eq!(db.warning_reads, 1);
                    assert_eq!(db.row_counter_reads, 0);
                }
            });
    }
    #[test]
    fn sqlite_script_reports_dml_and_keeps_conditional_creation_unknown() {
        use sqlx::Connection;
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let mut db = sqlx::SqliteConnection::connect("sqlite::memory:")
                    .await
                    .unwrap();
                for (sql, affected, created) in [
                    ("CREATE TABLE items(id INT PRIMARY KEY)", None, Some(1)),
                    (
                        "CREATE TABLE IF NOT EXISTS items(id INT PRIMARY KEY)",
                        None,
                        None,
                    ),
                    ("INSERT INTO items VALUES(1),(2)", Some(2), None),
                    ("UPDATE items SET id=3 WHERE id=2", Some(1), None),
                    ("DELETE FROM items WHERE id=3", Some(1), None),
                ] {
                    let plan = prepare_script(&mut db, DatabaseKind::Sqlite, "main", "3", sql)
                        .await
                        .unwrap();
                    let result =
                        execute_script(&mut db, DatabaseKind::Sqlite, "main", "3", &plan, true)
                            .await
                            .unwrap();
                    let summary = result.execution.unwrap();
                    assert_eq!(summary.affected_rows, affected);
                    assert_eq!(summary.created_tables, created);
                    assert_eq!(summary.objects[0].name, "items");
                }
                let plan = prepare_script(
                    &mut db,
                    DatabaseKind::Sqlite,
                    "main",
                    "3",
                    "INSERT INTO items VALUES(1) ON CONFLICT(id) DO UPDATE SET id=excluded.id",
                )
                .await
                .unwrap();
                let result =
                    execute_script(&mut db, DatabaseKind::Sqlite, "main", "3", &plan, true)
                        .await
                        .unwrap();
                assert!(result.execution.unwrap().mixed_write);
            });
    }
    #[test]
    fn sqlite_affected_rows_do_not_reuse_previous_dml_for_ddl() {
        use sqlx::Connection;
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let mut db = sqlx::SqliteConnection::connect("sqlite::memory:")
                    .await
                    .unwrap();
                sqlx::query("CREATE TABLE items (id INTEGER)")
                    .execute(&mut db)
                    .await
                    .unwrap();
                for (sql, expected) in [
                    ("INSERT INTO items VALUES (1), (2)", Some(2)),
                    ("UPDATE items SET id=3 WHERE id=99", Some(0)),
                    ("UPDATE items SET id=4", Some(2)),
                    ("ALTER TABLE items ADD COLUMN note TEXT", None),
                    ("DELETE FROM items WHERE id=4", Some(2)),
                    ("INSERT INTO items(id) VALUES (5) RETURNING id", Some(1)),
                ] {
                    let plan = prepare(&mut db, DatabaseKind::Sqlite, "main", "3", sql)
                        .await
                        .unwrap();
                    let result = execute(
                        &mut db,
                        DatabaseKind::Sqlite,
                        "main",
                        "3",
                        &plan,
                        true,
                        (0, 20),
                    )
                    .await
                    .unwrap();
                    assert_eq!(result.affected_rows, expected, "{sql}");
                    let summary = result.execution.as_ref().unwrap();
                    assert_eq!(summary.affected_rows, expected);
                    assert_eq!(summary.elapsed_ms, result.elapsed_ms);
                    let json: serde_json::Value =
                        serde_json::from_str(summary.json_rows().values[0][0].as_ref().unwrap())
                            .unwrap();
                    assert_eq!(json["status"], "succeeded");
                    assert!(json["matched_rows"].is_null() && json["changed_rows"].is_null());
                    assert_eq!(json.get("sql"), None);
                    if sql.contains("RETURNING") {
                        assert_eq!(result.rows.values, vec![vec![Some("5".into())]]);
                    }
                }
            });
    }
    #[tokio::test]
    async fn mysql_write_preflight_uses_exact_ddl_and_keeps_confirmation_and_stale_guards() {
        struct Db {
            ddl: Option<String>,
            error: Option<String>,
            calls: Vec<String>,
        }
        impl Access for Db {
            fn query<'a>(&'a mut self, sql: &'a str) -> workbench::Query<'a> {
                self.calls.push(sql.into());
                Box::pin(async move {
                    if sql.starts_with("SHOW CREATE TABLE ") {
                        if let Some(error) = &self.error {
                            return Err(error.clone());
                        }
                        return self
                            .ddl
                            .as_ref()
                            .map(|ddl| Rows {
                                columns: vec!["Table".into(), "Create Table".into()],
                                values: vec![vec![Some("name".into()), Some(ddl.clone())]],
                            })
                            .ok_or_else(|| crate::mysql::TABLE_MISSING.into());
                    }
                    if sql == "SELECT ROW_COUNT()" {
                        return Ok(Rows {
                            columns: vec!["ROW_COUNT()".into()],
                            values: vec![vec![Some("0".into())]],
                        });
                    }
                    if sql.starts_with("SHOW TABLES") {
                        return Err(crate::mysql::SQL_SYNTAX_ERROR.into());
                    }
                    Ok(Rows::default())
                })
            }
        }
        for version in ["4.0.30", "5.5.62", "8.0.32"] {
            let sql = "ALTER TABLE `items_%'``x` ADD COLUMN label VARCHAR(40) DEFAULT 'a;b'";
            let mut db = Db {
                ddl: Some("CREATE TABLE original".into()),
                error: None,
                calls: vec![],
            };
            let plan = prepare(&mut db, DatabaseKind::MySql, "fixture", version, sql)
                .await
                .unwrap();
            assert_eq!(db.calls, vec!["SHOW CREATE TABLE `fixture`.`items_%'``x`"]);
            assert_eq!(plan.sql, sql);
            assert!(plan.confirm && !plan.read_only);
            db.calls.clear();
            assert!(
                execute(
                    &mut db,
                    DatabaseKind::MySql,
                    "fixture",
                    version,
                    &plan,
                    false,
                    (0, 20)
                )
                .await
                .is_err()
            );
            assert!(db.calls.is_empty());
            db.ddl = Some("CREATE TABLE changed".into());
            assert!(
                execute(
                    &mut db,
                    DatabaseKind::MySql,
                    "fixture",
                    version,
                    &plan,
                    true,
                    (0, 20)
                )
                .await
                .is_err()
            );
            assert!(!db.calls.iter().any(|s| s == sql));
            db.ddl = Some("CREATE TABLE original".into());
            db.calls.clear();
            let result = execute(
                &mut db,
                DatabaseKind::MySql,
                "fixture",
                version,
                &plan,
                true,
                (0, 20),
            )
            .await
            .unwrap();
            assert_eq!(result.affected_rows, None);
            assert_eq!(
                db.calls,
                vec!["SHOW CREATE TABLE `fixture`.`items_%'``x`", sql,]
            );
            for error in [
                crate::mysql::SQL_SYNTAX_ERROR,
                "permission denied",
                "network failed",
            ] {
                db.error = Some(error.into());
                assert_eq!(
                    prepare(&mut db, DatabaseKind::MySql, "fixture", version, sql)
                        .await
                        .err()
                        .unwrap(),
                    error
                );
            }
            db.error = None;
            db.ddl = None;
            let create = "CREATE TABLE fresh(id INT)";
            let plan = prepare(&mut db, DatabaseKind::MySql, "fixture", version, create)
                .await
                .unwrap();
            assert!(plan.guards[0].stamp.is_none());
            db.ddl = Some("CREATE TABLE created_elsewhere".into());
            db.calls.clear();
            assert!(
                execute(
                    &mut db,
                    DatabaseKind::MySql,
                    "fixture",
                    version,
                    &plan,
                    true,
                    (0, 20)
                )
                .await
                .is_err()
            );
            assert!(!db.calls.iter().any(|s| s == create));
            db.ddl = Some(String::new());
            assert_eq!(
                prepare(&mut db, DatabaseKind::MySql, "fixture", version, sql)
                    .await
                    .err()
                    .unwrap(),
                "notes.schema_check_invalid"
            );
        }
    }
    #[test]
    fn mysql_show_metadata_preserves_comments_without_pagination() {
        struct Capture(Vec<String>);
        impl Access for Capture {
            fn query<'a>(&'a mut self, sql: &'a str) -> workbench::Query<'a> {
                self.0.push(sql.into());
                Box::pin(async {
                    Ok(Rows {
                        columns: vec!["Table".into(), "Create Table".into()],
                        values: vec![
                            vec![Some("t".into()), Some("CREATE TABLE t(id INT)".into())];
                            30
                        ],
                    })
                })
            }
        }
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            for sql in ["SHOW CREATE TABLE `seach_works`;\n# 递归方法链\n-- SHOW INDEX FROM `seach_works`;", "SHOW INDEX FROM `t`", "SHOW KEYS IN db.t", "SHOW FULL COLUMNS FROM t FROM db", "SHOW CREATE VIEW db.v", "SHOW DATABASES", "SHOW SCHEMAS LIKE 'test%'", "SHOW VARIABLES LIKE 'skip_show_database'", "SHOW SESSION VARIABLES LIKE 'character_set_%'", "SHOW GLOBAL STATUS LIKE 'Threads_%'"] {
                let mut db=Capture(vec![]);
                let plan=prepare(&mut db,DatabaseKind::MySql,"db","5.5",sql).await.unwrap();
                assert!(plan.read_only && !plan.confirm);
                let result=execute(&mut db,DatabaseKind::MySql,"db","5.5",&plan,false,(0,20)).await.unwrap();
                assert_eq!(db.0,vec![sql]); assert_eq!(result.rows.values.len(),30); assert!(result.grid.is_none() && !result.more);
                db.0.clear();
                assert!(execute(&mut db,DatabaseKind::MySql,"db","5.5",&plan,false,(20,20)).await.is_err());
                assert!(db.0.is_empty());
            }
            for sql in ["SHOW CREATE TABLE t; DELETE FROM t", "SHOW INDEX FROM t WHERE evil()", "SHOW CREATE PROCEDURE p", "SHOW CREATE TABLE t /*!; DROP TABLE t */"] {
                assert!(!mysql_metadata(DatabaseKind::MySql,sql));
            }
            assert!(!mysql_metadata(DatabaseKind::PostgreSql,"SHOW CREATE TABLE t"));
        });
    }
    #[test]
    fn line_comments_do_not_execute_or_hide_pagination() {
        struct Capture(Vec<String>);
        impl Access for Capture {
            fn query<'a>(&'a mut self, sql: &'a str) -> workbench::Query<'a> {
                self.0.push(sql.into());
                Box::pin(async { Ok(Rows::default()) })
            }
        }
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let sql = "-- 查询说明\r\n  # DELETE FROM items; 仅注释\r\nSELECT '#原值', '--原值' # 末尾注释";
            let mut db = Capture(vec![]);
            let plan = prepare(&mut db, DatabaseKind::MySql, "fixture", "5.5", sql).await.unwrap();
            assert!(plan.read_only && !plan.confirm);
            assert_eq!(plan.sql, sql);
            execute(&mut db, DatabaseKind::MySql, "fixture", "5.5", &plan, false, (0,20)).await.unwrap();
            assert!(db.0[0].contains("# 末尾注释\nLIMIT 21 OFFSET 0"));
            assert!(db.0[0].contains("'#原值', '--原值'"));
            for kind in [DatabaseKind::MySql,DatabaseKind::PostgreSql,DatabaseKind::Sqlite] {
                let sql="-- DELETE FROM items; 仅注释\nSELECT '--原值' -- 末尾注释";
                let plan=prepare(&mut db,kind,"fixture","8",sql).await.unwrap();
                assert!(plan.read_only);
                assert!(page_sql(kind,sql,0,20).unwrap().contains("-- 末尾注释\nLIMIT"));
            }
            use sqlx::Connection;
            let mut db=sqlx::SqliteConnection::connect("sqlite::memory:").await.unwrap();
            let sql="-- SELECT 9;\nSELECT '#字符串', '--字符串' -- 末尾";
            let plan=prepare(&mut db,DatabaseKind::Sqlite,"main","3",sql).await.unwrap();
            let page=execute(&mut db,DatabaseKind::Sqlite,"main","3",&plan,false,(0,20)).await.unwrap();
            assert_eq!(page.rows.values[0],vec![Some("#字符串".into()),Some("--字符串".into())]);
        });
    }
    #[test]
    fn explicit_explain_is_not_paginated_or_prefixed_twice() {
        struct Capture(Vec<String>);
        impl Access for Capture {
            fn query<'a>(&'a mut self, sql: &'a str) -> workbench::Query<'a> {
                self.0.push(sql.into());
                Box::pin(async {
                    Ok(Rows {
                        columns: vec!["plan".into()],
                        values: vec![vec![Some("scan".into())]; 25],
                    })
                })
            }
        }
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                for (kind, sql) in [
                    (DatabaseKind::MySql, "EXPLAIN SELECT * FROM items"),
                    (
                        DatabaseKind::MySql,
                        "EXPLAIN FORMAT=JSON SELECT * FROM items",
                    ),
                    (
                        DatabaseKind::PostgreSql,
                        "-- plan\nexplain (format json, costs false) SELECT * FROM items",
                    ),
                    (
                        DatabaseKind::Sqlite,
                        "EXPLAIN QUERY PLAN SELECT * FROM items",
                    ),
                ] {
                    let mut db = Capture(vec![]);
                    let plan = prepare(&mut db, kind, "fixture", "8.4", sql).await.unwrap();
                    assert!(plan.read_only && !plan.confirm && plan.guards.is_empty());
                    let result = execute(&mut db, kind, "fixture", "8.4", &plan, false, (0, 20))
                        .await
                        .unwrap();
                    assert_eq!(result.rows.values.len(), 25);
                    assert!(!result.more && result.grid.is_none());
                    assert_eq!(db.0, vec![sql]);
                    db.0.clear();
                    explain(&mut db, kind, sql).await.unwrap();
                    assert_eq!(db.0, vec![sql]);
                    db.0.clear();
                    assert!(
                        execute(&mut db, kind, "fixture", "8.4", &plan, false, (20, 20))
                            .await
                            .is_err()
                    );
                    assert!(db.0.is_empty());
                }
                for sql in [
                    "EXPLAIN ANALYZE SELECT 1",
                    "EXPLAIN (ANALYZE true) SELECT 1",
                    "EXPLAIN (ANALYZE false) SELECT 1",
                    "EXPLAIN DELETE FROM items",
                    "EXPLAIN SELECT * FROM items FOR UPDATE",
                    "EXPLAIN SELECT 1; SELECT 2",
                    "EXPLAIN WITH d AS (DELETE FROM items RETURNING *) SELECT * FROM d",
                ] {
                    let mut db = Capture(vec![]);
                    assert!(
                        prepare(&mut db, DatabaseKind::PostgreSql, "fixture", "17", sql)
                            .await
                            .is_err(),
                        "{sql}"
                    );
                    assert!(
                        explain(&mut db, DatabaseKind::PostgreSql, sql)
                            .await
                            .is_err()
                    );
                    assert!(db.0.is_empty());
                }
                use sqlx::Connection;
                let mut db = sqlx::SqliteConnection::connect("sqlite::memory:")
                    .await
                    .unwrap();
                sqlx::raw_sql(
                    "CREATE TABLE items(id INTEGER PRIMARY KEY); INSERT INTO items VALUES(1)",
                )
                .execute(&mut db)
                .await
                .unwrap();
                for sql in [
                    "EXPLAIN SELECT * FROM items",
                    "EXPLAIN QUERY PLAN SELECT * FROM items",
                ] {
                    let plan = prepare(&mut db, DatabaseKind::Sqlite, "main", "3", sql)
                        .await
                        .unwrap();
                    let result = execute(
                        &mut db,
                        DatabaseKind::Sqlite,
                        "main",
                        "3",
                        &plan,
                        false,
                        (0, 20),
                    )
                    .await
                    .unwrap();
                    assert!(
                        !result.rows.values.is_empty() && !result.more && result.grid.is_none()
                    );
                }
            });
    }
    #[test]
    fn legacy_plain_select_does_not_confirm_and_explain_never_executes_dml() {
        struct Capture(Vec<String>);
        impl Access for Capture {
            fn query<'a>(&'a mut self, sql: &'a str) -> workbench::Query<'a> {
                self.0.push(sql.into());
                Box::pin(async { Ok(Rows::default()) })
            }
        }
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let mut db = Capture(vec![]);
                for sql in ["SELECT * FROM items", "SELECT COUNT(*) FROM items"] {
                    let plan = prepare(&mut db, DatabaseKind::MySql, "fixture", "5.5.62", sql)
                        .await
                        .unwrap();
                    assert!(plan.read_only && !plan.confirm);
                    execute(
                        &mut db,
                        DatabaseKind::MySql,
                        "fixture",
                        "5.5.62",
                        &plan,
                        false,
                        (0, 20),
                    )
                    .await
                    .unwrap();
                }
                assert!(
                    prepare(
                        &mut db,
                        DatabaseKind::MySql,
                        "fixture",
                        "5.5.62",
                        "SELECT custom_write()"
                    )
                    .await
                    .unwrap()
                    .confirm
                );
                for kind in [
                    DatabaseKind::MySql,
                    DatabaseKind::PostgreSql,
                    DatabaseKind::Sqlite,
                ] {
                    db.0.clear();
                    explain(&mut db, kind, "SELECT * FROM items").await.unwrap();
                    assert!(db.0[0].starts_with("EXPLAIN ") && !db.0[0].contains("ANALYZE"));
                    db.0.clear();
                    assert!(explain(&mut db, kind, "DELETE FROM items").await.is_err());
                    assert!(db.0.is_empty());
                }
            });
    }
    #[test]
    fn pagination_preserves_original_literals_and_unicode() {
        for (kind, sql, prefix, suffix) in [
            (
                DatabaseKind::MySql,
                r"SELECT 'a\\b', '中文;字' LIMIT 25;",
                r"SELECT 'a\\b', '中文;字' ",
                "LIMIT 5 OFFSET 20",
            ),
            (
                DatabaseKind::Sqlite,
                "SELECT '中文', 'a; b' -- comment\n",
                "SELECT '中文', 'a; b' -- comment\n",
                "LIMIT 21 OFFSET 20",
            ),
            (
                DatabaseKind::PostgreSql,
                "SELECT '字' OFFSET 5",
                "SELECT '字' ",
                "LIMIT 21 OFFSET 25",
            ),
            (
                DatabaseKind::Sqlite,
                "SELECT '字'\r\nLIMIT 25",
                "SELECT '字'\r\n",
                "LIMIT 5 OFFSET 20",
            ),
        ] {
            let page = page_sql(kind, sql, 20, 20).unwrap();
            assert!(page.starts_with(prefix), "{page:?}");
            assert!(page.ends_with(suffix), "{page:?}");
            parse(kind, &page).unwrap();
        }
    }
    #[test]
    fn sqlite_pages_are_bounded_and_independent() {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            use sqlx::Connection;
            let mut db=sqlx::SqliteConnection::connect("sqlite::memory:").await.unwrap();
            let sql="WITH RECURSIVE numbers(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM numbers WHERE n<45) SELECT n FROM numbers ORDER BY n";
            let plan=prepare(&mut db,DatabaseKind::Sqlite,"main","3",sql).await.unwrap();
            for (offset, count, more) in [(0,20,true),(20,20,true),(40,5,false)] {
                let page=execute(&mut db,DatabaseKind::Sqlite,"main","3",&plan,false,(offset,20)).await.unwrap();
                assert_eq!(page.rows.values.len(),count);
                assert_eq!(page.more,more);
                assert_eq!(page.rows.values[0][0],Some((offset+1).to_string()));
            }
        });
    }
    #[test]
    fn pagination_preserves_limits_and_rejects_multiple_statements() {
        assert!(
            page_sql(DatabaseKind::Sqlite, "SELECT * FROM t LIMIT 25", 20, 20)
                .unwrap()
                .contains("LIMIT 5 OFFSET 20")
        );
        assert!(
            page_sql(DatabaseKind::MySql, "SELECT * FROM t LIMIT 10, 30", 20, 20)
                .unwrap()
                .contains("LIMIT 10 OFFSET 30")
        );
        assert!(page_sql(DatabaseKind::Sqlite, "SELECT 1; DELETE FROM t", 0, 20).is_err());
        assert!(
            page_sql(
                DatabaseKind::PostgreSql,
                "WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d",
                0,
                20
            )
            .is_err()
        );
        assert!(
            page_sql(
                DatabaseKind::MySql,
                "SELECT 1 INTO OUTFILE '/tmp/file'",
                0,
                20
            )
            .is_err()
        );
    }
    #[test]
    fn sqlite_prepare_is_readonly_and_stale_ddl_cannot_execute() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                use sqlx::Connection;
                let mut db = sqlx::SqliteConnection::connect("sqlite::memory:")
                    .await
                    .unwrap();
                sqlx::query("CREATE TABLE items(id INTEGER PRIMARY KEY,v TEXT)")
                    .execute(&mut db)
                    .await
                    .unwrap();
                let plan = prepare(
                    &mut db,
                    DatabaseKind::Sqlite,
                    "main",
                    "3",
                    "INSERT INTO items VALUES(1,'one')",
                )
                .await
                .unwrap();
                assert!(
                    execute(
                        &mut db,
                        DatabaseKind::Sqlite,
                        "main",
                        "3",
                        &plan,
                        false,
                        (0, 20)
                    )
                    .await
                    .is_err()
                );
                execute(
                    &mut db,
                    DatabaseKind::Sqlite,
                    "main",
                    "3",
                    &plan,
                    true,
                    (0, 20),
                )
                .await
                .unwrap();
                let plan = prepare(
                    &mut db,
                    DatabaseKind::Sqlite,
                    "main",
                    "3",
                    "DELETE FROM items",
                )
                .await
                .unwrap();
                sqlx::query("ALTER TABLE items ADD COLUMN changed INT")
                    .execute(&mut db)
                    .await
                    .unwrap();
                assert!(
                    execute(
                        &mut db,
                        DatabaseKind::Sqlite,
                        "main",
                        "3",
                        &plan,
                        true,
                        (0, 20)
                    )
                    .await
                    .is_err()
                );
                let plan = prepare(
                    &mut db,
                    DatabaseKind::Sqlite,
                    "main",
                    "3",
                    "SELECT * FROM items",
                )
                .await
                .unwrap();
                let result = execute(
                    &mut db,
                    DatabaseKind::Sqlite,
                    "main",
                    "3",
                    &plan,
                    false,
                    (0, 20),
                )
                .await
                .unwrap();
                assert_eq!(result.rows.values.len(), 1);
            });
    }
}

#[cfg(test)]
mod command_policy_tests {
    use super::*;
    #[test]
    fn read_only_diagnostics_never_return_grant_authentication_material() {
        struct Fake {
            requests: Vec<String>,
        }
        impl Access for Fake {
            fn query<'a>(&'a mut self, sql: &'a str) -> workbench::Query<'a> {
                self.requests.push(sql.into());
                let rows = if sql == "SHOW GRANTS" {
                    Rows{columns:vec!["grants".into()],values:vec![vec![Some("GRANT SELECT, TRIGGER ON app.* TO user IDENTIFIED BY 'fixture-only'".into())],vec![Some("GRANT `some_role` TO user".into())]]}
                } else if sql.starts_with("SELECT @@GLOBAL") {
                    Rows {
                        columns: vec!["binlog".into(), "trust".into()],
                        values: vec![vec![Some("1".into()), Some("0".into())]],
                    }
                } else {
                    Rows {
                        columns: vec!["v".into(); 3],
                        values: vec![vec![
                            Some("8.4.5".into()),
                            Some("fixture@localhost".into()),
                            Some("app".into()),
                        ]],
                    }
                };
                Box::pin(async move { Ok(rows) })
            }
        }
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let mut c = Fake { requests: vec![] };
                let report = script_diagnostics(&mut c, DatabaseKind::MySql)
                    .await
                    .unwrap();
                assert_eq!(report.direct_privileges, vec!["TRIGGER"]);
                assert!(report.grants_available);
                let json = serde_json::to_string(&report).unwrap();
                assert!(!json.contains("fixture-only"));
                assert!(!json.contains("IDENTIFIED"));
                assert_eq!(c.requests.len(), 3);
                assert!(c.requests[0].starts_with("SELECT VERSION()"));
                assert!(c.requests[1].starts_with("SELECT @@GLOBAL"));
                assert_eq!(c.requests[2], "SHOW GRANTS");
                let before = c.requests.len();
                assert!(
                    script_diagnostics(&mut c, DatabaseKind::Sqlite)
                        .await
                        .is_err()
                );
                assert_eq!(c.requests.len(), before);
                assert_ne!(
                    script_source_fingerprint("DROP TRIGGER a"),
                    script_source_fingerprint("DROP TRIGGER b")
                );
            });
    }
    #[test]
    fn script_errors_and_labels_preserve_safe_server_categories() {
        assert_eq!(
            script_operation_label("-- comment\n/* DDL */ DROP TRIGGER IF EXISTS t;"),
            "DROP TRIGGER"
        );
        struct Denied;
        impl Access for Denied {
            fn query<'a>(&'a mut self, sql: &'a str) -> workbench::Query<'a> {
                let result = if sql == "SELECT @@SESSION.sql_mode" {
                    Ok(Rows {
                        columns: vec!["mode".into()],
                        values: vec![vec![Some(String::new())]],
                    })
                } else if sql.starts_with("SELECT ACTION_STATEMENT") {
                    Ok(Rows::default())
                } else {
                    Err("mysql.error.privilege_1227".into())
                };
                Box::pin(async move { result })
            }
        }
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let mut c = Denied;
                let plan = prepare_script(
                    &mut c,
                    DatabaseKind::MySql,
                    "app",
                    "8.4.5",
                    "DROP TRIGGER IF EXISTS t",
                )
                .await
                .unwrap();
                assert_eq!(
                    execute_script(&mut c, DatabaseKind::MySql, "app", "8.4.5", &plan, true)
                        .await
                        .unwrap_err(),
                    "mysql.error.privilege_1227"
                );
            });
    }
    #[test]
    fn script_variables_and_isolation_are_narrow_and_script_only() {
        for (sql, key) in [
            (
                "SET TRANSACTION ISOLATION LEVEL REPEATABLE READ;",
                "script_transaction",
            ),
            ("SET @active_generation = 0;", "script_variables"),
            (
                "SET @new_generation = @active_generation + 1;",
                "script_variables",
            ),
            (
                "SELECT COALESCE(MAX(`id`), 0) INTO @delta_watermark FROM `recruit_post_count_delta`;",
                "script_variables",
            ),
            (
                "SELECT `active_generation` INTO @active_generation FROM `recruit_post_count_meta` WHERE `id` = 1 FOR UPDATE;",
                "script_lock",
            ),
        ] {
            assert_eq!(
                script_command_required(DatabaseKind::MySql, sql),
                Some(key),
                "{sql}"
            );
            assert_eq!(
                validate_script_statement(DatabaseKind::MySql, sql),
                Ok(false),
                "{sql}"
            );
            assert!(
                parse(DatabaseKind::MySql, sql)
                    .and_then(|s| inspect(&s, DatabaseKind::MySql))
                    .is_err()
            );
        }
        for sql in [
            "SET GLOBAL TRANSACTION ISOLATION LEVEL SERIALIZABLE",
            "SET @a = dangerous_udf()",
            "SELECT id INTO OUTFILE '/tmp/out' FROM t",
            "SELECT dangerous_udf() INTO @a FROM t",
            "SELECT id INTO @a FROM other.t",
            "SELECT id INTO @a FROM t; DELETE FROM t",
        ] {
            assert!(
                validate_script_statement(DatabaseKind::MySql, sql).is_err()
                    || validate_script_target(DatabaseKind::MySql, sql, "app").is_err(),
                "{sql}"
            );
        }
    }
    #[test]
    fn trigger_deletion_is_script_only_and_requires_unchanged_metadata() {
        let sql = "-- dump\nDROP TRIGGER IF EXISTS `app`.`sample_trigger`;";
        assert_eq!(
            script_command_required(DatabaseKind::MySql, sql),
            Some("script_drop_trigger")
        );
        assert!(command_required(DatabaseKind::MySql, sql).is_none());
        assert_eq!(
            validate_script_statement(DatabaseKind::MySql, sql),
            Ok(false)
        );
        assert!(validate_script_target(DatabaseKind::MySql, sql, "other").is_err());
        for bad in [
            "DROP TRIGGER",
            "DROP TRIGGER IF t",
            "DROP TRIGGER a,b",
            "DROP TRIGGER a CASCADE",
            "DROP TRIGGER a; DELETE FROM t",
        ] {
            assert!(validate_script_statement(DatabaseKind::MySql, bad).is_err());
        }
        struct Fake {
            stamp: String,
            writes: usize,
        }
        impl Access for Fake {
            fn query<'a>(&'a mut self, sql: &'a str) -> workbench::Query<'a> {
                let rows = if sql == "SELECT @@SESSION.sql_mode" {
                    Rows {
                        columns: vec!["mode".into()],
                        values: vec![vec![Some(String::new())]],
                    }
                } else if sql.starts_with("SELECT ACTION_STATEMENT") {
                    Rows {
                        columns: vec!["definition".into()],
                        values: vec![vec![Some(self.stamp.clone())]],
                    }
                } else if sql.starts_with("SHOW CREATE TABLE") {
                    Rows {
                        columns: vec!["Table".into(), "Create Table".into()],
                        values: vec![vec![
                            Some("t".into()),
                            Some("CREATE TABLE t(id INT)".into()),
                        ]],
                    }
                } else {
                    self.writes += 1;
                    Rows::default()
                };
                Box::pin(async move { Ok(rows) })
            }
        }
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let mut c = Fake {
                    stamp: "original".into(),
                    writes: 0,
                };
                let plan = prepare_script(&mut c, DatabaseKind::MySql, "app", "8.4.5", sql)
                    .await
                    .unwrap();
                assert!(plan.confirm && !plan.read_only);
                assert_eq!(plan.guards[0].object.kind, "trigger");
                assert!(
                    execute_script(&mut c, DatabaseKind::MySql, "app", "8.4.5", &plan, false)
                        .await
                        .is_err()
                );
                assert_eq!(c.writes, 0);
                c.stamp = "changed".into();
                assert!(
                    execute_script(&mut c, DatabaseKind::MySql, "app", "8.4.5", &plan, true)
                        .await
                        .is_err()
                );
                assert_eq!(c.writes, 0);
                c.stamp = "original".into();
                let result =
                    execute_script(&mut c, DatabaseKind::MySql, "app", "8.4.5", &plan, true)
                        .await
                        .unwrap();
                assert_eq!(c.writes, 1);
                assert_eq!(result.execution.unwrap().operation, "DROP TRIGGER");
                let sql = "SELECT COALESCE(MAX(id),0) INTO @watermark FROM t FOR UPDATE";
                let plan = prepare_script(&mut c, DatabaseKind::MySql, "app", "8.4.5", sql)
                    .await
                    .unwrap();
                assert_eq!(plan.sql, sql);
                assert!(plan.confirm && !plan.read_only);
                assert!(
                    execute_script(&mut c, DatabaseKind::MySql, "app", "8.4.5", &plan, false)
                        .await
                        .is_err()
                );
                let result =
                    execute_script(&mut c, DatabaseKind::MySql, "app", "8.4.5", &plan, true)
                        .await
                        .unwrap();
                assert_eq!(result.execution.unwrap().operation, "SELECT INTO");
                assert_eq!(c.writes, 2);
            });
    }
    #[test]
    fn mysql_dump_keys_require_alter_confirmation_and_recheck_schema() {
        let sql = "/*!40000 ALTER TABLE `account` ENABLE KEYS */;";
        assert_eq!(
            script_command_required(DatabaseKind::MySql, sql),
            Some("alter")
        );
        assert_eq!(
            validate_script_statement(DatabaseKind::MySql, sql),
            Ok(false)
        );
        assert!(
            validate_script_target(
                DatabaseKind::MySql,
                "ALTER TABLE other.account DISABLE KEYS",
                "app"
            )
            .is_err()
        );
        assert!(
            workbench::script_context::keys_target(
                "ALTER TABLE account ENABLE KEYS; DROP TABLE account"
            )
            .is_err()
        );
        struct Fake {
            ddl: String,
            executed: usize,
        }
        impl Access for Fake {
            fn query<'a>(&'a mut self, sql: &'a str) -> workbench::Query<'a> {
                let rows = if sql == "SELECT @@SESSION.sql_mode" {
                    Rows {
                        columns: vec!["mode".into()],
                        values: vec![vec![Some(String::new())]],
                    }
                } else if sql.starts_with("SHOW CREATE TABLE") {
                    Rows {
                        columns: vec!["Table".into(), "Create Table".into()],
                        values: vec![vec![Some("account".into()), Some(self.ddl.clone())]],
                    }
                } else {
                    self.executed += 1;
                    Rows::default()
                };
                Box::pin(async move { Ok(rows) })
            }
        }
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let mut fake = Fake {
                    ddl: "CREATE TABLE account(id INT)".into(),
                    executed: 0,
                };
                let plan = prepare_script(&mut fake, DatabaseKind::MySql, "app", "8.0.32", sql)
                    .await
                    .unwrap();
                assert!(
                    execute_script(
                        &mut fake,
                        DatabaseKind::MySql,
                        "app",
                        "8.0.32",
                        &plan,
                        false
                    )
                    .await
                    .is_err()
                );
                assert_eq!(fake.executed, 0);
                execute_script(&mut fake, DatabaseKind::MySql, "app", "8.0.32", &plan, true)
                    .await
                    .unwrap();
                assert_eq!(fake.executed, 1);
                fake.ddl = "CREATE TABLE account(id INT,changed INT)".into();
                assert!(
                    execute_script(&mut fake, DatabaseKind::MySql, "app", "8.0.32", &plan, true)
                        .await
                        .is_err()
                );
                assert_eq!(fake.executed, 1);
            });
    }
    #[test]
    fn mysql_script_unknown_functions_require_confirmation_without_readonly_wrappers() {
        assert!(
            script_requires_confirmation(DatabaseKind::MySql, "SELECT risky_udf()", false).unwrap()
        );
        assert!(
            !script_requires_confirmation(DatabaseKind::MySql, "SELECT COUNT(*) FROM t", false)
                .unwrap()
        );
        struct Fake {
            calls: Vec<String>,
        }
        impl Access for Fake {
            fn query<'a>(&'a mut self, sql: &'a str) -> workbench::Query<'a> {
                self.calls.push(sql.into());
                Box::pin(async {
                    Ok(Rows {
                        columns: vec!["mode".into()],
                        values: vec![vec![Some(String::new())]],
                    })
                })
            }
        }
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let mut fake = Fake { calls: vec![] };
                let plan = prepare_script(
                    &mut fake,
                    DatabaseKind::MySql,
                    "app",
                    "8.0.32",
                    "SELECT risky_udf()",
                )
                .await
                .unwrap();
                assert!(plan.confirm);
                assert!(
                    execute_script(
                        &mut fake,
                        DatabaseKind::MySql,
                        "app",
                        "8.0.32",
                        &plan,
                        false
                    )
                    .await
                    .is_err()
                );
                assert!(
                    fake.calls
                        .iter()
                        .all(|sql| sql == "SELECT @@SESSION.sql_mode")
                );
            });
    }
    #[test]
    fn script_executes_original_sql_in_one_sqlite_session_and_requires_confirmation() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                use sqlx::Connection;
                let mut db = sqlx::SqliteConnection::connect("sqlite::memory:")
                    .await
                    .unwrap();
                for sql in [
                    "CREATE TABLE script_test(id INTEGER PRIMARY KEY)",
                    "INSERT INTO script_test VALUES (1),(2),(3)",
                ] {
                    let plan = prepare(&mut db, DatabaseKind::Sqlite, "main", "3", sql)
                        .await
                        .unwrap();
                    assert!(
                        execute_script(&mut db, DatabaseKind::Sqlite, "main", "3", &plan, false)
                            .await
                            .is_err()
                    );
                    execute_script(&mut db, DatabaseKind::Sqlite, "main", "3", &plan, true)
                        .await
                        .unwrap();
                }
                let plan = prepare(
                    &mut db,
                    DatabaseKind::Sqlite,
                    "main",
                    "3",
                    "SELECT id FROM script_test ORDER BY id LIMIT 1 OFFSET 1",
                )
                .await
                .unwrap();
                let result =
                    execute_script(&mut db, DatabaseKind::Sqlite, "main", "3", &plan, false)
                        .await
                        .unwrap();
                assert_eq!(result.rows.values, vec![vec![Some("2".into())]]);
                assert!(result.grid.is_none());
                let plan = prepare(
                    &mut db,
                    DatabaseKind::Sqlite,
                    "main",
                    "3",
                    "DELETE FROM script_test",
                )
                .await
                .unwrap();
                sqlx::query("ALTER TABLE script_test ADD COLUMN changed INT")
                    .execute(&mut db)
                    .await
                    .unwrap();
                assert!(
                    execute_script(&mut db, DatabaseKind::Sqlite, "main", "3", &plan, true)
                        .await
                        .is_err()
                );
            });
    }
    #[test]
    fn categories_use_parsed_statements_and_metadata_whitelist() {
        let cases = [
            ("-- comment\nSELECT 1", Some("select")),
            ("# comment\nSHOW CREATE TABLE users", Some("show_create")),
            ("SHOW INDEX FROM users", Some("show_columns")),
            ("UPDATE users SET name='a'", Some("update")),
            ("WITH t AS (SELECT 1) SELECT * FROM t", Some("select")),
            ("SHOW VARIABLES", Some("show_variables")),
            (
                "SHOW VARIABLES LIKE 'skip_show_database'",
                Some("show_variables"),
            ),
            (
                "SHOW SESSION VARIABLES LIKE 'character_set_%'",
                Some("show_variables"),
            ),
            ("SHOW GLOBAL STATUS LIKE 'Threads_%'", Some("show_status")),
            (
                "SHOW DATABASES; -- manual detection",
                Some("show_databases"),
            ),
            ("SHOW SCHEMAS LIKE 'test%'", Some("show_databases")),
            ("SHOW DATABASES WHERE sleep(1)", None),
            ("SHOW VARIABLES LIKE 'x'; SET GLOBAL x=1", None),
            ("SHOW VARIABLES LIKE concat('a','b')", None),
            ("SHOW VARIABLES LIKE 'x\\'; DROP TABLE users; -- ';", None),
            ("SHOW DATABASES /*!50000 WHERE sleep(1) */", None),
            ("SHOW SESSION DATABASES", None),
            ("SELECT 1; DROP TABLE users", None),
            ("/*!50000 DROP TABLE users */", None),
            ("SET GLOBAL x=1", None),
        ];
        for (sql, expected) in cases {
            assert_eq!(
                command_required(DatabaseKind::MySql, sql),
                expected,
                "{sql}"
            );
        }
        assert!(COMMANDS.iter().all(|(key, _, default)| {
            !*default
                || ![
                    "insert", "update", "delete", "drop", "truncate", "create", "alter",
                ]
                .contains(key)
        }));
    }
}

#[cfg(test)]
mod table_entry_tests {
    use super::*;
    #[test]
    fn table_entry_auto_runs_only_unambiguous_reads() {
        for kind in [
            DatabaseKind::MySql,
            DatabaseKind::PostgreSql,
            DatabaseKind::Sqlite,
        ] {
            assert!(automatic_table_read(
                kind,
                "SELECT * FROM items WHERE price > 1"
            ));
            for sql in [
                "DELETE FROM items",
                "UPDATE items SET price=1",
                "SELECT * FROM items; DELETE FROM items",
                "SELECT dangerous_function()",
            ] {
                assert!(!automatic_table_read(kind, sql));
            }
        }
        assert!(!automatic_table_read(
            DatabaseKind::MySql,
            "SELECT * FROM items FOR UPDATE"
        ));
        assert!(!automatic_table_read(
            DatabaseKind::MySql,
            "SELECT price INTO @p FROM items"
        ));
    }
}

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
pub use result_grid::{GridAction, GridSource, grid_request};

pub const DEFAULT_PAGE_SIZE: usize = 20;
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
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResultPage {
    #[serde(default)]
    pub grid: Option<GridSource>,
    pub rows: Rows,
    pub more: bool,
    pub offset: usize,
    pub elapsed_ms: u64,
    pub read_only: bool,
}
fn parse(kind: DatabaseKind, sql: &str) -> Result<Statement, String> {
    if sql.trim().is_empty()
        || sql.len() > 60_000
        || sql.contains('\0')
        || sql.contains("/*!")
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
    writes: bool,
    blocked: bool,
    tables: BTreeSet<Vec<String>>,
}
impl Visitor for Inspection {
    type Break = ();
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
        return Err("当前笔记支持单条查询及表级 INSERT/UPDATE/DELETE/CREATE/ALTER/DROP/TRUNCATE；不支持会话控制、文件操作或带锁查询".into());
    }
    if matches!(s, Statement::Query(_)) && info.writes {
        return Err("分页查询不能包含写入型 CTE".into());
    }
    Ok(info)
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
async fn resolved_object(
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
    let literal = workbench::literal_for(kind, &o.name);
    let sql = match kind {
        DatabaseKind::Sqlite => format!(
            "SELECT sql FROM sqlite_schema WHERE name={literal} COLLATE NOCASE AND type IN ('table','view')"
        ),
        DatabaseKind::MySql => format!(
            "SHOW TABLES FROM {} LIKE {literal}",
            workbench::quote(kind, database)
        ),
        DatabaseKind::PostgreSql => format!(
            "SELECT table_name FROM information_schema.tables WHERE table_schema={} AND table_name={literal}",
            workbench::literal_for(kind, &o.schema)
        ),
        _ => return Err("不支持该数据库类型".into()),
    };
    let rows = c.query(&sql).await?;
    if rows.values.is_empty()
        || (kind == DatabaseKind::MySql
            && !rows.values.iter().any(|row| {
                row.first()
                    .and_then(|v| v.as_ref())
                    .is_some_and(|name| name.eq_ignore_ascii_case(&o.name))
            }))
    {
        return Ok(None);
    }
    if kind == DatabaseKind::Sqlite {
        return Ok(rows.values[0][0].clone());
    }
    Ok(Some(
        workbench::schema(c, kind, database, o, true).await?.ddl,
    ))
}
pub async fn prepare(
    c: &mut impl Access,
    kind: DatabaseKind,
    database: &str,
    version: &str,
    sql: &str,
) -> Result<Plan, String> {
    let stmt = parse(kind, sql)?;
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
        confirm: info.writes || legacy,
        guards,
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
    let (offset, size) = page;
    let stmt = parse(kind, &plan.sql)?;
    let info = inspect(&stmt, kind)?;
    if info.writes == plan.read_only {
        return Err("SQL 预览状态无效，请重新执行".into());
    }
    let legacy = kind == DatabaseKind::MySql && !crate::mysql::modern_version(version);
    if (info.writes || legacy) && !confirmed {
        return Err("此 SQL 必须先预览并确认".into());
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
    let sql = if plan.read_only {
        page_sql(kind, &plan.sql, offset, size)?
    } else {
        plan.sql.clone()
    };
    if plan.read_only && !legacy {
        c.query(match kind {
            DatabaseKind::Sqlite => "PRAGMA query_only=ON",
            DatabaseKind::PostgreSql => "BEGIN READ ONLY",
            _ => "START TRANSACTION READ ONLY",
        })
        .await?;
    }
    let result = c.query(&sql).await;
    if plan.read_only && !legacy {
        let _ = c
            .query(if kind == DatabaseKind::Sqlite {
                "PRAGMA query_only=OFF"
            } else {
                "ROLLBACK"
            })
            .await;
    }
    let mut rows = result.map_err(|_| {
        "SQL 执行失败，请检查语法、权限及对象是否存在；写入可能已生效，请勿盲目重试".to_string()
    })?;
    if rows.columns.len() > 512 {
        return Err("查询结果超过 512 列，请减少选择列".into());
    }
    let more = plan.read_only && rows.values.len() > size;
    rows.values.truncate(size);
    let grid = if plan.read_only {
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
    Ok(ResultPage {
        grid,
        rows,
        more,
        offset,
        elapsed_ms: start.elapsed().as_millis() as u64,
        read_only: plan.read_only,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
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

//! ClickHouse statement envelopes. Preserve original SQL; the server validates its grammar.
//! Never infer a permission from words inside expressions, strings or comments.
use sqlparser::{
    dialect::ClickHouseDialect,
    tokenizer::{Token, Tokenizer},
};

#[derive(Debug, Clone)]
pub struct Command {
    pub permission: &'static str,
    pub read: bool,
    pub targets: Vec<(String, String)>,
    pub mutation: bool,
    pub control: bool,
    pub object_kind: &'static str,
}
pub fn http_error(status: u16, code: Option<&str>) -> String {
    match code.and_then(|c| c.parse::<u32>().ok()) {
        Some(497 | 516) | None if status == 401 || status == 403 => "clickhouse.http.permission",
        Some(497 | 516) => "clickhouse.http.permission",
        Some(60 | 81) => "clickhouse.http.object",
        Some(62) => "clickhouse.http.syntax",
        _ => "clickhouse.http.execution",
    }
    .into()
}
fn word(t: Option<&Token>) -> String {
    match t {
        Some(Token::Word(w)) if w.quote_style.is_none() => w.value.to_ascii_uppercase(),
        _ => String::new(),
    }
}
fn ident(t: Option<&Token>) -> Result<String, String> {
    match t {
        Some(Token::Word(w)) => Ok(w.value.clone()),
        _ => Err("clickhouse.sql.target".into()),
    }
}
fn target(ts: &[Token], at: &mut usize, database: &str) -> Result<(String, String), String> {
    let first = ident(ts.get(*at))?;
    *at += 1;
    if ts.get(*at) == Some(&Token::Period) {
        *at += 1;
        let name = ident(ts.get(*at))?;
        *at += 1;
        if !database.is_empty() && first != database {
            return Err("script.error.database_mismatch".into());
        }
        Ok((first, name))
    } else {
        Ok((database.into(), first))
    }
}
pub fn tokens(sql: &str) -> Result<Vec<Token>, String> {
    if sql.is_empty() || sql.len() > super::notebook_query::SCRIPT_MAX_BYTES || sql.contains('\0') {
        return Err("script.error.statement_size".into());
    }
    let mut ts = Tokenizer::new(&ClickHouseDialect {}, sql)
        .tokenize()
        .map_err(|_| "script.error.syntax")?
        .into_iter()
        .filter(|t| !matches!(t, Token::Whitespace(_)))
        .collect::<Vec<_>>();
    if ts.last() == Some(&Token::SemiColon) {
        ts.pop();
    }
    if ts.is_empty() || ts.contains(&Token::SemiColon) {
        return Err("script.error.syntax".into());
    }
    Ok(ts)
}
pub fn parse(sql: &str) -> Result<sqlparser::ast::Statement, String> {
    use sqlparser::{ast::Statement, parser::Parser};
    let parse = |sql: &str| {
        Parser::new(&ClickHouseDialect {})
            .with_recursion_limit(64)
            .try_with_sql(sql)
            .and_then(|mut p| p.parse_statements())
    };
    if let Ok(mut statements) = parse(sql) {
        if statements.len() == 1 {
            return Ok(statements.remove(0));
        }
        return Err("script.error.syntax".into());
    }
    // sqlparser treats SETTINGS as an implicit projection alias on SELECT
    // without FROM. Parse that suffix independently, then attach its AST;
    // never add a synthetic FROM to the SQL sent to ClickHouse.
    let ts = Tokenizer::new(&ClickHouseDialect {}, sql)
        .tokenize_with_location()
        .map_err(|_| "script.error.syntax")?;
    let mut depth = 0;
    for t in ts {
        match &t.token {
            Token::LParen => depth += 1,
            Token::RParen => depth -= 1,
            _ => {}
        }
        if depth == 0 && word(Some(&t.token)) == "SETTINGS" {
            let (mut line, mut column) = (1, 1);
            let at = sql
                .char_indices()
                .find_map(|(i, c)| {
                    let found = (line, column) == (t.span.start.line, t.span.start.column);
                    if c == '\n' {
                        line += 1;
                        column = 1;
                    } else {
                        column += 1;
                    }
                    found.then_some(i)
                })
                .ok_or("script.error.syntax")?;
            let mut core = parse(&sql[..at]).map_err(|_| "script.error.syntax")?;
            let mut suffix = parse(&format!("SELECT * FROM _t {}", &sql[at..]))
                .map_err(|_| "script.error.syntax")?;
            if core.len() != 1 || suffix.len() != 1 {
                return Err("script.error.syntax".into());
            }
            if let (Statement::Query(q), Statement::Query(extra)) = (&mut core[0], suffix.remove(0))
            {
                q.settings = extra.settings;
                q.format_clause = extra.format_clause;
                return Ok(core.remove(0));
            }
            break;
        }
    }
    Err("script.error.syntax".into())
}
pub fn classify(sql: &str, database: &str) -> Result<Command, String> {
    let ts = tokens(sql)?;
    // Local file clauses are client operations, not server paths. Inline FORMAT data
    // belongs to the streaming file importer, never to the SQL statement splitter.
    if ts.windows(2).any(|t| {
        matches!(
            (word(t.first()).as_str(), word(t.get(1)).as_str()),
            ("INTO", "OUTFILE") | ("FROM", "INFILE")
        )
    }) {
        return Err("clickhouse.sql.file_clause".into());
    }
    let first = word(ts.first());
    let second = word(ts.get(1));
    if matches!(first.as_str(), "CREATE" | "ALTER" | "ATTACH")
        && (matches!(second.as_str(), "USER" | "DICTIONARY" | "NAMED")
            || (second == "OR" && ts.iter().take(6).any(|t| word(Some(t)) == "DICTIONARY")))
        && ts.iter().any(|t| {
            matches!(
                word(Some(t)).as_str(),
                "IDENTIFIED" | "PASSWORD" | "SECRET_ACCESS_KEY" | "ACCESS_KEY_ID"
            )
        })
    {
        return Err("clickhouse.sql.credentials".into());
    }
    if matches!(first.as_str(), "BACKUP" | "RESTORE")
        && ts
            .iter()
            .any(|t| matches!(word(Some(t)).as_str(), "S3" | "AZUREBLOBSTORAGE"))
    {
        return Err("clickhouse.sql.credentials".into());
    }

    let mut c = Command {
        permission: "clickhouse_admin",
        read: false,
        targets: vec![],
        mutation: false,
        control: false,
        object_kind: "table",
    };
    let mut at = 1;
    match first.as_str() {
        "SELECT" | "WITH" | "EXPLAIN" => {
            c.permission = if first == "EXPLAIN" {
                "explain"
            } else {
                "select"
            };
            c.read = true;
            // A complete query AST is required: prevents WITH ... INSERT or other
            // write expressions from acquiring SELECT permission.
            let ast = parse(sql)?;
            if !matches!(
                ast,
                sqlparser::ast::Statement::Query(_) | sqlparser::ast::Statement::Explain { .. }
            ) {
                return Err("script.error.syntax".into());
            }
            if first == "EXPLAIN" && ts.iter().any(|t| word(Some(t)) == "ANALYZE") {
                return Err("notes.explain.analyze_unsupported".into());
            }
        }
        "SHOW" | "DESCRIBE" | "DESC" | "EXISTS" | "CHECK" => {
            c.permission = match second.as_str() {
                "CREATE" => "show_create",
                "DATABASES" => "show_databases",
                "TABLES" => "show_tables",
                _ => "show_columns",
            };
            c.read = true;
            if first == "SHOW"
                && second == "CREATE"
                && matches!(word(ts.get(2)).as_str(), "USER" | "ROLE")
            {
                return Err("clickhouse.sql.credentials".into());
            }
        }
        "USE" => {
            if ts.len() != 2 || (!database.is_empty() && ident(ts.get(1))? != database) {
                return Err("script.error.database_mismatch".into());
            }
            c.permission = "script_session";
            c.control = true;
            c.read = true;
        }
        "SET" => {
            if ts.len() < 3 {
                return Err("script.error.syntax".into());
            }
            c.permission = "script_session";
            c.control = true;
        }
        "INSERT" => {
            if second != "INTO" {
                return Err("script.error.syntax".into());
            }
            at = 2;
            if word(ts.get(at)) == "FUNCTION" {
                return Err("clickhouse.sql.target".into());
            }
            c.targets.push(target(&ts, &mut at, database)?);
            c.permission = "insert";
            if ts.iter().any(|t| word(Some(t)) == "FORMAT") {
                return Err("clickhouse.sql.inline_format".into());
            }
            if !ts[at..]
                .iter()
                .any(|t| matches!(word(Some(t)).as_str(), "VALUES" | "SELECT" | "WITH"))
            {
                return Err("script.error.syntax".into());
            }
        }
        "UPDATE" | "DELETE" => {
            if first == "DELETE" {
                if second != "FROM" {
                    return Err("script.error.syntax".into());
                }
                at = 2;
            }
            c.targets.push(target(&ts, &mut at, database)?);
            c.permission = if first == "UPDATE" {
                "update"
            } else {
                "delete"
            };
            c.mutation = true;
        }
        "CREATE" | "ATTACH" | "ALTER" | "DROP" | "DETACH" | "TRUNCATE" => {
            if word(ts.get(at)) == "OR" && word(ts.get(at + 1)) == "REPLACE" {
                at += 2;
            }
            if matches!(
                word(ts.get(at)).as_str(),
                "TEMPORARY" | "MATERIALIZED" | "LIVE" | "WINDOW" | "REFRESHABLE"
            ) {
                at += 1;
            }
            let object = word(ts.get(at));
            at += 1;
            if !matches!(
                object.as_str(),
                "TABLE"
                    | "VIEW"
                    | "DICTIONARY"
                    | "DATABASE"
                    | "FUNCTION"
                    | "USER"
                    | "ROLE"
                    | "QUOTA"
                    | "ROW"
                    | "SETTINGS"
                    | "NAMED"
            ) {
                return Err("script.error.unsupported".into());
            }
            if word(ts.get(at)) == "IF" {
                at += 1;
                if word(ts.get(at)) == "NOT" {
                    at += 1;
                }
                if word(ts.get(at)) != "EXISTS" {
                    return Err("script.error.syntax".into());
                }
                at += 1;
            }
            if matches!(object.as_str(), "TABLE" | "VIEW" | "DICTIONARY") {
                c.targets.push(target(&ts, &mut at, database)?);
            } else if object == "DATABASE" {
                let name = ident(ts.get(at))?;
                if first != "CREATE" && !database.is_empty() && name != database {
                    return Err("script.error.database_mismatch".into());
                }
                c.object_kind = "database";
                c.targets.push((String::new(), name));
            }
            c.permission = match first.as_str() {
                "CREATE" => "create",
                "ALTER" => "alter",
                "DROP" | "TRUNCATE" => {
                    if first == "DROP" {
                        "drop"
                    } else {
                        "truncate"
                    }
                }
                _ => "clickhouse_admin",
            };
            if !matches!(
                object.as_str(),
                "TABLE" | "VIEW" | "DICTIONARY" | "DATABASE"
            ) {
                c.permission = "clickhouse_admin";
            }
            // ON CLUSTER follows the object. Classify the action, not identifiers
            // or UPDATE/DELETE words nested in an expression.
            if word(ts.get(at)) == "ON" && word(ts.get(at + 1)) == "CLUSTER" {
                at += 3;
            }
            if first == "ALTER" && object == "TABLE" {
                match word(ts.get(at)).as_str() {
                    "UPDATE" => {
                        c.permission = "update";
                        c.mutation = true;
                    }
                    "DELETE" => {
                        c.permission = "delete";
                        c.mutation = true;
                    }
                    _ => {}
                }
                // Mixed ALTER actions retain DBA permission; never let UPDATE
                // grant access to a later DROP/DELETE/partition operation.
                let mut depth = 0;
                for i in at..ts.len() {
                    match ts[i] {
                        Token::LParen => depth += 1,
                        Token::RParen => depth -= 1,
                        Token::Comma if depth == 0 => {
                            if matches!(
                                word(ts.get(i + 1)).as_str(),
                                "DROP"
                                    | "DELETE"
                                    | "ADD"
                                    | "MODIFY"
                                    | "ATTACH"
                                    | "DETACH"
                                    | "MOVE"
                                    | "REPLACE"
                                    | "CLEAR"
                                    | "MATERIALIZE"
                            ) {
                                c.permission = "alter";
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        "RENAME" | "EXCHANGE" => {
            if !matches!(
                second.as_str(),
                "TABLE" | "TABLES" | "DICTIONARY" | "DATABASE"
            ) {
                return Err("script.error.syntax".into());
            }
            at = 2;
            loop {
                c.targets.push(target(&ts, &mut at, database)?);
                if matches!(word(ts.get(at)).as_str(), "TO" | "AND") {
                    at += 1;
                    c.targets.push(target(&ts, &mut at, database)?);
                } else {
                    return Err("script.error.syntax".into());
                }
                if ts.get(at) == Some(&Token::Comma) {
                    at += 1;
                } else {
                    break;
                }
                if c.targets.len() > 32 {
                    return Err("notes.sql_scope_unsupported".into());
                }
            }
        }
        "OPTIMIZE" => {
            if second != "TABLE" {
                return Err("script.error.syntax".into());
            }
            at = 2;
            c.targets.push(target(&ts, &mut at, database)?);
        }
        "SYSTEM" | "KILL" | "BACKUP" | "RESTORE" | "GRANT" | "REVOKE" => {
            if ts.len() < 3 {
                return Err("script.error.syntax".into());
            }
            if matches!(first.as_str(), "BACKUP" | "RESTORE") {
                at = 1;
                while matches!(
                    word(ts.get(at)).as_str(),
                    "TABLE" | "DICTIONARY" | "DATABASE"
                ) {
                    let object = word(ts.get(at));
                    at += 1;
                    if object == "DATABASE" {
                        if !database.is_empty() && ident(ts.get(at))? != database {
                            return Err("script.error.database_mismatch".into());
                        }
                        at += 1;
                    } else {
                        c.targets.push(target(&ts, &mut at, database)?);
                    }
                    if word(ts.get(at)) == "AS" {
                        at += 1;
                        c.targets.push(target(&ts, &mut at, database)?);
                    }
                    if ts.get(at) == Some(&Token::Comma) {
                        at += 1;
                    } else {
                        break;
                    }
                }
                if second == "ALL" {
                    return Err("clickhouse.sql.backup_scope".into());
                }
            }
        }
        _ => return Err("script.error.unsupported".into()),
    }
    if matches!(first.as_str(), "ALTER" | "CREATE" | "ATTACH") && c.object_kind == "table" {
        for i in at..ts.len().saturating_sub(1) {
            let next = if word(ts.get(i)) == "TO" && word(ts.get(i + 1)) == "TABLE" {
                Some(i + 2)
            } else if (first == "ALTER"
                && word(ts.get(i)) == "FROM"
                && matches!(ts.get(i + 1), Some(Token::Word(_))))
                || (matches!(first.as_str(), "CREATE" | "ATTACH")
                    && matches!(second.as_str(), "MATERIALIZED" | "VIEW")
                    && word(ts.get(i)) == "TO"
                    && matches!(ts.get(i + 1), Some(Token::Word(_)))
                    && !matches!(word(ts.get(i + 1)).as_str(), "DISK" | "VOLUME"))
            {
                Some(i + 1)
            } else {
                None
            };
            if let Some(mut j) = next {
                let t = target(&ts, &mut j, database)?;
                if !c.targets.contains(&t) {
                    c.targets.push(t);
                }
            }
        }
    }
    if ts
        .windows(2)
        .any(|w| word(w.first()) == "ON" && word(w.get(1)) == "CLUSTER")
    {
        c.permission = "clickhouse_admin";
        c.read = false;
        c.mutation = true;
    }
    if ts.iter().any(|t| word(Some(t)) == "ASYNC") {
        c.mutation = true;
    }
    Ok(c)
}

use super::{
    notebook_query::{ExecutionSummary, Guard, Plan, ResultPage},
    workbench::{Access, Object, Rows},
};
async fn stamp(
    c: &mut impl Access,
    database: &str,
    name: &str,
    kind: &str,
) -> Result<Option<String>, String> {
    let literal = |s: &str| super::workbench::literal_for(crate::DatabaseKind::ClickHouse, s);
    if kind == "database" {
        let rows = c
            .query(&format!(
                "SELECT name, engine FROM system.databases WHERE name={}",
                literal(name)
            ))
            .await?;
        return if rows.values.is_empty() {
            Ok(None)
        } else {
            serde_json::to_string(&rows.values)
                .map(Some)
                .map_err(|_| "clickhouse.http.response".into())
        };
    }
    let rows = c
        .query(&format!(
            "SELECT create_table_query FROM system.tables WHERE database={} AND name={}",
            literal(database),
            literal(name)
        ))
        .await?;
    Ok(rows
        .values
        .first()
        .and_then(|r| r.first())
        .cloned()
        .flatten())
}
pub async fn prepare(c: &mut impl Access, database: &str, sql: &str) -> Result<Plan, String> {
    let command = classify(sql, database)?;
    let mut guards = vec![];
    if !command.read {
        for (db, name) in command.targets {
            let stamp = stamp(c, &db, &name, command.object_kind).await?;
            guards.push(Guard {
                object: Object {
                    schema: db,
                    name,
                    kind: command.object_kind.into(),
                    comment: String::new(),
                    parent: String::new(),
                },
                stamp,
            });
        }
    }
    Ok(Plan {
        sql: sql.trim().into(),
        read_only: command.read,
        confirm: !command.read,
        guards,
    })
}
pub async fn execute(
    c: &mut impl Access,
    database: &str,
    plan: &Plan,
    confirmed: bool,
    page: (usize, usize),
) -> Result<ResultPage, String> {
    let command = classify(&plan.sql, database)?;
    let expected = prepare(c, database, &plan.sql).await?;
    if plan.read_only != expected.read_only
        || plan.confirm != expected.confirm
        || plan.guards.len() != expected.guards.len()
        || plan
            .guards
            .iter()
            .zip(&expected.guards)
            .any(|(a, b)| a.object != b.object || a.stamp != b.stamp)
    {
        return Err("notes.preview_changed".into());
    }
    if plan.confirm && !confirmed {
        return Err("notes.preview_changed".into());
    }
    let (offset, size) = page;
    if !(1..=1000).contains(&size) || (!command.read && offset != 0) {
        return Err("notes.preview_changed".into());
    }
    let started = std::time::Instant::now();
    let sql = if command.permission == "select" {
        super::notebook_query::page_sql(crate::DatabaseKind::ClickHouse, &plan.sql, offset, size)?
    } else {
        if offset != 0 {
            return Err("notes.metadata.invalid".into());
        }
        plan.sql.clone()
    };
    let mut rows = c.query(&sql).await?;
    if rows.columns.len() > 512 {
        return Err("notes.metadata.limit".into());
    }
    let more = command.read && rows.values.len() > size;
    rows.values.truncate(size);
    let elapsed_ms = started.elapsed().as_millis() as u64;
    Ok(ResultPage {
        execution: (!command.read || command.control).then(|| {
            Box::new(ExecutionSummary {
                operation: command.permission.to_ascii_uppercase(),
                // Mutation settings can change synchronicity; an acknowledgement alone
                // is deliberately never presented as completed data modification.
                status: if command.mutation {
                    "submitted"
                } else {
                    "succeeded"
                }
                .into(),
                elapsed_ms,
                objects: plan.guards.iter().map(|g| g.object.clone()).collect(),
                ..Default::default()
            })
        }),
        affected_rows: None,
        grid: None,
        rows: if command.read { rows } else { Rows::default() },
        more,
        offset,
        elapsed_ms,
        read_only: command.read,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DatabaseKind, access_mode::AccessMode, database::notebook_query as note};
    #[test]
    fn dba_envelopes_preserve_command_permissions() {
        for (sql, key) in [
            (
                "CREATE DATABASE IF NOT EXISTS fixture ENGINE=Atomic",
                "create",
            ),
            (
                "CREATE TABLE fixture.items (id UInt64, label Nullable(String), tags Array(String), v Decimal(18,4), stamp DateTime64(3,'Asia/Shanghai')) ENGINE=MergeTree PARTITION BY toYYYYMM(stamp) ORDER BY id TTL stamp + INTERVAL 1 MONTH SETTINGS index_granularity=8192",
                "create",
            ),
            (
                "CREATE MATERIALIZED VIEW fixture.mv TO fixture.target AS SELECT id FROM fixture.items",
                "create",
            ),
            (
                "CREATE DICTIONARY fixture.dict (id UInt64) PRIMARY KEY id SOURCE(CLICKHOUSE(TABLE 'items')) LIFETIME(60) LAYOUT(HASHED())",
                "create",
            ),
            (
                "CREATE FUNCTION norm AS x -> lowerUTF8(x)",
                "clickhouse_admin",
            ),
            (
                "ALTER TABLE fixture.items UPDATE label='x' WHERE id IN (1,2) AND stamp>=toDateTime('2026-08-25 00:00:00','Asia/Shanghai')",
                "update",
            ),
            ("ALTER TABLE fixture.items DELETE WHERE id=1", "delete"),
            ("UPDATE fixture.items SET label='x' WHERE id=1", "update"),
            ("DELETE FROM fixture.items WHERE id=1", "delete"),
            (
                "ALTER TABLE fixture.items ADD COLUMN x LowCardinality(String)",
                "alter",
            ),
            (
                "ALTER TABLE fixture.items MATERIALIZE INDEX ix IN PARTITION 202610",
                "alter",
            ),
            (
                "ALTER TABLE fixture.items REPLACE PARTITION 202610 FROM fixture.staging",
                "alter",
            ),
            (
                "ALTER TABLE fixture.items MOVE PARTITION 202610 TO TABLE fixture.archive",
                "alter",
            ),
            (
                "ALTER TABLE fixture.items FREEZE WITH NAME 'snapshot'",
                "alter",
            ),
            ("ATTACH TABLE fixture.items", "clickhouse_admin"),
            ("DETACH TABLE fixture.items PERMANENTLY", "clickhouse_admin"),
            (
                "RENAME TABLE fixture.items TO fixture.renamed, fixture.old TO fixture.backup",
                "clickhouse_admin",
            ),
            (
                "EXCHANGE TABLES fixture.items AND fixture.staging",
                "clickhouse_admin",
            ),
            ("TRUNCATE TABLE fixture.items", "truncate"),
            ("OPTIMIZE TABLE fixture.items FINAL", "clickhouse_admin"),
            ("SYSTEM RELOAD DICTIONARY fixture.dict", "clickhouse_admin"),
            ("SYSTEM SYNC REPLICA fixture.items", "clickhouse_admin"),
            (
                "KILL MUTATION WHERE database='fixture' AND table='items'",
                "clickhouse_admin",
            ),
            (
                "BACKUP TABLE fixture.items TO Disk('backups','snapshot.zip') ASYNC",
                "clickhouse_admin",
            ),
            (
                "RESTORE TABLE fixture.items FROM Disk('backups','snapshot.zip')",
                "clickhouse_admin",
            ),
            ("CREATE USER reader", "clickhouse_admin"),
            ("GRANT SELECT ON fixture.* TO reader", "clickhouse_admin"),
            ("REVOKE SELECT ON fixture.* FROM reader", "clickhouse_admin"),
            (
                "INSERT INTO fixture.items (id,label) VALUES (1,'a;\\\'b'),(2,'x')",
                "insert",
            ),
            (
                "INSERT INTO fixture.items SELECT * FROM fixture.staging",
                "insert",
            ),
            ("SET mutations_sync=1", "script_session"),
            ("USE fixture", "script_session"),
            (
                "SELECT toDateTime('2026-08-25','Asia/Shanghai') SETTINGS max_threads=2",
                "select",
            ),
        ] {
            assert_eq!(classify(sql, "fixture").map(|c| c.permission), Ok(key));
            assert_eq!(
                note::script_command_required(DatabaseKind::ClickHouse, sql),
                Some(key)
            );
        }
    }
    #[test]
    fn controls_and_mixed_actions_cannot_bypass_note_modes() {
        for sql in [
            "ALTER TABLE items ON CLUSTER cluster UPDATE v=1 WHERE id=1",
            "ALTER TABLE items UPDATE v=1 WHERE id=1, DROP COLUMN obsolete",
            "ALTER TABLE items UPDATE v=1 WHERE id=1, DELETE WHERE id=2",
        ] {
            assert!(!note::note_mode_allows(
                AccessMode::ReadWrite,
                DatabaseKind::ClickHouse,
                sql,
                true
            ));
            assert!(note::note_mode_allows(
                AccessMode::Admin,
                DatabaseKind::ClickHouse,
                sql,
                true
            ));
        }
        assert!(note::note_mode_allows(
            AccessMode::ReadWrite,
            DatabaseKind::ClickHouse,
            "ALTER TABLE items UPDATE v=1 WHERE id=1",
            true
        ));
        assert!(!note::note_mode_allows(
            AccessMode::ReadWrite,
            DatabaseKind::ClickHouse,
            "ALTER TABLE items DELETE WHERE id=1",
            true
        ));
        assert_eq!(
            note::script_requires_confirmation(DatabaseKind::ClickHouse, "SELECT 1", false),
            Ok(false)
        );
        assert_eq!(
            note::script_requires_confirmation(
                DatabaseKind::ClickHouse,
                "SET mutations_sync=1",
                false
            ),
            Ok(true)
        );
    }
    #[test]
    fn reject_ambiguous_payloads_multiple_statements_and_wrong_targets() {
        assert!(
            classify(
                "CREATE TABLE items(password String) ENGINE=MergeTree ORDER BY tuple()",
                "fixture"
            )
            .is_ok()
        );
        assert!(classify("CREATE USER reader IDENTIFIED BY ''", "fixture").is_err());

        for sql in [
            "SELECT 1; DROP TABLE items",
            "INSERT INTO items FORMAT CSV\n1,a;2,b",
            "INSERT INTO FUNCTION remote('host','db','table') SELECT 1",
            "SELECT 1 INTO OUTFILE 'local.csv'",
            "INSERT INTO items FROM INFILE 'local.csv'",
            "BACKUP ALL TO Disk('backups','x')",
            "ALTER TABLE other.items UPDATE v=1 WHERE id=1",
            "USE other",
        ] {
            assert!(classify(sql, "fixture").is_err());
        }
        assert!(
            classify(
                "ALTER TABLE `fixture`.`items` UPDATE label='DROP TABLE x; --' WHERE id=1",
                "fixture"
            )
            .is_ok()
        );
    }
    #[test]
    fn script_index_uses_clickhouse_not_mysql_and_retains_boundaries() {
        use turbodbn_core::notebook::SqlDialect;
        let mut sql = String::from("USE fixture;\n");
        for i in 0..67 {
            sql.push_str(&format!("ALTER TABLE fixture.items UPDATE label='a;\\\'b' WHERE id={i} AND stamp<addDays(toDateTime('2026-08-25','Asia/Shanghai'),1);\n"));
        }
        let prepared = crate::sql_script::prepare_text(
            &sql,
            SqlDialect::ClickHouse,
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(prepared.script.statements, 68);
        assert_eq!(prepared.script.unsupported, 0);
        assert_eq!(prepared.script.writes, 67);
        assert!(
            prepared
                .reader()
                .unwrap()
                .statement(67)
                .unwrap()
                .contains("id=66")
        );
    }
    #[test]
    fn paging_keeps_clickhouse_settings_and_never_replays_writes() {
        let sql = note::page_sql(
            DatabaseKind::ClickHouse,
            "SELECT id FROM items LIMIT 100 SETTINGS max_threads=2",
            20,
            20,
        )
        .unwrap();
        assert!(sql.find("LIMIT 21").unwrap() < sql.find("SETTINGS").unwrap());
        assert!(sql.contains("OFFSET 20"));
        assert!(note::page_sql(DatabaseKind::ClickHouse, "SELECT 1 FORMAT CSV", 0, 20).is_err());
        assert!(
            note::page_sql(
                DatabaseKind::ClickHouse,
                "ALTER TABLE items UPDATE id=1 WHERE id=2",
                0,
                20
            )
            .is_err()
        );
    }
    #[test]
    fn reanalyze_legacy_attachment_changes_only_derived_dialect_and_index() {
        use turbodbn_core::notebook::SqlDialect;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fixture.note");
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let old = crate::sql_script::prepare_text(
            "ALTER TABLE items UPDATE label='x' WHERE id=1;",
            SqlDialect::MySql,
            &cancel,
        )
        .unwrap();
        old.publish(&path, &cancel).unwrap();
        assert_eq!(old.script.unsupported, 1);
        let new = crate::sql_script::reparse(&path, &old.script, SqlDialect::ClickHouse, &cancel)
            .unwrap();
        assert_eq!(new.source_hash, old.script.source_hash);
        assert_eq!(new.name, old.script.name);
        assert_eq!(new.bytes, old.script.bytes);
        assert_eq!(new.unsupported, 0);
        assert_eq!(new.writes, 1);
    }
}

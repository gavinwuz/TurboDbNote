//! Copy-only migration drafts. Never opens a connection or executes SQL.
use super::super::{
    DatabaseKind,
    workbench::{self, Object, Schema},
};
use sqlparser::{
    ast::{ColumnDef, ColumnOption, CreateTable, Expr, Statement, UnaryOperator, Value},
    dialect::{ClickHouseDialect, Dialect, MySqlDialect, PostgreSqlDialect, SQLiteDialect},
    parser::Parser,
};
pub struct SyncDraft {
    pub sql: String,
    pub incomplete: bool,
    pub issues: Vec<SyncIssue>,
}
#[derive(Debug, PartialEq, Eq)]
pub struct SyncIssue {
    pub key: &'static str,
    pub column: String,
}
fn issue(issues: &mut Vec<SyncIssue>, key: &'static str, column: &str) {
    let value = SyncIssue {
        key,
        column: column.to_string(),
    };
    if !issues.contains(&value) {
        issues.push(value);
    }
}
fn parse(kind: DatabaseKind, ddl: &str) -> Option<CreateTable> {
    let dialect: Box<dyn Dialect> = match kind {
        DatabaseKind::MySql => Box::new(MySqlDialect {}),
        DatabaseKind::PostgreSql => Box::new(PostgreSqlDialect {}),
        DatabaseKind::Sqlite => Box::new(SQLiteDialect {}),
        DatabaseKind::ClickHouse => Box::new(ClickHouseDialect {}),
        _ => return None,
    };
    let mut statements = Parser::parse_sql(dialect.as_ref(), ddl).ok()?;
    if statements.len() != 1 {
        return None;
    }
    match statements.remove(0) {
        Statement::CreateTable(table) if table.query.is_none() && !table.columns.is_empty() => {
            Some(table)
        }
        _ => None,
    }
}
fn definition(kind: DatabaseKind, column: &ColumnDef) -> String {
    let value = column.to_string();
    format!(
        "{}{}",
        workbench::quote(kind, &column.name.value),
        &value[column.name.to_string().len()..]
    )
}
fn default(column: &ColumnDef) -> Option<String> {
    column.options.iter().find_map(|o| match &o.option {
        ColumnOption::Default(expr) => Some(expr.to_string()),
        _ => None,
    })
}
pub fn sync_ddl(
    kind: DatabaseKind,
    database: &str,
    object: &Object,
    current: &Schema,
    desired: &Schema,
) -> Result<SyncDraft, String> {
    if current.ddl.len() > 256_000
        || desired.ddl.len() > 256_000
        || current.columns.len() > 2000
        || desired.columns.len() > 2000
    {
        return Err("compare.ddl_limit".into());
    }
    let (left, right) = (
        parse(kind, &current.ddl).ok_or("compare.sync_parse")?,
        parse(kind, &desired.ddl).ok_or("compare.sync_parse")?,
    );
    let target = workbench::qualified(kind, database, object);
    let mut statements = vec![];
    let mut issues = vec![];
    // Other CREATE clauses, constraints and ordering are never silently translated.
    let mut comparable = right.clone();
    comparable.name = left.name.clone();
    comparable.columns = left.columns.clone();
    comparable.constraints = left.constraints.clone();
    if comparable != left {
        issue(&mut issues, "compare.issue.options", "");
    }
    if left.constraints != right.constraints {
        issue(&mut issues, "compare.issue.constraints", "");
    }
    let shared_left = left
        .columns
        .iter()
        .filter(|l| right.columns.iter().any(|r| r.name.value == l.name.value))
        .map(|c| &c.name.value)
        .collect::<Vec<_>>();
    let shared_right = right
        .columns
        .iter()
        .filter(|r| left.columns.iter().any(|l| l.name.value == r.name.value))
        .map(|c| &c.name.value)
        .collect::<Vec<_>>();
    if shared_left != shared_right {
        issue(&mut issues, "compare.issue.order", "");
    }
    let final_order = left
        .columns
        .iter()
        .filter(|l| right.columns.iter().any(|r| r.name.value == l.name.value))
        .chain(
            right
                .columns
                .iter()
                .filter(|r| !left.columns.iter().any(|l| l.name.value == r.name.value)),
        )
        .map(|c| &c.name.value)
        .collect::<Vec<_>>();
    if final_order
        != right
            .columns
            .iter()
            .map(|c| &c.name.value)
            .collect::<Vec<_>>()
    {
        issue(&mut issues, "compare.issue.order", "");
    }

    for column in &right.columns {
        let old = left
            .columns
            .iter()
            .find(|c| c.name.value == column.name.value);
        let q = workbench::quote(kind, &column.name.value);
        if old.is_some_and(|old| definition(kind, old) == definition(kind, column)) {
            continue;
        }
        // Do not emit transformations of generated/identity/reference columns without dependencies.
        let dependent = |c: &ColumnDef| {
            c.options.iter().any(|o| {
                matches!(
                    o.option,
                    ColumnOption::ForeignKey(_)
                        | ColumnOption::Generated { .. }
                        | ColumnOption::Identity(_)
                        | ColumnOption::Materialized(_)
                        | ColumnOption::Alias(_)
                )
            })
        };
        if dependent(column) || old.is_some_and(dependent) {
            issue(&mut issues, "compare.issue.column", &column.name.value);
            continue;
        }

        match old {
            None => {
                // SQLite cannot add key/unique/reference/generated columns portably.
                if kind == DatabaseKind::Sqlite
                    && !column.options.iter().all(|o| {
                        matches!(
                            o.option,
                            ColumnOption::Null | ColumnOption::NotNull | ColumnOption::Default(_)
                        )
                    })
                {
                    issue(&mut issues, "compare.issue.column", &column.name.value);
                    continue;
                }
                if kind == DatabaseKind::Sqlite {
                    let value = default(column);
                    let required = column
                        .options
                        .iter()
                        .any(|o| matches!(o.option, ColumnOption::NotNull));
                    let literal_default = column.options.iter().all(|o| match &o.option {
                        ColumnOption::Default(Expr::Value(v)) => matches!(v.value,
                            Value::Null | Value::Number(_, _) | Value::SingleQuotedString(_)
                            | Value::HexStringLiteral(_) | Value::Boolean(_)),
                        ColumnOption::Default(Expr::UnaryOp { op: UnaryOperator::Plus | UnaryOperator::Minus, expr }) =>
                            matches!(expr.as_ref(), Expr::Value(v) if matches!(v.value, Value::Number(_, _))),
                        ColumnOption::Default(_) => false,
                        _ => true,
                    });
                    if !literal_default
                        || (required
                            && value
                                .as_ref()
                                .is_none_or(|d| d.eq_ignore_ascii_case("NULL")))
                    {
                        issue(&mut issues, "compare.issue.column", &column.name.value);
                        continue;
                    }
                }
                statements.push(format!(
                    "ALTER TABLE {target} ADD COLUMN {};",
                    definition(kind, column)
                ));
            }
            Some(old) if definition(kind, old) != definition(kind, column) => match kind {
                DatabaseKind::MySql | DatabaseKind::ClickHouse => statements.push(format!(
                    "ALTER TABLE {target} MODIFY COLUMN {};",
                    definition(kind, column)
                )),
                DatabaseKind::PostgreSql => {
                    if old.data_type != column.data_type {
                        statements.push(format!(
                            "ALTER TABLE {target} ALTER COLUMN {q} TYPE {};",
                            column.data_type
                        ));
                    }
                    if default(old) != default(column) {
                        statements.push(format!(
                            "ALTER TABLE {target} ALTER COLUMN {q} {};",
                            default(column)
                                .map(|d| format!("SET DEFAULT {d}"))
                                .unwrap_or_else(|| "DROP DEFAULT".into())
                        ));
                    }
                    let not_null = |c: &ColumnDef| {
                        c.options
                            .iter()
                            .any(|o| matches!(o.option, ColumnOption::NotNull))
                    };
                    if not_null(old) != not_null(column) {
                        statements.push(format!(
                            "ALTER TABLE {target} ALTER COLUMN {q} {} NOT NULL;",
                            if not_null(column) { "SET" } else { "DROP" }
                        ));
                    }
                    let other = |c: &ColumnDef| {
                        c.options
                            .iter()
                            .filter(|o| {
                                !matches!(
                                    o.option,
                                    ColumnOption::Default(_)
                                        | ColumnOption::Null
                                        | ColumnOption::NotNull
                                )
                            })
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                    };
                    if other(old) != other(column) {
                        issue(&mut issues, "compare.issue.column", &column.name.value);
                    }
                }
                _ => issue(&mut issues, "compare.issue.column", &column.name.value),
            },
            _ => {}
        }
    }
    for column in &left.columns {
        if !right
            .columns
            .iter()
            .any(|c| c.name.value == column.name.value)
        {
            if kind == DatabaseKind::Sqlite {
                issue(&mut issues, "compare.issue.column", &column.name.value);
            } else {
                statements.push(format!(
                    "ALTER TABLE {target} DROP COLUMN {};",
                    workbench::quote(kind, &column.name.value)
                ));
            }
        }
    }
    // Describe may carry objects/comments outside the CREATE statement.
    for (changed, key) in [
        (
            current.triggers.values != desired.triggers.values,
            "compare.issue.triggers",
        ),
        (
            current.indexes.values != desired.indexes.values,
            "compare.issue.indexes",
        ),
        (
            current.foreign_keys.values != desired.foreign_keys.values,
            "compare.issue.foreign_keys",
        ),
    ] {
        if changed {
            issue(&mut issues, key, "");
        }
    }
    if kind != DatabaseKind::MySql {
        for column in &desired.columns {
            if current
                .columns
                .iter()
                .find(|c| c.name == column.name)
                .map(|c| c.comment.as_str())
                .unwrap_or("")
                != column.comment
            {
                issue(&mut issues, "compare.issue.comment", &column.name);
            }
        }
    }
    let incomplete = !issues.is_empty();
    let mut sql = String::from(
        "-- COPY ONLY: review before running in a Note.
-- Applies to the CURRENT (left) table; desired definitions come from the right.
-- DROP/MODIFY may lose data or fail on constraints. No automatic transaction or retry.
",
    );
    if incomplete {
        sql.push_str("-- INCOMPLETE: review indexes, constraints, table options, column order and unsupported changes manually.
");
    }
    for item in &issues {
        // Quote control characters to keep an identifier from breaking out of a SQL comment.
        sql.push_str(&format!(
            "-- MANUAL REVIEW: {} {:?}\n",
            match item.key {
                "compare.issue.options" => "Table options differ",
                "compare.issue.constraints" => "Constraints differ",
                "compare.issue.order" => "Column order differs",
                "compare.issue.indexes" => "Indexes differ",
                "compare.issue.foreign_keys" => "Foreign keys differ",
                "compare.issue.triggers" => "Triggers differ",
                "compare.issue.comment" => "Column comment differs",
                _ => "Column requires manual migration",
            },
            item.column
        ));
    }
    if statements.is_empty() {
        sql.push_str(
            "-- No supported column changes generated.
",
        );
    } else {
        sql.push_str(&statements.join(
            "
",
        ));
    }
    if sql.len() > 256_000 {
        return Err("compare.ddl_limit".into());
    }
    Ok(SyncDraft {
        sql,
        incomplete,
        issues,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn schema(ddl: &str) -> Schema {
        Schema {
            ddl: ddl.into(),
            ..Default::default()
        }
    }
    fn object() -> Object {
        Object {
            name: "current_table".into(),
            schema: String::new(),
            kind: "table".into(),
            comment: String::new(),
            parent: String::new(),
        }
    }
    #[test]
    fn mysql_draft_targets_current_table_and_retains_full_definition() {
        let a = schema("CREATE TABLE current_table (id INT, obsolete TEXT)");
        let b = schema(
            "CREATE TABLE desired_table (id BIGINT NOT NULL DEFAULT 7, label VARCHAR(40) DEFAULT 'sample' COMMENT 'label')",
        );
        let plan = sync_ddl(DatabaseKind::MySql, "left_db", &object(), &a, &b).unwrap();
        assert!(plan.sql.contains(
            "ALTER TABLE `left_db`.`current_table` MODIFY COLUMN `id` BIGINT NOT NULL DEFAULT 7;"
        ));
        assert!(
            plan.sql
                .contains("ADD COLUMN `label` VARCHAR(40) DEFAULT 'sample' COMMENT 'label'")
        );
        assert!(plan.sql.contains("DROP COLUMN `obsolete`"));
        assert!(!plan.sql.contains("ALTER TABLE desired_table"));
        assert!(!plan.incomplete);
    }
    #[test]
    fn postgres_changes_type_default_and_null_without_replacing_table() {
        let a = schema("CREATE TABLE current_table (id INTEGER DEFAULT 1)");
        let b = schema("CREATE TABLE desired_table (id BIGINT NOT NULL DEFAULT 2)");
        let plan = sync_ddl(DatabaseKind::PostgreSql, "left_db", &object(), &a, &b).unwrap();
        assert!(plan.sql.contains("TYPE BIGINT"));
        assert!(plan.sql.contains("SET DEFAULT 2"));
        assert!(plan.sql.contains("SET NOT NULL"));
        assert!(!plan.sql.contains("DROP TABLE"));
    }
    #[test]
    fn sqlite_unsupported_changes_and_constraint_changes_are_not_silently_complete() {
        let a = schema("CREATE TABLE current_table (id INTEGER, name TEXT)");
        let b = schema("CREATE TABLE desired_table (id TEXT PRIMARY KEY)");
        let plan = sync_ddl(DatabaseKind::Sqlite, "main", &object(), &a, &b).unwrap();
        assert!(plan.incomplete);
        assert!(!plan.sql.contains("ALTER TABLE"));
        let plan = sync_ddl(
            DatabaseKind::MySql,
            "db",
            &object(),
            &schema("CREATE TABLE current_table (id INT)"),
            &schema("CREATE TABLE desired_table (id INT, PRIMARY KEY(id))"),
        )
        .unwrap();
        assert!(plan.incomplete);
    }
    #[test]
    fn unchanged_generated_columns_are_not_reported_as_missing_work() {
        let value = schema(
            "CREATE TABLE current_table (id INT, calculated INT GENERATED ALWAYS AS (id + 1) STORED)",
        );
        let plan = sync_ddl(DatabaseKind::MySql, "db", &object(), &value, &value).unwrap();
        assert!(!plan.incomplete);
        assert!(!plan.sql.contains("ALTER TABLE"));
    }
    #[test]
    fn sqlite_rejects_additions_that_require_a_rebuild() {
        for column in [
            "name TEXT NOT NULL",
            "created TEXT DEFAULT CURRENT_TIMESTAMP",
            "key INTEGER PRIMARY KEY",
            "label TEXT DEFAULT ('a' || 'b')",
        ] {
            let plan = sync_ddl(
                DatabaseKind::Sqlite,
                "main",
                &object(),
                &schema("CREATE TABLE current_table (id INTEGER)"),
                &schema(&format!("CREATE TABLE desired (id INTEGER, {column})")),
            )
            .unwrap();
            assert!(plan.incomplete, "{column}");
            assert!(plan.issues.iter().any(|i| i.key == "compare.issue.column"));
            assert!(!plan.sql.contains("ALTER TABLE"));
        }
    }
    #[test]
    fn default_text_is_not_mistaken_for_a_generated_option() {
        let plan = sync_ddl(
            DatabaseKind::MySql,
            "db",
            &object(),
            &schema("CREATE TABLE current_table (id INT)"),
            &schema(
                "CREATE TABLE desired (id INT, label TEXT DEFAULT 'IDENTITY REFERENCES GENERATED')",
            ),
        )
        .unwrap();
        assert!(!plan.incomplete);
        assert!(plan.sql.contains("ADD COLUMN"));
    }
    #[test]
    fn partial_drafts_list_each_unhandled_difference() {
        let plan = sync_ddl(
            DatabaseKind::MySql,
            "db",
            &object(),
            &schema("CREATE TABLE current_table (id INT, label TEXT)"),
            &schema("CREATE TABLE desired (label TEXT, id INT, PRIMARY KEY(id))"),
        )
        .unwrap();
        assert!(
            plan.issues
                .iter()
                .any(|i| i.key == "compare.issue.constraints")
        );
        assert!(plan.issues.iter().any(|i| i.key == "compare.issue.order"));
        assert!(plan.sql.contains("Constraints differ"));
        assert!(plan.sql.contains("Column order differs"));
    }
    #[test]
    fn changed_generated_columns_do_not_emit_unsafe_modify() {
        let plan = sync_ddl(
            DatabaseKind::MySql,
            "db",
            &object(),
            &schema(
                "CREATE TABLE current_table (id INT, x INT GENERATED ALWAYS AS (id + 1) STORED)",
            ),
            &schema("CREATE TABLE desired (id INT, x INT GENERATED ALWAYS AS (id + 2) STORED)"),
        )
        .unwrap();
        assert!(plan.incomplete);
        assert_eq!(plan.issues[0].column, "x");
        assert!(!plan.sql.contains("MODIFY COLUMN"));
    }
    #[test]
    fn rejects_non_table_and_multiple_statements() {
        for ddl in [
            "CREATE VIEW desired AS SELECT 1",
            "CREATE TABLE x(id INT); DROP TABLE x",
        ] {
            assert!(
                sync_ddl(
                    DatabaseKind::MySql,
                    "db",
                    &object(),
                    &schema("CREATE TABLE current_table (id INT)"),
                    &schema(ddl)
                )
                .is_err()
            );
        }
    }
}

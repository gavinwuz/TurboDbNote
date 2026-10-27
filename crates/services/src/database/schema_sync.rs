//! Copy-only migration drafts. Never opens a connection or executes SQL.
use super::super::{
    DatabaseKind,
    workbench::{self, Object, Schema},
};
#[path = "mysql_ddl_parts.rs"]
pub(super) mod mysql_parts;
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
    if kind == DatabaseKind::MySql && (ddl.contains("/*!") || ddl.contains("/*M!")) {
        return None;
    }
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
    let (left, right) = match (parse(kind, &current.ddl), parse(kind, &desired.ddl)) {
        (Some(left), Some(right)) => (left, right),
        _ if kind == DatabaseKind::MySql => {
            return mysql_recover(database, object, current, desired);
        }
        _ => return Err("compare.sync_parse".into()),
    };
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
    if kind == DatabaseKind::MySql {
        let (index_sql, unresolved) =
            super::super::index_design::sync_mysql(database, object, current, desired);
        statements.extend(index_sql);
        if unresolved {
            issue(&mut issues, "compare.issue.indexes", "");
        } else {
            issues.retain(|item| item.key != "compare.issue.indexes");
            let constraints = |table: &CreateTable, schema: &Schema| {
                table
                    .constraints
                    .iter()
                    .map(ToString::to_string)
                    .filter(|definition| !represented_index(schema, definition))
                    .collect::<Vec<_>>()
            };
            if constraints(&left, current) == constraints(&right, desired) {
                issues.retain(|item| item.key != "compare.issue.constraints");
            }
        }
    }
    let incomplete = !issues.is_empty();
    let mut sql = String::from(
        "-- COPY ONLY: review before running in a Note.
-- Applies to the explicitly named target; desired definitions come from the reference snapshot.
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

pub(super) fn mysql_comments(current: Option<&Schema>, desired: &Schema) -> Option<Schema> {
    let target = mysql_parts::split(&desired.ddl)?;
    let source = current.and_then(|schema| mysql_parts::split(&schema.ddl));
    let rewrite = |value: &str, previous: Option<&str>| -> Option<String> {
        let mut value = value.to_owned();
        if let Some((start, end)) = workbench::comment_clause(&value).ok()? {
            value.replace_range(start..end, "");
        }
        if let Some(previous) = previous
            && let Some((start, end)) = workbench::comment_clause(previous).ok()?
        {
            value.push(' ');
            value.push_str(&previous[start..end]);
        }
        Some(value)
    };
    let mut columns = Vec::new();
    for (name, value) in &target.columns {
        let previous = source
            .as_ref()
            .and_then(|s| s.columns.get(name))
            .map(String::as_str);
        // New columns have no old comment to preserve.
        columns.push(if current.is_some() && previous.is_none() {
            value.clone()
        } else {
            rewrite(value, previous)?
        });
    }
    columns.extend(target.constraints);
    let options = rewrite(&target.options, source.as_ref().map(|s| s.options.as_str()))?;
    let mut output = desired.clone();
    output.ddl = format!("CREATE TABLE _comments ({}) {}", columns.join(","), options);
    for column in &mut output.columns {
        column.comment = current
            .and_then(|s| s.columns.iter().find(|c| c.name == column.name))
            .map(|c| c.comment.clone())
            .unwrap_or_default();
    }
    if let Some(index) = output
        .indexes
        .columns
        .iter()
        .position(|name| name.eq_ignore_ascii_case("Comment"))
    {
        for row in &mut output.indexes.values {
            if let Some(value) = row.get_mut(index) {
                *value = Some(String::new());
            }
        }
    }
    Some(output)
}

fn mysql_recover(
    database: &str,
    object: &Object,
    current: &Schema,
    desired: &Schema,
) -> Result<SyncDraft, String> {
    let (a, b) = (
        mysql_parts::split(&current.ddl).ok_or("compare.sync_parse")?,
        mysql_parts::split(&desired.ddl).ok_or("compare.sync_parse")?,
    );
    let kind = DatabaseKind::MySql;
    let mut blocked = std::collections::BTreeSet::new();
    for (name, definition) in a.columns.iter().chain(&b.columns) {
        if parse(kind, &format!("CREATE TABLE _recovered ({definition})")).is_none() {
            blocked.insert(name.clone());
        }
    }
    let sanitized = |original: &Schema, columns: &std::collections::BTreeMap<String, String>| {
        let mut schema = original.clone();
        let mut definitions = original
            .columns
            .iter()
            .filter_map(|c| columns.get(&c.name).filter(|_| !blocked.contains(&c.name)))
            .cloned()
            .collect::<Vec<_>>();
        for (name, definition) in columns {
            if !blocked.contains(name) && !original.columns.iter().any(|c| &c.name == name) {
                definitions.push(definition.clone());
            }
        }
        if definitions.is_empty() {
            definitions.push("_recovery_placeholder INT".into());
        }
        schema.ddl = format!("CREATE TABLE _recovered ({})", definitions.join(","));
        schema
    };
    let mut plan = sync_ddl(
        kind,
        database,
        object,
        &sanitized(current, &a.columns),
        &sanitized(desired, &b.columns),
    )?;
    for name in blocked {
        issue(&mut plan.issues, "compare.issue.parse_column", &name);
    }
    let non_index = |parts: &mysql_parts::Parts, schema: &Schema| {
        parts
            .constraints
            .iter()
            .filter(|definition| !represented_index(schema, definition))
            .cloned()
            .collect::<Vec<_>>()
    };
    if non_index(&a, current) != non_index(&b, desired) {
        issue(&mut plan.issues, "compare.issue.constraints", "");
    }
    if mysql_parts::options_key(&a.options) != mysql_parts::options_key(&b.options) {
        issue(&mut plan.issues, "compare.issue.options", "");
    }
    plan.incomplete = !plan.issues.is_empty();
    if plan.incomplete {
        for item in &plan.issues {
            plan.sql.push_str(&format!(
                "\n-- MANUAL REVIEW: {} {:?}",
                item.key, item.column
            ));
        }
    }
    plan.sql.insert_str(0, "-- Recovered individual columns and indexes from metadata; unsupported components are listed below.\n");
    Ok(plan)
}

type ColumnAttributes =
    std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>;
pub(super) fn column_attributes(kind: DatabaseKind, schema: &Schema) -> Option<ColumnAttributes> {
    let table = match parse(kind, &schema.ddl) {
        Some(table) => table,
        None if kind == DatabaseKind::MySql => {
            let parts = mysql_parts::split(&schema.ddl)?;
            let columns = parts
                .columns
                .values()
                .filter(|definition| {
                    parse(kind, &format!("CREATE TABLE _detail ({definition})")).is_some()
                })
                .cloned()
                .collect::<Vec<_>>();
            if columns.len() != parts.columns.len() {
                return None;
            }
            parse(
                kind,
                &format!("CREATE TABLE _detail ({})", columns.join(",")),
            )?
        }
        None => return None,
    };
    Some(
        table
            .columns
            .into_iter()
            .map(|column| {
                let mut attributes = std::collections::BTreeMap::new();
                attributes.insert("compare.column_definition".into(), column.to_string());
                attributes.insert("grid.structure.type".into(), column.data_type.to_string());
                attributes.insert(
                    "ddl.column.not_null".into(),
                    if column
                        .options
                        .iter()
                        .any(|o| matches!(o.option, ColumnOption::NotNull))
                    {
                        "✓"
                    } else {
                        "—"
                    }
                    .into(),
                );
                for option in &column.options {
                    let (key, value) = match &option.option {
                        ColumnOption::NotNull | ColumnOption::Null => continue,
                        ColumnOption::Default(value) => {
                            ("compare.default_value", value.to_string())
                        }
                        ColumnOption::Comment(value) => ("ddl.column.comment", value.clone()),
                        _ => ("ddl.column.extra", option.to_string()),
                    };
                    attributes
                        .entry(key.into())
                        .and_modify(|v| {
                            v.push_str("; ");
                            v.push_str(&value);
                        })
                        .or_insert(value);
                }
                (column.name.value, attributes)
            })
            .collect(),
    )
}

fn represented_index(schema: &Schema, definition: &str) -> bool {
    let Some(name) = mysql_parts::index_name(definition) else {
        return false;
    };
    if !schema.index_columns.contains_key(&name) {
        return false;
    }
    schema
        .indexes
        .values
        .iter()
        .any(|row| row.first().and_then(Option::as_deref) == Some(name.as_str()))
}

/// Canonical structural pieces, excluding names and mutable row-allocation counters.
type ComparisonParts = (Vec<(String, String)>, Vec<String>, String);
pub(super) fn comparison_parts(kind: DatabaseKind, schema: &Schema) -> Option<ComparisonParts> {
    use sqlparser::ast::{CreateTableOptions, SqlOption};
    let mut table = match parse(kind, &schema.ddl) {
        Some(table) => table,
        None if kind == DatabaseKind::MySql => {
            let parts = mysql_parts::split(&schema.ddl)?;
            return Some((
                parts.columns.into_iter().collect(),
                parts
                    .constraints
                    .into_iter()
                    .filter(|definition| !represented_index(schema, definition))
                    .collect(),
                mysql_parts::options_key(&parts.options),
            ));
        }
        None => return None,
    };
    let columns = table
        .columns
        .iter()
        .map(|column| (column.name.value.clone(), column.to_string()))
        .collect();
    let mut constraints = table
        .constraints
        .iter()
        .map(ToString::to_string)
        .filter(|definition| kind != DatabaseKind::MySql || !represented_index(schema, definition))
        .collect::<Vec<_>>();
    constraints.sort();
    table.columns.clear();
    table.constraints.clear();
    table.name = parse(kind, "CREATE TABLE _comparison (id INT)")?.name;
    if let CreateTableOptions::Plain(options) = &mut table.table_options {
        options.retain(|option|!matches!(option,SqlOption::KeyValue{key,..} if key.value.eq_ignore_ascii_case("AUTO_INCREMENT")));
    }
    Some((columns, constraints, table.to_string()))
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
    #[test]
    fn unknown_table_option_does_not_block_known_column_changes() {
        let a = schema(
            "CREATE TABLE old(id INT DEFAULT 0 COMMENT 'old') ENGINE=InnoDB FUTURE_OPTION='x'",
        );
        let b = schema(
            "CREATE TABLE desired(id BIGINT DEFAULT 1 COMMENT 'new', label VARCHAR(30)) ENGINE=InnoDB FUTURE_OPTION='x'",
        );
        assert!(parse(DatabaseKind::MySql, &a.ddl).is_none());
        let plan = sync_ddl(DatabaseKind::MySql, "target", &object(), &a, &b).unwrap();
        assert!(
            plan.sql
                .contains("MODIFY COLUMN `id` BIGINT DEFAULT 1 COMMENT 'new'"),
            "{}",
            plan.sql
        );
        assert!(plan.sql.contains("ADD COLUMN `label` VARCHAR(30)"));
        assert!(!plan.sql.contains("DROP TABLE"));
    }
    #[test]
    fn recovery_skips_unknown_columns_without_dropping_or_modifying_them() {
        let a = schema(
            "CREATE TABLE old(id INT, opaque FUTURE_TYPE FUTURE_COLUMN_OPTION) FUTURE_OPTION='x'",
        );
        let b = schema(
            "CREATE TABLE desired(id BIGINT, opaque FUTURE_TYPE FUTURE_COLUMN_OPTION) FUTURE_OPTION='x'",
        );
        let plan = sync_ddl(DatabaseKind::MySql, "target", &object(), &a, &b).unwrap();
        assert!(plan.sql.contains("MODIFY COLUMN `id` BIGINT"));
        assert!(!plan.sql.contains("DROP COLUMN `opaque`"));
        assert!(!plan.sql.contains("MODIFY COLUMN `opaque`"));
        assert!(plan.incomplete);
    }
    #[test]
    fn recovered_comment_preservation_keeps_target_comment_and_default_literals() {
        let a = schema(
            "CREATE TABLE old(id INT COMMENT 'keep') COMMENT='target table' FUTURE_OPTION='x'",
        );
        let b = schema(
            "CREATE TABLE desired(id BIGINT DEFAULT 'COMMENT text' COMMENT 'replace') COMMENT='source table' FUTURE_OPTION='x'",
        );
        let plan = super::super::sync_preserving_comments(
            DatabaseKind::MySql,
            "target",
            &object(),
            &a,
            &b,
        )
        .unwrap();
        assert!(
            plan.sql.contains("DEFAULT 'COMMENT text' COMMENT 'keep'"),
            "{}",
            plan.sql
        );
        assert!(!plan.sql.contains("COMMENT 'replace'"));
    }
    #[test]
    fn mysql_versioned_fulltext_clause_does_not_block_unrelated_column_alignment() {
        let a = schema(
            "CREATE TABLE old(id INT, body TEXT, FULLTEXT KEY idx_body(body) /*!50100 WITH PARSER `ngram` */) ENGINE=InnoDB",
        );
        let b = schema(
            "CREATE TABLE desired(id BIGINT, body TEXT, FULLTEXT KEY idx_body(body) /*!50100 WITH PARSER `ngram` */) ENGINE=InnoDB",
        );
        let plan = sync_ddl(DatabaseKind::MySql, "target", &object(), &a, &b).unwrap();
        assert!(
            plan.sql.contains("MODIFY COLUMN `id` BIGINT"),
            "{}",
            plan.sql
        );
        assert!(!plan.sql.contains("DROP INDEX"));
        assert!(!plan.sql.contains("ADD FULLTEXT"));
    }
}

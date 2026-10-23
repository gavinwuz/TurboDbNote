//! Validation for editable DDL previews. Keep SQL source bytes intact after parsing.
use super::{
    DatabaseKind,
    workbench::{self, Object},
};
use sqlparser::{
    ast::{AlterTableOperation as Op, Statement},
    dialect::{
        ClickHouseDialect, Dialect, GenericDialect, MySqlDialect, PostgreSqlDialect, SQLiteDialect,
    },
    parser::Parser,
};
fn dialect(kind: DatabaseKind) -> Box<dyn Dialect> {
    match kind {
        DatabaseKind::MySql => Box::new(MySqlDialect {}),
        DatabaseKind::PostgreSql => Box::new(PostgreSqlDialect {}),
        DatabaseKind::Sqlite => Box::new(SQLiteDialect {}),
        DatabaseKind::ClickHouse => Box::new(ClickHouseDialect {}),
        _ => Box::new(GenericDialect {}),
    }
}
pub fn valid_definition(kind: DatabaseKind, text: &str) -> bool {
    if text.is_empty() || text.len() > 16000 || text.contains('\0') {
        return false;
    }
    matches!(Parser::parse_sql(dialect(kind).as_ref(),&format!("CREATE TABLE _design (_column {text})")).as_deref(),Ok([Statement::CreateTable(table)]) if table.columns.len()==1 && table.constraints.is_empty() && table.query.is_none())
}
pub fn valid_target(kind: DatabaseKind, database: &str, object: &Object, sql: &str) -> bool {
    if sql.len() > 60000 || sql.contains('\0') {
        return false;
    }
    let dialect = dialect(kind);
    let Ok(expected) = Parser::parse_sql(
        dialect.as_ref(),
        &format!(
            "ALTER TABLE {} DROP COLUMN _validation",
            workbench::qualified(kind, database, object)
        ),
    ) else {
        return false;
    };
    let Statement::AlterTable(expected) = &expected[0] else {
        return false;
    };
    let Ok(statements) = Parser::parse_sql(dialect.as_ref(), sql) else {
        return false;
    };
    !statements.is_empty()
        && statements.len() <= 8
        && statements.iter().all(|statement| {
            let Statement::AlterTable(table) = statement else {
                return false;
            };
            table.name == expected.name
                && table.on_cluster.is_none()
                && table.location.is_none()
                && !table.operations.is_empty()
                && table.operations.iter().all(|op| {
                    matches!(
                        op,
                        Op::AddColumn { .. }
                            | Op::ChangeColumn { .. }
                            | Op::ModifyColumn { .. }
                            | Op::RenameColumn { .. }
                            | Op::DropColumn { .. }
                            | Op::AlterColumn { .. }
                            | Op::AddConstraint { .. }
                            | Op::DropConstraint { .. }
                            | Op::DropPrimaryKey { .. }
                    )
                })
        })
}
/// Toggle one MySQL primary-key member while preserving the existing composite order.
pub fn toggle_primary(
    database: &str,
    object: &Object,
    schema: &workbench::Schema,
    column: &str,
) -> Result<String, String> {
    let selected = schema
        .columns
        .iter()
        .find(|c| c.name == column)
        .ok_or("designer.invalid")?;
    let mut primary = schema
        .indexes
        .values
        .iter()
        .filter(|r| r.first().and_then(|v| v.as_deref()) == Some("PRIMARY"))
        .filter_map(|r| r.get(1).and_then(Clone::clone))
        .collect::<Vec<_>>();
    if schema.columns.iter().any(|c| c.primary) && primary.is_empty() {
        return Err("dbwork.context.changed".into());
    }
    let had_primary = !primary.is_empty();
    if selected.primary {
        primary.retain(|c| c != column);
    } else {
        primary.push(column.into());
    }
    let mut clauses = vec![];
    if had_primary {
        clauses.push("DROP PRIMARY KEY".to_string());
    }
    if !primary.is_empty() {
        clauses.push(format!(
            "ADD PRIMARY KEY ({})",
            primary
                .iter()
                .map(|n| workbench::quote(DatabaseKind::MySql, n))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Ok(format!(
        "ALTER TABLE {} {};",
        workbench::qualified(DatabaseKind::MySql, database, object),
        clauses.join(", ")
    ))
}
/// Editable MySQL column attributes. Extra SQL retains defaults, charset and collation verbatim.
pub struct ColumnDraft {
    pub name: String,
    pub data_type: String,
    pub extra: String,
    pub comment: String,
    pub not_null: bool,
    pub auto_increment: bool,
    pub primary: bool,
}
pub fn mysql_extra(old: &workbench::Column, ddl: &str) -> Result<String, String> {
    let definition = mysql_definition(old, ddl)?;
    let mut tail = definition
        .get(old.data_type.len()..)
        .ok_or("dbwork.context.changed")?
        .to_string();
    if let Some((a, b)) = workbench::comment_clause(&tail)? {
        tail.replace_range(a..b, "");
    }
    let tokens = attribute_tokens(&tail);
    let mut remove = vec![];
    for (i, (word, a, b)) in tokens.iter().enumerate() {
        if word == "AUTO_INCREMENT"
            || word == "NOT" && tokens.get(i + 1).is_some_and(|t| t.0 == "NULL")
            || word == "NULL" && (i == 0 || tokens[i - 1].0 != "DEFAULT")
        {
            remove.push((*a, *b));
        }
    }
    for (a, b) in remove.into_iter().rev() {
        tail.replace_range(a..b, "");
    }
    Ok(tail.trim().to_owned())
}
fn mysql_definition<'a>(old: &workbench::Column, ddl: &'a str) -> Result<&'a str, String> {
    let prefix = format!("{} ", workbench::quote(DatabaseKind::MySql, &old.name));
    let definition = ddl
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix(&prefix))
        .ok_or("dbwork.context.changed")?
        .trim_end_matches(',');
    if !definition
        .get(..old.data_type.len())
        .is_some_and(|p| p.eq_ignore_ascii_case(&old.data_type))
    {
        return Err("dbwork.context.changed".into());
    }
    Ok(definition)
}
fn attribute_tokens(sql: &str) -> Vec<(String, usize, usize)> {
    let b = sql.as_bytes();
    let (mut i, mut depth) = (0, 0usize);
    let mut tokens = vec![];
    while i < b.len() {
        let start = i;
        if matches!(b[i], b'\'' | b'"' | b'`') {
            let quote = b[i];
            i += 1;
            while i < b.len() {
                if b[i] == b'\\' {
                    i = (i + 2).min(b.len());
                } else if b[i] == quote {
                    if b.get(i + 1) == Some(&quote) {
                        i += 2;
                    } else {
                        i += 1;
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            if depth == 0 {
                tokens.push(("VALUE".into(), start, i));
            }
            continue;
        }
        if b[i] == b'(' {
            depth += 1;
            i += 1;
            continue;
        }
        if b[i] == b')' {
            depth = depth.saturating_sub(1);
            i += 1;
            if depth == 0 {
                tokens.push(("VALUE".into(), start, i));
            }
            continue;
        }
        if b[i..].starts_with(b"/*") {
            i += 2;
            while i + 1 < b.len() && !b[i..].starts_with(b"*/") {
                i += 1;
            }
            i = (i + 2).min(b.len());
            continue;
        }
        if depth == 0 && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
            i += 1;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            tokens.push((sql[start..i].to_ascii_uppercase(), start, i));
        } else {
            i += 1;
        }
    }
    tokens
}
pub fn mysql_edit(
    database: &str,
    object: &Object,
    schema: &workbench::Schema,
    old: Option<&workbench::Column>,
    draft: &ColumnDraft,
) -> Result<String, String> {
    if draft.name.trim().is_empty()
        || draft.name.len() > 256
        || draft.name.chars().any(char::is_control)
    {
        return Err("designer.invalid".into());
    }
    if attribute_tokens(&draft.extra).iter().any(|(word, _, _)| {
        matches!(
            word.as_str(),
            "AUTO_INCREMENT" | "COMMENT" | "PRIMARY" | "NOT"
        )
    }) {
        return Err("designer.extra_invalid".into());
    }
    let comment = if let Some(old) = old.filter(|c| c.comment == draft.comment) {
        let tail = mysql_definition(old, &schema.ddl)?
            .get(old.data_type.len()..)
            .ok_or("dbwork.context.changed")?;
        workbench::comment_clause(tail)?
            .map(|(a, b)| tail[a..b].to_string())
            .unwrap_or_default()
    } else {
        if draft.comment.contains(['\\', '\0']) {
            return Err("designer.comment_escape".into());
        }
        format!("COMMENT {}", workbench::literal(&draft.comment))
    };
    let definition = format!(
        "{} {} {} {} {}",
        draft.data_type.trim(),
        draft.extra.trim(),
        if draft.not_null { "NOT NULL" } else { "NULL" },
        if draft.auto_increment {
            "AUTO_INCREMENT"
        } else {
            ""
        },
        comment
    );
    if !valid_definition(DatabaseKind::MySql, &definition) {
        return Err("designer.invalid".into());
    }
    let q = |name: &str| workbench::quote(DatabaseKind::MySql, name);
    let mut clauses = vec![if let Some(old) = old {
        format!(
            "CHANGE COLUMN {} {} {definition}",
            q(&old.name),
            q(draft.name.trim())
        )
    } else {
        format!("ADD COLUMN {} {definition}", q(draft.name.trim()))
    }];
    if old.is_some_and(|c| c.primary) != draft.primary {
        let mut primary = schema
            .indexes
            .values
            .iter()
            .filter(|r| r.first().and_then(|v| v.as_deref()) == Some("PRIMARY"))
            .filter_map(|r| r.get(1).and_then(Clone::clone))
            .collect::<Vec<_>>();
        if !primary.is_empty() {
            clauses.push("DROP PRIMARY KEY".into());
        }
        if let Some(old) = old {
            primary.retain(|name| name != &old.name);
        }
        if draft.primary {
            primary.push(draft.name.trim().to_owned());
        }
        if !primary.is_empty() {
            clauses.push(format!(
                "ADD PRIMARY KEY ({})",
                primary
                    .iter()
                    .map(|name| q(name))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    let sql = format!(
        "ALTER TABLE {} {};",
        workbench::qualified(DatabaseKind::MySql, database, object),
        clauses.join(", ")
    );
    if !valid_target(DatabaseKind::MySql, database, object, &sql) {
        return Err("designer.invalid".into());
    }
    Ok(sql)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mysql_form_retains_defaults_charset_and_quoted_keywords() {
        let column = workbench::Column {
            name: "name".into(),
            data_type: "varchar(32)".into(),
            not_null: Some(true),
            comment: "not null".into(),
            ..Default::default()
        };
        let ddl = "CREATE TABLE `t` (\n  `name` varchar(32) CHARACTER SET latin1 NOT NULL DEFAULT 'NULL NOT AUTO_INCREMENT' COMMENT 'not null'\n)";
        let extra = mysql_extra(&column, ddl).unwrap();
        assert!(extra.contains("DEFAULT 'NULL NOT AUTO_INCREMENT'"));
        assert!(extra.contains("CHARACTER SET latin1"));
        let schema = workbench::Schema {
            columns: vec![column.clone()],
            ddl: ddl.into(),
            ..Default::default()
        };
        let object = Object {
            schema: String::new(),
            name: "t".into(),
            kind: "table".into(),
            comment: String::new(),
            parent: String::new(),
        };
        let draft = ColumnDraft {
            name: "renamed".into(),
            data_type: "varchar(32)".into(),
            extra,
            comment: "not null".into(),
            not_null: true,
            auto_increment: false,
            primary: false,
        };
        let sql = mysql_edit("db", &object, &schema, Some(&column), &draft).unwrap();
        assert!(sql.contains("COMMENT 'not null'"));
        assert!(sql.contains("CHARACTER SET latin1"));
        assert!(sql.contains("DEFAULT 'NULL NOT AUTO_INCREMENT'"));
        let nullable = workbench::Column {
            name: "n".into(),
            data_type: "int".into(),
            ..Default::default()
        };
        assert_eq!(
            mysql_extra(&nullable, "CREATE TABLE t (\n `n` int DEFAULT NULL\n)").unwrap(),
            "DEFAULT NULL"
        );
    }
    #[test]
    fn composite_primary_order_is_retained() {
        let object = Object {
            schema: String::new(),
            name: "t".into(),
            kind: "table".into(),
            comment: String::new(),
            parent: String::new(),
        };
        let schema = workbench::Schema {
            columns: vec![
                workbench::Column {
                    name: "a".into(),
                    primary: true,
                    ..Default::default()
                },
                workbench::Column {
                    name: "b".into(),
                    primary: true,
                    ..Default::default()
                },
                workbench::Column {
                    name: "c".into(),
                    ..Default::default()
                },
            ],
            indexes: workbench::Rows {
                columns: vec![],
                values: vec![
                    vec![Some("PRIMARY".into()), Some("b".into())],
                    vec![Some("PRIMARY".into()), Some("a".into())],
                ],
            },
            ..Default::default()
        };
        let sql = toggle_primary("db", &object, &schema, "c").unwrap();
        assert!(sql.contains("(`b`, `a`, `c`)"));
        assert!(valid_target(DatabaseKind::MySql, "db", &object, &sql));
        assert!(!valid_target(
            DatabaseKind::MySql,
            "db",
            &object,
            "ALTER TABLE `db`.`t` RENAME TO `other`;"
        ));
    }
    #[test]
    fn keeps_raw_defaults_and_rejects_statement_escape() {
        assert!(valid_definition(
            DatabaseKind::MySql,
            "varchar(32) CHARACTER SET latin1 NOT NULL DEFAULT 'a\\\\b' COMMENT 'text'"
        ));
        assert!(!valid_definition(DatabaseKind::MySql, "int, injected int"));
        assert!(!valid_definition(
            DatabaseKind::MySql,
            "int); DROP TABLE other;--"
        ));
        let object = Object {
            schema: String::new(),
            name: "account".into(),
            kind: "table".into(),
            comment: String::new(),
            parent: String::new(),
        };
        assert!(valid_target(
            DatabaseKind::MySql,
            "test",
            &object,
            "ALTER TABLE `test`.`account` DROP COLUMN `obsolete`;"
        ));
        assert!(!valid_target(
            DatabaseKind::MySql,
            "test",
            &object,
            "ALTER TABLE `test`.`other` DROP COLUMN `obsolete`;"
        ));
        assert!(!valid_target(
            DatabaseKind::MySql,
            "test",
            &object,
            "DELETE FROM `test`.`account`;"
        ));
    }
}

/// Common templates only; custom server types remain editable.
pub fn default_type(kind: DatabaseKind) -> &'static str {
    match kind {
        DatabaseKind::MySql | DatabaseKind::PostgreSql => "varchar(255)",
        DatabaseKind::Sqlite => "TEXT",
        DatabaseKind::ClickHouse => "String",
        _ => "",
    }
}
pub fn type_presets(kind: DatabaseKind) -> &'static [&'static str] {
    match kind {
        DatabaseKind::MySql => &[
            "int",
            "bigint",
            "tinyint",
            "smallint",
            "mediumint",
            "decimal(10,2)",
            "float",
            "double",
            "varchar(255)",
            "char(32)",
            "text",
            "mediumtext",
            "longtext",
            "date",
            "datetime",
            "timestamp",
            "time",
            "year",
            "boolean",
            "json",
            "blob",
            "varbinary(255)",
        ],
        DatabaseKind::PostgreSql => &[
            "integer",
            "bigint",
            "smallint",
            "numeric(10,2)",
            "real",
            "double precision",
            "varchar(255)",
            "text",
            "boolean",
            "uuid",
            "json",
            "jsonb",
            "date",
            "timestamp",
            "timestamp with time zone",
            "time",
            "bytea",
            "integer[]",
        ],
        DatabaseKind::Sqlite => &["INTEGER", "TEXT", "REAL", "NUMERIC", "BLOB"],
        DatabaseKind::ClickHouse => &[
            "Int8",
            "Int16",
            "Int32",
            "Int64",
            "UInt8",
            "UInt16",
            "UInt32",
            "UInt64",
            "Float32",
            "Float64",
            "Decimal(18,2)",
            "String",
            "FixedString(32)",
            "Date",
            "DateTime",
            "DateTime64(3)",
            "UUID",
            "Nullable(String)",
            "Array(String)",
        ],
        _ => &[],
    }
}
/// Replace the parsed type while retaining DEFAULT/NULL/other column options.
pub fn replace_type(kind: DatabaseKind, definition: &str, preset: &str) -> Result<String, String> {
    let parse = |text: &str| {
        let mut statements = Parser::parse_sql(
            dialect(kind).as_ref(),
            &format!("CREATE TABLE _design (_column {text})"),
        )
        .ok()?;
        match statements.remove(0) {
            Statement::CreateTable(mut table)
                if table.columns.len() == 1 && table.constraints.is_empty() =>
            {
                Some(table.columns.remove(0))
            }
            _ => None,
        }
    };
    if definition.trim().is_empty() {
        return valid_definition(kind, preset)
            .then(|| preset.to_string())
            .ok_or_else(|| "designer.invalid".into());
    }
    if !valid_definition(kind, definition) || !valid_definition(kind, preset) {
        return Err("designer.invalid".into());
    }
    let mut column = parse(definition).ok_or("designer.invalid")?;
    column.data_type = parse(preset).ok_or("designer.invalid")?.data_type;
    column
        .to_string()
        .strip_prefix("_column ")
        .map(str::to_string)
        .ok_or_else(|| "designer.invalid".into())
}

#[cfg(test)]
mod type_tests {
    use super::*;
    #[test]
    fn type_templates_keep_draft_attributes_and_driver_specific_choices() {
        let value = replace_type(
            DatabaseKind::PostgreSql,
            "varchar(12) NOT NULL DEFAULT 'sample'",
            "text",
        )
        .unwrap();
        assert_eq!(value, "TEXT NOT NULL DEFAULT 'sample'");
        for kind in [
            DatabaseKind::MySql,
            DatabaseKind::PostgreSql,
            DatabaseKind::Sqlite,
            DatabaseKind::ClickHouse,
        ] {
            for preset in type_presets(kind) {
                assert!(
                    valid_definition(kind, preset),
                    "invalid type template for {kind:?}"
                );
            }
        }
        assert!(type_presets(DatabaseKind::PostgreSql).contains(&"jsonb"));
        assert!(!type_presets(DatabaseKind::MySql).contains(&"jsonb"));
        assert_eq!(
            type_presets(DatabaseKind::Sqlite),
            &["INTEGER", "TEXT", "REAL", "NUMERIC", "BLOB"]
        );
        assert!(replace_type(DatabaseKind::Sqlite, "TEXT; DROP TABLE x", "INTEGER").is_err());
    }
}

#[cfg(test)]
mod picker_tests {
    use super::*;
    #[test]
    fn cleared_type_accepts_a_preset_without_accepting_multiple_statements() {
        assert_eq!(
            replace_type(DatabaseKind::Sqlite, "", "TEXT").unwrap(),
            "TEXT"
        );
        assert!(replace_type(DatabaseKind::Sqlite, "TEXT; DROP TABLE x", "INTEGER").is_err());
        assert!(replace_type(DatabaseKind::MySql, "INT", "INT; DROP TABLE x").is_err());
    }
    #[test]
    fn defaults_match_each_supported_dialect() {
        for kind in [
            DatabaseKind::MySql,
            DatabaseKind::PostgreSql,
            DatabaseKind::Sqlite,
            DatabaseKind::ClickHouse,
        ] {
            assert!(valid_definition(kind, default_type(kind)));
        }
        assert_eq!(default_type(DatabaseKind::ClickHouse), "String");
    }
}

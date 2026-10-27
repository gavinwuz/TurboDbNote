//! Index design is a SQL draft only; no server mutation is performed here.
use super::workbench::{self, Object, Schema};
use crate::DatabaseKind;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draft {
    pub name: String,
    pub columns: Vec<(String, bool)>,
    pub prefixes: std::collections::BTreeMap<String, u64>,
    pub unique: bool,
    pub previous: Option<String>,
    pub drop_only: bool,
}
pub fn protected(schema: &Schema, name: &str) -> bool {
    if name.eq_ignore_ascii_case("PRIMARY") || name.starts_with("sqlite_autoindex_") {
        return true;
    }
    if !schema.foreign_keys.values.is_empty() {
        return true;
    }
    schema
        .indexes
        .values
        .iter()
        .filter(|r| r.first().and_then(Option::as_deref) == Some(name))
        .any(|row| {
            schema
                .indexes
                .columns
                .iter()
                .zip(row)
                .any(|(column, value)| {
                    let column = column.to_lowercase();
                    let value = value.as_deref().unwrap_or("").to_lowercase();
                    (column == "non_unique" && value == "0")
                        || (column == "unique" && value == "1")
                        || (column == "origin" && value != "c")
                        || value.contains("unique index")
                })
        })
}
pub fn sql(
    kind: DatabaseKind,
    database: &str,
    object: &Object,
    schema: &Schema,
    draft: &Draft,
) -> Result<String, String> {
    if !matches!(
        kind,
        DatabaseKind::MySql | DatabaseKind::PostgreSql | DatabaseKind::Sqlite
    ) {
        return Err("indexes.unsupported".into());
    }
    sql_with_options(kind, database, object, schema, draft, true)
}
pub fn sql_with_options(
    kind: DatabaseKind,
    database: &str,
    object: &Object,
    schema: &Schema,
    draft: &Draft,
    include_database: bool,
) -> Result<String, String> {
    if !matches!(
        kind,
        DatabaseKind::MySql | DatabaseKind::PostgreSql | DatabaseKind::Sqlite
    ) {
        return Err("indexes.unsupported".into());
    }
    let valid = |value: &str| {
        !value.trim().is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
    };
    if !valid(&draft.name)
        || !valid(&object.name)
        || !valid(database)
        || (kind == DatabaseKind::MySql && draft.name.chars().count() > 64)
        || (kind == DatabaseKind::PostgreSql && draft.name.len() > 63)
    {
        return Err("indexes.invalid".into());
    }
    let table = if include_database {
        workbench::qualified(kind, database, object)
    } else if kind == DatabaseKind::PostgreSql && !object.schema.is_empty() {
        format!(
            "{}.{}",
            workbench::quote(kind, &object.schema),
            workbench::quote(kind, &object.name)
        )
    } else {
        workbench::quote(kind, &object.name)
    };
    let quote = |value: &str| workbench::quote(kind, value);
    let mut sql = String::new();
    if let Some(previous) = &draft.previous {
        if !schema
            .indexes
            .values
            .iter()
            .any(|r| r.first().and_then(Option::as_deref) == Some(previous))
            || protected(schema, previous)
        {
            return Err("indexes.protected".into());
        }
        let name = if kind == DatabaseKind::PostgreSql && !object.schema.is_empty() {
            format!("{}.{}", quote(&object.schema), quote(previous))
        } else {
            quote(previous)
        };
        sql.push_str(&if kind == DatabaseKind::MySql {
            format!("ALTER TABLE {table} DROP INDEX {name};\n")
        } else {
            format!("DROP INDEX {name};\n")
        });
    }
    if !draft.drop_only {
        if draft.columns.is_empty()
            || draft.columns.len() > 32
            || draft
                .columns
                .iter()
                .any(|(name, _)| !schema.columns.iter().any(|column| column.name == *name))
        {
            return Err("indexes.invalid".into());
        }
        if schema
            .indexes
            .values
            .iter()
            .any(|r| r.first().and_then(Option::as_deref) == Some(&draft.name))
            && draft.previous.as_deref() != Some(&draft.name)
        {
            return Err("indexes.duplicate".into());
        }
        let mut names = std::collections::HashSet::new();
        if draft.columns.iter().any(|(name, _)| !names.insert(name)) {
            return Err("indexes.invalid".into());
        }
        for (name, prefix) in &draft.prefixes {
            if kind != DatabaseKind::MySql
                || *prefix == 0
                || *prefix > 65535
                || !draft.columns.iter().any(|(column, _)| column == name)
                || !schema.columns.iter().any(|column| {
                    column.name == *name
                        && ["char", "text", "binary", "blob"]
                            .iter()
                            .any(|ty| column.data_type.to_lowercase().contains(ty))
                })
            {
                return Err("indexes.invalid".into());
            }
        }
        let columns = draft
            .columns
            .iter()
            .map(|(name, desc)| {
                format!(
                    "{}{}{}",
                    quote(name),
                    draft
                        .prefixes
                        .get(name)
                        .map(|length| format!("({length})"))
                        .unwrap_or_default(),
                    if *desc { " DESC" } else { "" }
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        sql.push_str(&format!(
            "CREATE {}INDEX {} ON {table} ({columns});\n",
            if draft.unique { "UNIQUE " } else { "" },
            quote(&draft.name)
        ));
    } else if draft.previous.is_none() {
        return Err("indexes.invalid".into());
    }
    Ok(sql)
}
pub fn suggest_name(
    kind: DatabaseKind,
    columns: &[(String, bool)],
    unique: bool,
    schema: &Schema,
) -> String {
    let limit = if kind == DatabaseKind::PostgreSql {
        63
    } else {
        64
    };
    let raw = format!(
        "{}_{}",
        if unique { "un" } else { "idx" },
        columns
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join("_")
    );
    let cut = |value: &str, size: usize| {
        let mut end = value.len().min(size);
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        value[..end].to_owned()
    };
    let mut name = cut(&raw, limit);
    for index in 2..=20_002 {
        if !schema.indexes.values.iter().any(|row| {
            row.first()
                .and_then(Option::as_deref)
                .is_some_and(|old| old.eq_ignore_ascii_case(&name))
        }) {
            break;
        }
        let suffix = format!("_{index}");
        name = format!("{}{}", cut(&raw, limit - suffix.len()), suffix);
    }
    name
}
type IndexSignature = (
    Vec<(String, bool)>,
    std::collections::BTreeMap<String, u64>,
    bool,
);
/// Ordinary BTREE indexes from verified ordered metadata. Unsupported changes stay manual.
pub(super) fn sync_mysql(
    database: &str,
    object: &Object,
    current: &Schema,
    desired: &Schema,
) -> (Vec<String>, bool) {
    let signature = |schema: &Schema, name: &str| -> Option<IndexSignature> {
        let columns = schema.index_columns.get(name)?;
        let row = schema
            .indexes
            .values
            .iter()
            .find(|row| row.first().and_then(Option::as_deref) == Some(name))?;
        let value = |key: &str| {
            schema
                .indexes
                .columns
                .iter()
                .position(|c| c.eq_ignore_ascii_case(key))
                .and_then(|i| row.get(i))
                .and_then(Option::as_deref)
        };
        if !value("Type")
            .unwrap_or("BTREE")
            .eq_ignore_ascii_case("BTREE")
        {
            return None;
        }
        if value("Comment").is_some_and(|value| !value.is_empty())
            || super::comparison::index_requires_review(schema, name)
        {
            return None;
        }
        let unique = value("Non_unique")? == "0";
        if columns.is_empty()
            || columns
                .iter()
                .any(|c| !schema.columns.iter().any(|field| field.name == c.name))
        {
            return None;
        }
        Some((
            columns
                .iter()
                .map(|c| (c.name.clone(), c.descending.unwrap_or(false)))
                .collect(),
            columns
                .iter()
                .filter_map(|c| c.prefix.map(|p| (c.name.clone(), p)))
                .collect(),
            unique,
        ))
    };
    let names = current
        .indexes
        .values
        .iter()
        .chain(&desired.indexes.values)
        .filter_map(|row| row.first().cloned().flatten())
        .collect::<std::collections::BTreeSet<_>>();
    let mut statements = vec![];
    let mut unresolved = false;
    for name in names {
        let a = signature(current, &name);
        let b = signature(desired, &name);
        if a == b && a.is_some() {
            continue;
        }
        let exists = |schema: &Schema| {
            schema
                .indexes
                .values
                .iter()
                .any(|r| r.first().and_then(Option::as_deref) == Some(name.as_str()))
        };
        if name.eq_ignore_ascii_case("PRIMARY") {
            if a != b {
                unresolved = true;
            }
            continue;
        }
        if !exists(desired) {
            if exists(current) {
                unresolved = true;
            }
            continue;
        } // Extra indexes are never silently dropped.
        let Some((columns, prefixes, unique)) = b else {
            unresolved = true;
            continue;
        };
        let draft = Draft {
            name: name.clone(),
            columns,
            prefixes,
            unique,
            previous: exists(current).then_some(name),
            drop_only: false,
        };
        match sql(DatabaseKind::MySql, database, object, current, &draft) {
            Ok(sql) => statements.push(sql),
            Err(_) => unresolved = true,
        }
    }
    (statements, unresolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generated_names_prefixes_and_optional_qualification_are_consistent() {
        let schema = Schema {
            columns: vec![
                workbench::Column {
                    name: "label".into(),
                    data_type: "varchar(30)".into(),
                    ..Default::default()
                },
                workbench::Column {
                    name: "id".into(),
                    data_type: "int".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let object = Object {
            name: "items".into(),
            schema: String::new(),
            kind: "table".into(),
            comment: String::new(),
            parent: String::new(),
        };
        let columns = vec![("label".into(), false), ("id".into(), true)];
        let name = suggest_name(DatabaseKind::MySql, &columns, true, &schema);
        assert_eq!(name, "un_label_id");
        let draft = Draft {
            name,
            columns,
            prefixes: std::collections::BTreeMap::from([("label".into(), 8)]),
            unique: true,
            previous: None,
            drop_only: false,
        };
        let plain = sql_with_options(
            DatabaseKind::MySql,
            "target",
            &object,
            &schema,
            &draft,
            false,
        )
        .unwrap();
        assert!(plain.contains("ON `items` (`label`(8), `id` DESC)"));
        assert!(!plain.contains(" ASC"));
        assert!(!plain.contains("`target`."));
        assert!(
            sql_with_options(
                DatabaseKind::MySql,
                "target",
                &object,
                &schema,
                &draft,
                true
            )
            .unwrap()
            .contains("`target`.`items`")
        );
    }
    #[test]
    fn drafts_quote_targets_protect_constraints_and_do_not_allow_unknown_columns() {
        let object = Object {
            name: "a`b".into(),
            kind: "table".into(),
            schema: String::new(),
            parent: String::new(),
            comment: String::new(),
        };
        let schema = Schema {
            columns: vec![workbench::Column {
                name: "id".into(),
                ..Default::default()
            }],
            indexes: workbench::Rows {
                columns: vec!["Index".into(), "Non_unique".into()],
                values: vec![
                    vec![Some("PRIMARY".into()), Some("0".into())],
                    vec![Some("old".into()), Some("1".into())],
                ],
            },
            ..Default::default()
        };
        let mut draft = Draft {
            name: "new".into(),
            prefixes: Default::default(),
            columns: vec![("id".into(), true)],
            unique: false,
            previous: Some("old".into()),
            drop_only: false,
        };
        let out = sql(DatabaseKind::MySql, "db", &object, &schema, &draft).unwrap();
        assert!(out.contains("`a``b`"));
        assert!(out.contains("`id` DESC"));
        assert!(out.contains("DROP INDEX `old`"));
        draft.previous = Some("PRIMARY".into());
        assert!(sql(DatabaseKind::MySql, "db", &object, &schema, &draft).is_err());
        draft.previous = None;
        draft.columns[0].0 = "missing".into();
        assert!(sql(DatabaseKind::MySql, "db", &object, &schema, &draft).is_err());
    }
}

#[cfg(test)]
mod sqlite_tests {
    use super::*;
    #[test]
    fn generated_sql_creates_rebuilds_and_removes_only_selected_fixture_index() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE items(id INTEGER PRIMARY KEY,label TEXT);CREATE INDEX old ON items(label);").unwrap();
        let object = Object {
            name: "items".into(),
            kind: "table".into(),
            schema: String::new(),
            parent: String::new(),
            comment: String::new(),
        };
        let schema = Schema {
            columns: vec![
                workbench::Column {
                    name: "id".into(),
                    ..Default::default()
                },
                workbench::Column {
                    name: "label".into(),
                    ..Default::default()
                },
            ],
            indexes: workbench::Rows {
                columns: vec!["name".into(), "unique".into(), "origin".into()],
                values: vec![vec![Some("old".into()), Some("0".into()), Some("c".into())]],
            },
            ..Default::default()
        };
        let draft = Draft {
            name: "new".into(),
            prefixes: Default::default(),
            columns: vec![("label".into(), true), ("id".into(), false)],
            previous: Some("old".into()),
            unique: false,
            drop_only: false,
        };
        db.execute_batch(&sql(DatabaseKind::Sqlite, "main", &object, &schema, &draft).unwrap())
            .unwrap();
        assert_eq!(
            db.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='new'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        assert_eq!(
            db.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='old'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }
}

pub fn compact_rows(rows: &workbench::Rows) -> workbench::Rows {
    let column = rows.columns.iter().position(|name| {
        name.eq_ignore_ascii_case("column") || name.eq_ignore_ascii_case("column_name")
    });
    let mut result = workbench::Rows {
        columns: rows.columns.clone(),
        values: vec![],
    };
    let mut groups = std::collections::BTreeMap::<String, usize>::new();
    for row in &rows.values {
        let name = row.first().cloned().flatten().unwrap_or_default();
        if let Some(index) = groups.get(&name).copied() {
            if let Some(column) = column {
                let value = row.get(column).cloned().flatten().unwrap_or_default();
                if !value.is_empty() {
                    let output = result.values[index][column].get_or_insert_with(String::new);
                    if !output.is_empty() {
                        output.push_str(", ");
                    }
                    output.push_str(&value);
                }
            }
        } else {
            groups.insert(name, result.values.len());
            result.values.push(row.clone());
        }
    }
    result
}

/// Preserve composite key order while presenting one catalog item per index.
pub fn mysql_metadata(
    rows: &workbench::Rows,
) -> (
    workbench::Rows,
    std::collections::BTreeMap<String, Vec<workbench::IndexColumn>>,
) {
    let value = |row: &Vec<Option<String>>, index: usize| {
        row.get(index).cloned().flatten().unwrap_or_default()
    };
    let mut source = rows.values.clone();
    source.sort_by_key(|row| (value(row, 2), value(row, 3).parse::<usize>().unwrap_or(0)));
    let mut details = std::collections::BTreeMap::<String, Vec<workbench::IndexColumn>>::new();
    let mut normalized = workbench::Rows {
        columns: vec![
            "Index".into(),
            "Column".into(),
            "Non_unique".into(),
            "Type".into(),
            "Comment".into(),
        ],
        values: vec![],
    };
    for row in source {
        let name = value(&row, 2);
        let column = value(&row, 4);
        let expression = value(&row, 14);
        let column = if column.is_empty() {
            expression
        } else {
            column
        };
        let descending = match value(&row, 5).as_str() {
            "A" => Some(false),
            "D" => Some(true),
            _ => None,
        };
        let prefix = value(&row, 7).parse::<u64>().ok();
        details
            .entry(name.clone())
            .or_default()
            .push(workbench::IndexColumn {
                name: column.clone(),
                position: value(&row, 3).parse().unwrap_or(0),
                descending,
                nullable: Some(value(&row, 9) == "YES"),
                prefix,
            });
        let column = format!(
            "{}{}{}",
            column,
            prefix
                .map(|prefix| format!("({prefix})"))
                .unwrap_or_default(),
            if descending == Some(true) {
                " DESC"
            } else {
                ""
            }
        );
        normalized.values.push(vec![
            Some(name),
            Some(column),
            row.get(1).cloned().flatten(),
            row.get(10).cloned().flatten(),
            row.get(12).cloned().flatten(),
        ]);
    }
    (compact_rows(&normalized), details)
}

#[cfg(test)]
mod metadata_tests {
    use super::*;
    #[test]
    fn composite_indexes_are_single_items_and_keep_order_direction_prefix() {
        let mut first = vec![None; 15];
        first[2] = Some("idx".into());
        first[3] = Some("1".into());
        first[4] = Some("a".into());
        first[5] = Some("A".into());
        first[7] = Some("8".into());
        first[9] = Some("YES".into());
        let mut second = first.clone();
        second[3] = Some("2".into());
        second[4] = Some("b".into());
        second[5] = Some("D".into());
        second[7] = None;
        let (rows, details) = mysql_metadata(&workbench::Rows {
            columns: vec![],
            values: vec![second, first],
        });
        assert_eq!(rows.values.len(), 1);
        assert_eq!(rows.values[0][1].as_deref(), Some("a(8), b DESC"));
        assert_eq!(details["idx"].len(), 2);
        assert_eq!(details["idx"][0].name, "a");
        assert_eq!(details["idx"][1].descending, Some(true));
    }
}

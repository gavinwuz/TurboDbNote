//! Bounded comparison; results describe samples, never whole-table equivalence.
use super::workbench::{Rows, Schema};
use std::collections::{BTreeMap, BTreeSet};
pub struct Compared {
    pub left: Rows,
    pub right: Rows,
    pub different: usize,
    pub keyed: bool,
}
pub fn data(left: &Rows, ls: &Schema, right: &Rows, rs: &Schema) -> Compared {
    let lk: Vec<_> = ls
        .columns
        .iter()
        .filter(|c| c.primary)
        .map(|c| c.name.as_str())
        .collect();
    let rk: Vec<_> = rs
        .columns
        .iter()
        .filter(|c| c.primary)
        .map(|c| c.name.as_str())
        .collect();
    let indices = |r: &Rows, keys: &[&str]| {
        keys.iter()
            .map(|k| r.columns.iter().position(|c| c == k))
            .collect::<Option<Vec<_>>>()
    };
    let index = |r: &Rows, keys: &[usize]| -> Option<BTreeMap<String, usize>> {
        let mut map = BTreeMap::new();
        for (i, row) in r.values.iter().enumerate() {
            let key = keys
                .iter()
                .map(|k| row.get(*k)?.as_ref())
                .collect::<Option<Vec<_>>>()?;
            if map.insert(serde_json::to_string(&key).ok()?, i).is_some() {
                return None;
            }
        }
        Some(map)
    };
    let compatible = lk.iter().all(|name| {
        ls.columns
            .iter()
            .find(|c| c.name == *name)
            .zip(rs.columns.iter().find(|c| c.name == *name))
            .is_some_and(|(l, r)| l.data_type.eq_ignore_ascii_case(&r.data_type))
    });
    let maps = (!lk.is_empty() && lk == rk && compatible)
        .then(|| {
            Some((
                index(left, &indices(left, &lk)?)?,
                index(right, &indices(right, &rk)?)?,
            ))
        })
        .flatten();
    let keyed = maps.is_some();
    let pairs: Vec<_> = if let Some((l, r)) = maps {
        l.keys()
            .chain(r.keys())
            .cloned()
            .collect::<BTreeSet<_>>()
            .iter()
            .map(|k| (l.get(k).copied(), r.get(k).copied()))
            .collect()
    } else {
        (0..left.values.len().max(right.values.len()))
            .map(|i| {
                (
                    (i < left.values.len()).then_some(i),
                    (i < right.values.len()).then_some(i),
                )
            })
            .collect()
    };
    let mut output = Compared {
        left: Rows {
            columns: std::iter::once("Δ".into())
                .chain(left.columns.clone())
                .collect(),
            values: vec![],
        },
        right: Rows {
            columns: std::iter::once("Δ".into())
                .chain(right.columns.clone())
                .collect(),
            values: vec![],
        },
        different: 0,
        keyed,
    };
    let right_columns = left
        .columns
        .iter()
        .map(|c| right.columns.iter().position(|v| v == c))
        .collect::<Option<Vec<_>>>();
    for (l, r) in pairs {
        let same = match (l, r, &right_columns) {
            (Some(l), Some(r), Some(cols)) if left.columns.len() == right.columns.len() => cols
                .iter()
                .enumerate()
                .all(|(i, j)| left.values[l].get(i) == right.values[r].get(*j)),
            _ => false,
        };
        if !same {
            output.different += 1;
        }
        for (source, pos, dest) in [(left, l, &mut output.left), (right, r, &mut output.right)] {
            let marker = if pos.is_none() {
                "∅"
            } else if same {
                "="
            } else {
                "≠"
            };
            let mut row = vec![Some(marker.into())];
            row.extend(
                pos.map(|i| source.values[i].clone())
                    .unwrap_or_else(|| vec![None; source.columns.len()]),
            );
            dest.values.push(row);
        }
    }
    output
}
pub fn ddl(left: &str, right: &str) -> Result<Compared, String> {
    let l: Vec<_> = left.lines().collect();
    let r: Vec<_> = right.lines().collect();
    if l.len() > 2000 || r.len() > 2000 {
        return Err("compare.ddl_limit".into());
    }
    let n = r.len() + 1;
    let mut dp = vec![0u16; (l.len() + 1) * n];
    for i in (0..l.len()).rev() {
        for j in (0..r.len()).rev() {
            dp[i * n + j] = if l[i] == r[j] {
                1 + dp[(i + 1) * n + j + 1]
            } else {
                dp[(i + 1) * n + j].max(dp[i * n + j + 1])
            };
        }
    }
    let mut out = Compared {
        left: Rows {
            columns: vec!["Δ".into(), "DDL".into()],
            values: vec![],
        },
        right: Rows {
            columns: vec!["Δ".into(), "DDL".into()],
            values: vec![],
        },
        different: 0,
        keyed: false,
    };
    let (mut i, mut j) = (0, 0);
    while i < l.len() || j < r.len() {
        let (a, b, same) = if i < l.len() && j < r.len() && l[i] == r[j] {
            let p = (Some(l[i]), Some(r[j]), true);
            i += 1;
            j += 1;
            p
        } else if i < l.len() && (j == r.len() || dp[(i + 1) * n + j] >= dp[i * n + j + 1]) {
            let p = (Some(l[i]), None, false);
            i += 1;
            p
        } else {
            let p = (None, Some(r[j]), false);
            j += 1;
            p
        };
        if !same {
            out.different += 1;
        }
        for (text, rows, marker) in [(a, &mut out.left, "-"), (b, &mut out.right, "+")] {
            rows.values.push(vec![
                Some(
                    if same {
                        "="
                    } else if text.is_none() {
                        "∅"
                    } else {
                        marker
                    }
                    .into(),
                ),
                text.map(str::to_owned),
            ]);
        }
    }
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::super::workbench::Column;
    use super::*;
    #[test]
    fn key_alignment_and_nulls() {
        let s = Schema {
            columns: vec![Column {
                name: "id".into(),
                primary: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        let rows = |v: Vec<Vec<Option<&str>>>| Rows {
            columns: vec!["id".into(), "value".into()],
            values: v
                .into_iter()
                .map(|r| r.into_iter().map(|v| v.map(str::to_string)).collect())
                .collect(),
        };
        let a = rows(vec![vec![Some("1"), None], vec![Some("2"), Some("b")]]);
        let b = rows(vec![vec![Some("2"), Some("c")], vec![Some("1"), None]]);
        let result = data(&a, &s, &b, &s);
        assert!(result.keyed);
        assert_eq!(result.different, 1);
        assert_eq!(result.left.values[0][0].as_deref(), Some("="));
    }
    #[test]
    fn ddl_insert_does_not_shift_every_following_line() {
        let result = ddl("a\nb\nc", "a\nx\nb\nc").unwrap();
        assert_eq!(result.different, 1);
        assert_eq!(result.left.values.len(), 4);
        assert!(ddl(&"x\n".repeat(2001), "").is_err());
    }
    #[test]
    fn ddl_markers_separate_source_removals_target_additions_and_aligned_gaps() {
        let compared = ddl("a\nb\nc", "a\nx\nc").unwrap();
        assert_eq!(compared.different, 2);
        assert_eq!(compared.left.values[1][0].as_deref(), Some("-"));
        assert_eq!(compared.right.values[1][0].as_deref(), Some("∅"));
        assert_eq!(compared.right.values[2][0].as_deref(), Some("+"));
        assert_eq!(compared.left.values[2][0].as_deref(), Some("∅"));
    }
}

#[path = "schema_sync.rs"]
mod schema_sync;
pub use schema_sync::{SyncDraft, sync_ddl};

#[derive(Clone, Copy, Default)]
pub struct CompareOptions {
    pub ignore_comments: bool,
    pub only_differences: bool,
}
pub fn fields_with_options(
    left: &Schema,
    right: &Schema,
    options: CompareOptions,
) -> Result<Compared, String> {
    let mut left = left.clone();
    let mut right = right.clone();
    if options.ignore_comments {
        for column in left.columns.iter_mut().chain(&mut right.columns) {
            column.comment.clear();
        }
    }
    let mut result = fields(&left, &right)?;
    if options.only_differences {
        filter_differences(&mut result);
    }
    Ok(result)
}
pub fn filter_differences(result: &mut Compared) {
    let keep = result
        .left
        .values
        .iter()
        .zip(&result.right.values)
        .map(|(a, b)| {
            a.first().and_then(Option::as_deref) != Some("=")
                || b.first().and_then(Option::as_deref) != Some("=")
        })
        .collect::<Vec<_>>();
    for rows in [&mut result.left, &mut result.right] {
        let mut i = 0;
        rows.values.retain(|_| {
            let k = keep[i];
            i += 1;
            k
        });
    }
}
/// Remove only parsed comment options; literals/defaults and generated expressions remain intact.
pub fn without_comments(kind: crate::DatabaseKind, schema: &Schema) -> Result<Schema, String> {
    use sqlparser::{
        ast::{ColumnOption, Statement},
        dialect::{ClickHouseDialect, Dialect, MySqlDialect, PostgreSqlDialect, SQLiteDialect},
        parser::Parser,
    };
    let dialect: Box<dyn Dialect> = match kind {
        crate::DatabaseKind::MySql => Box::new(MySqlDialect {}),
        crate::DatabaseKind::PostgreSql => Box::new(PostgreSqlDialect {}),
        crate::DatabaseKind::Sqlite => Box::new(SQLiteDialect {}),
        crate::DatabaseKind::ClickHouse => Box::new(ClickHouseDialect {}),
        _ => return Err("compare.sync_parse".into()),
    };
    let parsed = Parser::parse_sql(dialect.as_ref(), &schema.ddl);
    if parsed.is_err() && kind == crate::DatabaseKind::MySql {
        return schema_sync::mysql_comments(None, schema)
            .ok_or_else(|| "compare.sync_parse".into());
    }
    let mut statements = parsed.map_err(|_| "compare.sync_parse")?;
    if statements.len() != 1 {
        return Err("compare.sync_parse".into());
    }
    let Statement::CreateTable(table) = &mut statements[0] else {
        return Err("compare.sync_parse".into());
    };
    table.comment = None;
    match &mut table.table_options {
        sqlparser::ast::CreateTableOptions::With(v)
        | sqlparser::ast::CreateTableOptions::Options(v)
        | sqlparser::ast::CreateTableOptions::Plain(v)
        | sqlparser::ast::CreateTableOptions::TableProperties(v) => {
            v.retain(|o| !matches!(o, sqlparser::ast::SqlOption::Comment(_)))
        }
        _ => {}
    }

    for col in &mut table.columns {
        col.options
            .retain(|o| !matches!(o.option, ColumnOption::Comment(_)));
    }
    let mut output = schema.clone();
    output.ddl = statements[0].to_string();
    for c in &mut output.columns {
        c.comment.clear();
    }
    Ok(output)
}

/// A bounded read snapshot; metadata errors abort rather than being counted as equality.
#[derive(Clone)]
pub struct TableSnapshot {
    pub object: super::workbench::Object,
    pub schema: Schema,
    pub rows: Option<Rows>,
    pub more: bool,
}
#[derive(Clone)]
pub struct DatabaseSnapshot {
    pub tables: Vec<TableSnapshot>,
}
pub fn database_snapshot(
    endpoint: &super::transfer::Endpoint,
    include_data: bool,
    control: &super::transfer::Control,
) -> Result<DatabaseSnapshot, String> {
    database_snapshot_progress(endpoint, include_data, control, |_, _| {})
}
pub fn database_snapshot_progress(
    endpoint: &super::transfer::Endpoint,
    include_data: bool,
    control: &super::transfer::Control,
    mut progress: impl FnMut(usize, usize),
) -> Result<DatabaseSnapshot, String> {
    database_snapshot_stream(endpoint, include_data, control, &mut progress, |_| {})
}
pub fn database_snapshot_stream(
    endpoint: &super::transfer::Endpoint,
    include_data: bool,
    control: &super::transfer::Control,
    mut progress: impl FnMut(usize, usize),
    mut table_ready: impl FnMut(&TableSnapshot),
) -> Result<DatabaseSnapshot, String> {
    use super::{
        OperationKind, OperationResult,
        workbench::{Action, PageOptions, Reply, Request},
    };
    let call = |action| {
        super::transfer::service_operation(
            endpoint,
            OperationKind::Workbench(Box::new(Request {
                database: endpoint.profile.database.clone(),
                action,
            })),
            control,
        )
    };
    let mut objects = Vec::new();
    let mut offset = 0;
    loop {
        let OperationResult::Workbench(reply) = call(Action::BrowseCatalog {
            category: "relations".into(),
            search: String::new(),
            offset,
            limit: 200,
            include_comments: false,
        })?
        else {
            return Err("mcp.failed".into());
        };
        let Reply::CatalogPage {
            objects: page,
            more,
        } = *reply
        else {
            return Err("mcp.failed".into());
        };
        if page.is_empty() && more {
            return Err("compare.database_limit".into());
        }
        offset += page.len();
        objects.extend(page.into_iter().filter(|o| o.kind == "table"));
        if objects.len() > 20_000 || offset > 40_000 {
            return Err("compare.database_limit".into());
        }
        progress(0, objects.len());
        if !more {
            break;
        }
    }
    let total = objects.len();
    let mut tables = vec![];
    let mut snapshot_bytes = 0usize;
    for object in objects {
        let OperationResult::Workbench(reply) = call(if include_data {
            Action::GridPage {
                object: object.clone(),
                options: PageOptions {
                    page_size: 200,
                    ..Default::default()
                },
            }
        } else {
            Action::Describe(object.clone())
        })?
        else {
            return Err("mcp.failed".into());
        };
        let (schema, rows, more) = match *reply {
            Reply::Schema(s) => (s, None, false),
            Reply::GridPage {
                rows, schema, more, ..
            } => (schema, Some(rows), more),
            _ => return Err("mcp.failed".into()),
        };
        if schema.ddl.len() > 256_000 || schema.columns.len() > 2000 {
            return Err("compare.ddl_limit".into());
        }
        snapshot_bytes = snapshot_bytes
            .saturating_add(schema.ddl.len())
            .saturating_add(rows.as_ref().map_or(0, |r| {
                r.values
                    .iter()
                    .flatten()
                    .filter_map(Option::as_ref)
                    .map(String::len)
                    .sum()
            }));
        if snapshot_bytes > 32 * 1024 * 1024 {
            return Err("compare.database_limit".into());
        }
        tables.push(TableSnapshot {
            object,
            schema,
            rows,
            more,
        });
        if let Some(table) = tables.last() {
            table_ready(table);
        }
        progress(tables.len(), total);
    }
    tables.sort_by_key(table_key);
    Ok(DatabaseSnapshot { tables })
}
pub fn table_key(table: &TableSnapshot) -> String {
    if table.object.schema.is_empty() {
        table.object.name.clone()
    } else {
        format!("{}.{}", table.object.schema, table.object.name)
    }
}
pub fn database_pairs(left: &DatabaseSnapshot, right: &DatabaseSnapshot) -> Vec<String> {
    left.tables
        .iter()
        .chain(&right.tables)
        .map(table_key)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}
pub fn database_find<'a>(db: &'a DatabaseSnapshot, key: &str) -> Option<&'a TableSnapshot> {
    db.tables
        .binary_search_by(|table| table_key(table).as_str().cmp(key))
        .ok()
        .map(|index| &db.tables[index])
        .or_else(|| db.tables.iter().find(|table| table_key(table) == key))
}
#[derive(Clone, Debug)]
pub struct Difference {
    pub category: &'static str,
    pub reference: String,
    pub target: String,
}
fn canonical_rows(rows: &Rows) -> String {
    let mut rows = rows
        .values
        .iter()
        .map(|row| {
            row.iter()
                .map(|value| value.as_deref().unwrap_or("∅"))
                .collect::<Vec<_>>()
                .join(" · ")
        })
        .collect::<Vec<_>>();
    rows.sort();
    rows.join("\n")
}
pub(super) fn index_requires_review(schema: &Schema, name: &str) -> bool {
    schema_sync::mysql_parts::split(&schema.ddl).is_some_and(|parts| {
        parts.constraints.iter().any(|definition| {
            schema_sync::mysql_parts::index_name(definition).as_deref() == Some(name)
                && ["INVISIBLE", "WITH PARSER", "ENGINE_ATTRIBUTE", "IGNORED"]
                    .iter()
                    .any(|key| definition.to_ascii_uppercase().contains(key))
        })
    })
}
fn index_rows(schema: &Schema) -> Rows {
    let mut rows = schema.indexes.clone();
    for row in &mut rows.values {
        if let Some(columns) = row
            .first()
            .and_then(Option::as_ref)
            .and_then(|name| schema.index_columns.get(name))
        {
            row.push(Some(
                columns
                    .iter()
                    .map(|column| {
                        format!(
                            "{}{}{}",
                            column.name,
                            column.prefix.map(|p| format!("({p})")).unwrap_or_default(),
                            if column.descending == Some(true) {
                                " DESC"
                            } else {
                                ""
                            }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
            ));
        }
    }
    if let Some(parts) = schema_sync::mysql_parts::split(&schema.ddl) {
        for definition in parts.constraints {
            let upper = definition.to_ascii_uppercase();
            if (upper.contains("WITH PARSER")
                || upper.contains("INVISIBLE")
                || upper.starts_with("FULLTEXT")
                || upper.starts_with("SPATIAL"))
                && let Some(name) = schema_sync::mysql_parts::index_name(&definition)
                && let Some(row) = rows
                    .values
                    .iter_mut()
                    .find(|row| row.first().and_then(Option::as_deref) == Some(name.as_str()))
            {
                row.push(Some(definition));
            }
        }
    }
    rows
}
pub fn structural_differences(
    kind: crate::DatabaseKind,
    left: Option<&TableSnapshot>,
    right: Option<&TableSnapshot>,
    ignore_comments: bool,
    ignore_order: bool,
) -> Vec<Difference> {
    let (Some(left), Some(right)) = (left, right) else {
        return vec![Difference {
            category: "compare.reason.missing",
            reference: left
                .map(|table| table.schema.ddl.clone())
                .unwrap_or_default(),
            target: right
                .map(|table| table.schema.ddl.clone())
                .unwrap_or_default(),
        }];
    };
    let mut out = vec![];
    let mut push = |category, reference: String, target: String| {
        if reference != target {
            out.push(Difference {
                category,
                reference,
                target,
            });
        }
    };
    let normalize = |schema: &Schema| {
        if ignore_comments {
            without_comments(kind, schema).unwrap_or_else(|_| schema.clone())
        } else {
            schema.clone()
        }
    };
    let l = normalize(&left.schema);
    let r = normalize(&right.schema);
    if fields_with_options(
        &l,
        &r,
        CompareOptions {
            ignore_comments,
            only_differences: true,
        },
    )
    .map_or(true, |fields| fields.different > 0)
    {
        push(
            "compare.reason.fields",
            l.columns
                .iter()
                .map(|c| {
                    format!(
                        "{} {} {:?} {:?} {} {} {}",
                        c.name,
                        c.data_type,
                        c.not_null,
                        c.auto_increment,
                        c.primary,
                        c.extra,
                        c.comment
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
            r.columns
                .iter()
                .map(|c| {
                    format!(
                        "{} {} {:?} {:?} {} {} {}",
                        c.name,
                        c.data_type,
                        c.not_null,
                        c.auto_increment,
                        c.primary,
                        c.extra,
                        c.comment
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
    if !ignore_order {
        push(
            "compare.reason.order",
            l.columns
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            r.columns
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    push(
        "compare.reason.indexes",
        canonical_rows(&index_rows(&l)),
        canonical_rows(&index_rows(&r)),
    );
    push(
        "compare.reason.foreign",
        canonical_rows(&l.foreign_keys),
        canonical_rows(&r.foreign_keys),
    );
    push(
        "compare.reason.triggers",
        canonical_rows(&l.triggers),
        canonical_rows(&r.triggers),
    );
    match (
        schema_sync::comparison_parts(kind, &l),
        schema_sync::comparison_parts(kind, &r),
    ) {
        (Some((mut lc, la, lp)), Some((mut rc, ra, rp))) => {
            lc.sort_by(|a, b| a.0.cmp(&b.0));
            rc.sort_by(|a, b| a.0.cmp(&b.0));
            push(
                "compare.reason.definitions",
                lc.iter()
                    .map(|(_, definition)| definition.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
                rc.iter()
                    .map(|(_, definition)| definition.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
            push("compare.reason.constraints", la.join("\n"), ra.join("\n"));
            push("compare.reason.options", lp, rp);
        }
        _ => push("compare.reason.ddl", l.ddl.clone(), r.ddl.clone()),
    }
    out
}
/// Attribute-level structure differences, in reference-to-target synchronization direction.
pub fn structure_details(
    kind: crate::DatabaseKind,
    left: &Schema,
    right: &Schema,
    options: CompareOptions,
    ignore_order: bool,
    query: &str,
) -> Result<Rows, String> {
    let mut rows = Rows {
        columns: vec![
            "Δ".into(),
            "ddl.column.name".into(),
            "compare.attribute".into(),
            "compare.reference_value".into(),
            "compare.target_value".into(),
        ],
        values: vec![],
    };
    let marker = |a: bool, b: bool| {
        if !b {
            "+"
        } else if !a {
            "-"
        } else {
            "≠"
        }
    };
    let normalize = |schema: &Schema| {
        if options.ignore_comments {
            without_comments(kind, schema).unwrap_or_else(|_| schema.clone())
        } else {
            schema.clone()
        }
    };
    let l = normalize(left);
    let r = normalize(right);
    let query = query.to_lowercase();
    let mut add = |name: String, attribute: &str, a: Option<String>, b: Option<String>| {
        if (options.only_differences && a == b)
            || (!query.is_empty()
                && !format!(
                    "{name} {} {}",
                    a.as_deref().unwrap_or(""),
                    b.as_deref().unwrap_or("")
                )
                .to_lowercase()
                .contains(&query))
        {
            return;
        }
        rows.values.push(vec![
            Some(
                if a == b {
                    "="
                } else if attribute.starts_with("ddl.column.")
                    || matches!(
                        attribute,
                        "grid.structure.type"
                            | "compare.default_value"
                            | "compare.column_definition"
                            | "compare.reason.order"
                            | "compare.reason.definitions"
                    )
                {
                    marker(
                        l.columns.iter().any(|c| c.name == name),
                        r.columns.iter().any(|c| c.name == name),
                    )
                } else {
                    marker(a.is_some(), b.is_some())
                }
                .into(),
            ),
            Some(name),
            Some(attribute.into()),
            a,
            b,
        ]);
    };
    let detailed = match (
        schema_sync::column_attributes(kind, &l),
        schema_sync::column_attributes(kind, &r),
    ) {
        (Some(a), Some(b)) => {
            for name in a.keys().chain(b.keys()).collect::<BTreeSet<_>>() {
                let aa = a.get(name);
                let bb = b.get(name);
                if aa.is_none() || bb.is_none() {
                    add(
                        name.clone(),
                        "compare.column_definition",
                        aa.and_then(|v| v.get("compare.column_definition")).cloned(),
                        bb.and_then(|v| v.get("compare.column_definition")).cloned(),
                    );
                    continue;
                }
                let keys = aa
                    .into_iter()
                    .flat_map(|v| v.keys())
                    .chain(bb.into_iter().flat_map(|v| v.keys()))
                    .collect::<BTreeSet<_>>();
                for key in keys {
                    if key == "compare.column_definition" {
                        continue;
                    }
                    add(
                        name.clone(),
                        key,
                        aa.and_then(|v| v.get(key)).cloned(),
                        bb.and_then(|v| v.get(key)).cloned(),
                    );
                }
            }
            true
        }
        _ => {
            let fields = field_details(&l, &r, options, &query)?;
            for row in fields.values {
                add(
                    row[1].clone().unwrap_or_default(),
                    row[2].as_deref().unwrap_or(""),
                    row[3].clone(),
                    row[4].clone(),
                );
            }
            false
        }
    };
    if !ignore_order {
        for (index, column) in l.columns.iter().enumerate() {
            if let Some(target) = r.columns.iter().position(|c| c.name == column.name) {
                add(
                    column.name.clone(),
                    "compare.reason.order",
                    Some((index + 1).to_string()),
                    Some((target + 1).to_string()),
                );
            }
        }
    }
    let left_indexes = index_rows(&l);
    let right_indexes = index_rows(&r);
    for (category, a, b) in [
        ("compare.reason.indexes", &left_indexes, &right_indexes),
        ("compare.reason.foreign", &l.foreign_keys, &r.foreign_keys),
        ("compare.reason.triggers", &l.triggers, &r.triggers),
    ] {
        let map = |data: &Rows| -> BTreeMap<String, String> {
            data.values
                .iter()
                .filter_map(|row| {
                    Some((
                        row.first()?.clone()?,
                        row.iter()
                            .map(|v| v.as_deref().unwrap_or("NULL"))
                            .collect::<Vec<_>>()
                            .join(" · "),
                    ))
                })
                .collect()
        };
        let a = map(a);
        let b = map(b);
        for name in a.keys().chain(b.keys()).collect::<BTreeSet<_>>() {
            add(
                name.clone(),
                category,
                a.get(name).cloned(),
                b.get(name).cloned(),
            );
        }
    }
    match (
        schema_sync::comparison_parts(kind, &l),
        schema_sync::comparison_parts(kind, &r),
    ) {
        (Some((a, ac, ap)), Some((b, bc, bp))) => {
            let a = a.into_iter().collect::<BTreeMap<_, _>>();
            let b = b.into_iter().collect::<BTreeMap<_, _>>();
            if !detailed {
                for name in a.keys().chain(b.keys()).collect::<BTreeSet<_>>() {
                    add(
                        name.clone(),
                        "compare.reason.definitions",
                        a.get(name).cloned(),
                        b.get(name).cloned(),
                    );
                }
            }
            let map = |values: Vec<String>| {
                let mut map = BTreeMap::<String, String>::new();
                for value in values {
                    let key = value.split('(').next().unwrap_or(&value).trim().to_string();
                    map.entry(key)
                        .and_modify(|v| {
                            v.push_str("; ");
                            v.push_str(&value);
                        })
                        .or_insert(value);
                }
                map
            };
            let a = map(ac);
            let b = map(bc);
            for name in a.keys().chain(b.keys()).collect::<BTreeSet<_>>() {
                add(
                    name.clone(),
                    "compare.reason.constraints",
                    a.get(name).cloned(),
                    b.get(name).cloned(),
                );
            }
            add(String::new(), "compare.reason.options", Some(ap), Some(bp));
        }
        _ => add(
            String::new(),
            "compare.reason.ddl",
            Some(l.ddl),
            Some(r.ddl),
        ),
    }
    Ok(rows)
}
pub fn table_different(
    kind: crate::DatabaseKind,
    left: Option<&TableSnapshot>,
    right: Option<&TableSnapshot>,
    ignore_comments: bool,
) -> bool {
    !structural_differences(kind, left, right, ignore_comments, false).is_empty()
}
/// Copy-only, optimistic data repair for the loaded rows. Never implies full-table equality.
/// Deletes are intentionally excluded; no key or truncated/unknown values means no draft.
pub fn data_sync(
    kind: crate::DatabaseKind,
    database: &str,
    object: &super::workbench::Object,
    left: &TableSnapshot,
    right: &TableSnapshot,
) -> Result<String, String> {
    use super::workbench::{literal_for, qualified, quote};
    if left
        .schema
        .columns
        .iter()
        .chain(&right.schema.columns)
        .any(|c| {
            c.extra.to_ascii_uppercase().contains("GENERATED")
                || matches!(
                    c.data_type.to_ascii_uppercase().as_str(),
                    "BLOB" | "BYTEA" | "BINARY"
                )
        })
    {
        return Err("compare.data_sync_unavailable".into());
    }
    if kind == crate::DatabaseKind::ClickHouse {
        return Err("compare.data_sync_unavailable".into());
    }
    let (l, r) = (
        left.rows.as_ref().ok_or("compare.data_sync_unavailable")?,
        right.rows.as_ref().ok_or("compare.data_sync_unavailable")?,
    );
    if l.columns != r.columns
        || left
            .schema
            .columns
            .iter()
            .map(|c| (&c.name, &c.data_type))
            .collect::<Vec<_>>()
            != right
                .schema
                .columns
                .iter()
                .map(|c| (&c.name, &c.data_type))
                .collect::<Vec<_>>()
    {
        return Err("compare.data_sync_unavailable".into());
    }
    let keys = left
        .schema
        .columns
        .iter()
        .filter(|c| c.primary)
        .map(|c| c.name.clone())
        .collect::<Vec<_>>();
    if keys.is_empty()
        || keys
            != right
                .schema
                .columns
                .iter()
                .filter(|c| c.primary)
                .map(|c| c.name.clone())
                .collect::<Vec<_>>()
    {
        return Err("compare.data_sync_unavailable".into());
    }
    let indices = keys
        .iter()
        .map(|k| {
            l.columns
                .iter()
                .position(|n| n == k)
                .ok_or("compare.data_sync_unavailable")
        })
        .collect::<Result<Vec<_>, _>>()?;
    let key = |row: &Vec<Option<String>>| {
        indices
            .iter()
            .map(|i| {
                row.get(*i)
                    .cloned()
                    .flatten()
                    .ok_or("compare.data_sync_unavailable")
            })
            .collect::<Result<Vec<_>, _>>()
    };
    let mut target = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for row in &r.values {
        if row.len() != r.columns.len() || target.insert(key(row)?, row).is_some() {
            return Err("compare.data_sync_unavailable".into());
        }
    }
    let value = |v: &Option<String>| {
        v.as_deref()
            .map(|s| literal_for(kind, s))
            .unwrap_or_else(|| "NULL".into())
    };
    let condition = |row: &Vec<Option<String>>| {
        l.columns
            .iter()
            .zip(row)
            .map(|(n, v)| match v {
                None => format!("{} IS NULL", quote(kind, n)),
                Some(_) => format!("{} = {}", quote(kind, n), value(v)),
            })
            .collect::<Vec<_>>()
            .join(" AND ")
    };
    let table = qualified(kind, database, object);
    let mut sql = String::from(
        "-- Loaded sample only. Recheck current target values before execution. No DELETE statements.\n",
    );
    for row in &l.values {
        if row.len() != l.columns.len() || !seen.insert(key(row)?) {
            return Err("compare.data_sync_unavailable".into());
        }
        match target.get(&key(row)?) {
            Some(old) if *old != row => {
                let assignments = l
                    .columns
                    .iter()
                    .zip(row)
                    .filter(|(n, _)| !keys.contains(n))
                    .map(|(n, v)| format!("{} = {}", quote(kind, n), value(v)))
                    .collect::<Vec<_>>();
                if !assignments.is_empty() {
                    sql.push_str(&format!(
                        "UPDATE {table} SET {} WHERE {};\n",
                        assignments.join(", "),
                        condition(old)
                    ));
                }
            }
            None => {
                // Absence from a sample is not absence from the database: use NOT EXISTS.
                let key_where = indices
                    .iter()
                    .map(|i| format!("{} = {}", quote(kind, &l.columns[*i]), value(&row[*i])))
                    .collect::<Vec<_>>()
                    .join(" AND ");
                sql.push_str(&format!("INSERT INTO {table} ({}) SELECT {} WHERE NOT EXISTS (SELECT 1 FROM {table} WHERE {key_where});\n",l.columns.iter().map(|n|quote(kind,n)).collect::<Vec<_>>().join(", "),row.iter().map(value).collect::<Vec<_>>().join(", ")));
            }
            _ => {}
        }
    }
    if sql.len() > 1024 * 1024 {
        return Err("compare.ddl_limit".into());
    }
    Ok(sql)
}

/// Align fields by exact identifier; unmatched rows remain visible on both sides.
pub fn fields(left: &Schema, right: &Schema) -> Result<Compared, String> {
    if left.columns.len() > 2000 || right.columns.len() > 2000 {
        return Err("compare.ddl_limit".into());
    }
    let names = left
        .columns
        .iter()
        .chain(&right.columns)
        .map(|c| c.name.clone())
        .collect::<BTreeSet<_>>();
    let columns = vec![
        "Δ",
        "ddl.column.name",
        "grid.structure.type",
        "ddl.column.not_null",
        "ddl.column.auto_increment",
        "ddl.column.primary",
        "ddl.column.extra",
        "ddl.column.comment",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<Vec<_>>();
    let mut out = Compared {
        left: Rows {
            columns: columns.clone(),
            values: vec![],
        },
        right: Rows {
            columns,
            values: vec![],
        },
        different: 0,
        keyed: false,
    };
    let row = |c: &super::workbench::Column| {
        let flag = |v: Option<bool>| {
            Some(
                match v {
                    Some(true) => "✓",
                    Some(false) => "",
                    None => "—",
                }
                .to_string(),
            )
        };
        vec![
            Some(c.name.clone()),
            Some(c.data_type.clone()),
            flag(c.not_null),
            flag(c.auto_increment),
            flag(Some(c.primary)),
            Some(c.extra.clone()),
            Some(c.comment.clone()),
        ]
    };
    for name in names {
        let l = left.columns.iter().find(|c| c.name == name).map(row);
        let r = right.columns.iter().find(|c| c.name == name).map(row);
        let same = l == r;
        if !same {
            out.different += 1;
        }
        for (value, rows, marker) in [(l, &mut out.left, "-"), (r, &mut out.right, "+")] {
            let mut output = vec![Some(
                if value.is_none() {
                    "∅"
                } else if same {
                    "="
                } else {
                    marker
                }
                .to_string(),
            )];
            output.extend(value.unwrap_or_else(|| vec![None; 7]));
            rows.values.push(output);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod field_tests {
    use super::super::workbench::Column;
    use super::*;
    #[test]
    fn fields_align_names_and_keep_missing_and_changed_attributes() {
        let left = Schema {
            columns: vec![
                Column {
                    name: "id".into(),
                    data_type: "INT".into(),
                    ..Default::default()
                },
                Column {
                    name: "old".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let right = Schema {
            columns: vec![
                Column {
                    name: "id".into(),
                    data_type: "BIGINT".into(),
                    ..Default::default()
                },
                Column {
                    name: "new".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let result = fields(&left, &right).unwrap();
        assert_eq!(result.different, 3);
        assert_eq!(result.left.values.len(), result.right.values.len());
        assert_eq!(result.left.values[0][1].as_deref(), Some("id"));
        assert_eq!(result.left.values[1][0].as_deref(), Some("∅"));
        assert_eq!(result.right.values[2][0].as_deref(), Some("∅"));
    }
}

/// Missing target table: copy-only draft, target-name rewrite via AST.
pub fn create_table_draft(
    kind: crate::DatabaseKind,
    database: &str,
    object: &super::workbench::Object,
    source: &Schema,
) -> Result<SyncDraft, String> {
    use sqlparser::{
        ast::Statement,
        dialect::{ClickHouseDialect, Dialect, MySqlDialect, PostgreSqlDialect, SQLiteDialect},
        parser::Parser,
    };
    let dialect: Box<dyn Dialect> = match kind {
        crate::DatabaseKind::MySql => Box::new(MySqlDialect {}),
        crate::DatabaseKind::PostgreSql => Box::new(PostgreSqlDialect {}),
        crate::DatabaseKind::Sqlite => Box::new(SQLiteDialect {}),
        crate::DatabaseKind::ClickHouse => Box::new(ClickHouseDialect {}),
        _ => return Err("compare.sync_parse".into()),
    };
    let mut source =
        Parser::parse_sql(dialect.as_ref(), &source.ddl).map_err(|_| "compare.sync_parse")?;
    if source.len() != 1 {
        return Err("compare.sync_parse".into());
    }
    let Statement::CreateTable(table) = &mut source[0] else {
        return Err("compare.sync_parse".into());
    };
    let target = super::workbench::qualified(kind, database, object);
    let mut dummy = Parser::parse_sql(dialect.as_ref(), &format!("CREATE TABLE {target}(id INT)"))
        .map_err(|_| "compare.sync_parse")?;
    let Statement::CreateTable(target) = dummy.remove(0) else {
        return Err("compare.sync_parse".into());
    };
    table.name = target.name;
    table.if_not_exists = true;
    Ok(SyncDraft {
        sql: format!(
            "-- Review foreign-key references, engines and replication paths before execution.\n{};\n",
            source[0]
        ),
        incomplete: true,
        issues: vec![],
    })
}

/// Ignoring comments must preserve existing target comments in a full-column MODIFY.
pub fn sync_preserving_comments(
    kind: crate::DatabaseKind,
    database: &str,
    object: &super::workbench::Object,
    current: &Schema,
    desired: &Schema,
) -> Result<SyncDraft, String> {
    use sqlparser::{
        ast::{ColumnOption, Statement},
        dialect::{ClickHouseDialect, Dialect, MySqlDialect, PostgreSqlDialect, SQLiteDialect},
        parser::Parser,
    };
    let dialect: Box<dyn Dialect> = match kind {
        crate::DatabaseKind::MySql => Box::new(MySqlDialect {}),
        crate::DatabaseKind::PostgreSql => Box::new(PostgreSqlDialect {}),
        crate::DatabaseKind::Sqlite => Box::new(SQLiteDialect {}),
        crate::DatabaseKind::ClickHouse => Box::new(ClickHouseDialect {}),
        _ => return Err("compare.sync_parse".into()),
    };
    let old = Parser::parse_sql(dialect.as_ref(), &current.ddl);
    let new = Parser::parse_sql(dialect.as_ref(), &desired.ddl);
    if kind == crate::DatabaseKind::MySql && (old.is_err() || new.is_err()) {
        let desired =
            schema_sync::mysql_comments(Some(current), desired).ok_or("compare.sync_parse")?;
        return sync_ddl(kind, database, object, current, &desired);
    }
    let old = old.map_err(|_| "compare.sync_parse")?;
    let mut new = new.map_err(|_| "compare.sync_parse")?;
    if old.len() != 1 || new.len() != 1 {
        return Err("compare.sync_parse".into());
    }
    let (Statement::CreateTable(old), Statement::CreateTable(new_table)) = (&old[0], &mut new[0])
    else {
        return Err("compare.sync_parse".into());
    };
    new_table.comment = old.comment.clone();
    let comments = match &old.table_options {
        sqlparser::ast::CreateTableOptions::With(v)
        | sqlparser::ast::CreateTableOptions::Options(v)
        | sqlparser::ast::CreateTableOptions::Plain(v)
        | sqlparser::ast::CreateTableOptions::TableProperties(v) => v
            .iter()
            .filter(|o| matches!(o, sqlparser::ast::SqlOption::Comment(_)))
            .cloned()
            .collect::<Vec<_>>(),
        _ => vec![],
    };
    match &mut new_table.table_options {
        sqlparser::ast::CreateTableOptions::With(v)
        | sqlparser::ast::CreateTableOptions::Options(v)
        | sqlparser::ast::CreateTableOptions::Plain(v)
        | sqlparser::ast::CreateTableOptions::TableProperties(v) => {
            v.retain(|o| !matches!(o, sqlparser::ast::SqlOption::Comment(_)));
            v.extend(comments);
        }
        sqlparser::ast::CreateTableOptions::None => {
            if !comments.is_empty() {
                new_table.table_options = sqlparser::ast::CreateTableOptions::Plain(comments);
            }
        }
    }

    for col in &mut new_table.columns {
        if let Some(previous) = old.columns.iter().find(|c| c.name.value == col.name.value) {
            col.options
                .retain(|o| !matches!(o.option, ColumnOption::Comment(_)));
            col.options.extend(
                previous
                    .options
                    .iter()
                    .filter(|o| matches!(o.option, ColumnOption::Comment(_)))
                    .cloned(),
            );
        }
    }
    let mut desired = desired.clone();
    desired.ddl = new[0].to_string();
    sync_ddl(kind, database, object, current, &desired)
}

/// Safe default: deletions are left for a separate explicit DBA review.
pub fn omit_column_deletions(
    kind: crate::DatabaseKind,
    draft: &mut SyncDraft,
) -> Result<(), String> {
    use sqlparser::{
        dialect::GenericDialect,
        tokenizer::{Token, Tokenizer},
    };
    let mut sql = String::new();
    let dialect = crate::sql_script::dialect(kind).ok_or("compare.sync_parse")?;
    let ranges = crate::sql_script::editor_ranges(&draft.sql, dialect)?;
    if ranges.is_empty() {
        return Ok(());
    }
    for range in ranges {
        let statement = &draft.sql[range];
        let tokens = Tokenizer::new(&GenericDialect {}, statement)
            .tokenize()
            .map_err(|_| "compare.sync_parse")?
            .into_iter()
            .filter(|t| !matches!(t, Token::Whitespace(_)))
            .collect::<Vec<_>>();
        let drops=tokens.windows(2).any(|w|matches!((&w[0],&w[1]),(Token::Word(a),Token::Word(b))if a.quote_style.is_none()&&b.quote_style.is_none()&&a.value.eq_ignore_ascii_case("DROP")&&b.value.eq_ignore_ascii_case("COLUMN")));
        if drops {
            draft.incomplete = true;
            sql.push_str("-- Column deletion omitted; explicit DBA review required.\n");
        } else if !statement.trim().is_empty() {
            sql.push_str(statement);
            if !statement.trim_end().ends_with(';') {
                sql.push(';');
            }
            sql.push('\n');
        }
    }
    draft.sql = sql;
    Ok(())
}

#[cfg(test)]
mod database_tests {
    use super::super::workbench::{Column, Object};
    use super::*;
    use crate::DatabaseKind;
    fn snapshot(comment: &str, ty: &str) -> TableSnapshot {
        TableSnapshot {
            object: Object {
                schema: String::new(),
                name: "items".into(),
                kind: "table".into(),
                comment: String::new(),
                parent: String::new(),
            },
            schema: Schema {
                columns: vec![Column {
                    name: "id".into(),
                    data_type: ty.into(),
                    comment: comment.into(),
                    primary: true,
                    not_null: Some(true),
                    ..Default::default()
                }],
                ddl: format!(
                    "CREATE TABLE items(id {ty} NOT NULL PRIMARY KEY COMMENT '{comment}') COMMENT='{comment}'"
                ),
                ..Default::default()
            },
            rows: Some(Rows {
                columns: vec!["id".into()],
                values: vec![vec![Some("1".into())]],
            }),
            more: false,
        }
    }
    #[test]
    fn database_ignore_comments_preserves_target_comment_when_type_changes() {
        let current = snapshot("keep target", "INT");
        let desired = snapshot("source comment", "BIGINT");
        let draft = sync_preserving_comments(
            DatabaseKind::MySql,
            "target",
            &current.object,
            &current.schema,
            &desired.schema,
        )
        .unwrap();
        assert!(draft.sql.contains("BIGINT"));
        assert!(draft.sql.contains("keep target"));
        assert!(!draft.sql.contains("source comment"));
        let desired = snapshot("different comment", "INT");
        assert!(
            !table_different(DatabaseKind::MySql, Some(&current), Some(&desired), true),
            "normalized fixtures: {:?} / {:?}",
            without_comments(DatabaseKind::MySql, &current.schema)
                .unwrap()
                .ddl,
            without_comments(DatabaseKind::MySql, &desired.schema)
                .unwrap()
                .ddl
        );
        assert!(table_different(
            DatabaseKind::MySql,
            Some(&current),
            Some(&desired),
            false
        ));
        assert_eq!(
            fields_with_options(
                &current.schema,
                &desired.schema,
                CompareOptions {
                    ignore_comments: true,
                    only_differences: true
                }
            )
            .unwrap()
            .left
            .values
            .len(),
            0
        );
    }
    #[test]
    fn data_sync_is_keyed_and_never_infers_deletion_from_samples() {
        let mut desired = snapshot("", "INT");
        let mut current = desired.clone();
        desired
            .rows
            .as_mut()
            .unwrap()
            .values
            .push(vec![Some("2".into())]);
        current.more = true;
        let sql = data_sync(
            DatabaseKind::MySql,
            "target",
            &current.object,
            &desired,
            &current,
        )
        .unwrap();
        assert!(sql.contains("NOT EXISTS"));
        assert!(!sql.contains("DELETE FROM"));
        current.schema.columns[0].primary = false;
        assert!(
            data_sync(
                DatabaseKind::MySql,
                "target",
                &current.object,
                &desired,
                &current
            )
            .is_err()
        );
    }
    #[test]
    fn omitted_deletions_do_not_remove_a_literal_containing_drop_words() {
        let mut draft=SyncDraft{sql:"ALTER TABLE t DROP COLUMN old; ALTER TABLE t ADD COLUMN label TEXT DEFAULT 'DROP COLUMN';".into(),incomplete:false,issues:vec![]};
        omit_column_deletions(DatabaseKind::MySql, &mut draft).unwrap();
        assert!(draft.incomplete);
        assert!(!draft.sql.contains("DROP COLUMN old"));
        assert!(draft.sql.contains("DEFAULT 'DROP COLUMN'"));
    }
    #[test]
    fn missing_table_uses_target_name_not_source_database() {
        let source = snapshot("", "INT");
        let sql = create_table_draft(DatabaseKind::MySql, "newdb", &source.object, &source.schema)
            .unwrap()
            .sql;
        assert!(sql.contains("`newdb`.`items`"));
        assert!(sql.contains("IF NOT EXISTS"));
    }
    #[test]
    fn database_snapshot_reads_only_owned_sqlite_fixture() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("owned.db");
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE items(id INTEGER PRIMARY KEY,label TEXT NOT NULL DEFAULT ''); INSERT INTO items VALUES(1,'fixture');").unwrap();
        drop(db);
        let mut profile = crate::driver_catalog::find("sqlite").unwrap().profile();
        profile.sqlite_path = path.display().to_string();
        profile.sqlite_read_only = true;
        let endpoint = super::super::transfer::Endpoint {
            profile,
            password: String::new(),
        };
        let result =
            database_snapshot(&endpoint, true, &super::super::transfer::Control::default())
                .unwrap();
        assert_eq!(result.tables.len(), 1);
        assert_eq!(result.tables[0].schema.columns.len(), 2);
        assert_eq!(result.tables[0].rows.as_ref().unwrap().values.len(), 1);
        let c = super::super::transfer::Control::default();
        c.cancel();
        assert!(database_snapshot(&endpoint, false, &c).is_err());
    }
}

/// Attribute-aligned rows avoid two very wide schema tables on narrow split panes.
pub fn field_details(
    left: &Schema,
    right: &Schema,
    options: CompareOptions,
    query: &str,
) -> Result<Rows, String> {
    let compared = fields_with_options(
        left,
        right,
        CompareOptions {
            ignore_comments: options.ignore_comments,
            only_differences: false,
        },
    )?;
    let query = query.to_lowercase();
    let mut rows = Rows {
        columns: vec![
            "Δ".into(),
            "ddl.column.name".into(),
            "compare.attribute".into(),
            "compare.reference_value".into(),
            "compare.target_value".into(),
        ],
        values: vec![],
    };
    for (l, r) in compared.left.values.iter().zip(&compared.right.values) {
        let name = l
            .get(1)
            .and_then(Option::as_ref)
            .or_else(|| r.get(1).and_then(Option::as_ref))
            .cloned()
            .unwrap_or_default();
        if !query.is_empty() && !name.to_lowercase().contains(&query) {
            continue;
        }
        let different = l.first().and_then(Option::as_deref) != Some("=");
        if options.only_differences && !different {
            continue;
        }
        for index in 2..compared.left.columns.len() {
            let a = l.get(index).cloned().flatten();
            let b = r.get(index).cloned().flatten();
            if options.only_differences && a == b {
                continue;
            }
            if index == 7 && options.ignore_comments {
                continue;
            }
            rows.values.push(vec![
                Some(if a == b { "=" } else { "≠" }.into()),
                Some(name.clone()),
                Some(compared.left.columns[index].clone()),
                a,
                b,
            ]);
        }
    }
    Ok(rows)
}

#[cfg(test)]
mod large_catalog_tests {
    use super::*;
    #[test]
    fn paged_sqlite_snapshot_exceeds_legacy_499_table_limit_and_reports_progress() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("large.db");
        let db = rusqlite::Connection::open(&path).unwrap();
        let sql = (0..505)
            .map(|index| format!("CREATE TABLE t_{index:04}(id INTEGER PRIMARY KEY,label TEXT);"))
            .collect::<String>();
        db.execute_batch(&sql).unwrap();
        drop(db);
        let mut profile = crate::driver_catalog::find("sqlite").unwrap().profile();
        profile.sqlite_path = path.display().to_string();
        profile.sqlite_read_only = true;
        let endpoint = super::super::transfer::Endpoint {
            profile,
            password: String::new(),
        };
        let mut last = 0;
        let mut streamed = 0;
        let snapshot = database_snapshot_stream(
            &endpoint,
            false,
            &super::super::transfer::Control::default(),
            |done, _| last = done,
            |_| streamed += 1,
        )
        .unwrap();
        assert_eq!((snapshot.tables.len(), last, streamed), (505, 505, 505));
    }
    #[test]
    fn attribute_rows_find_long_columns_and_only_include_actual_differences() {
        let left = Schema {
            columns: (0..600)
                .map(|index| super::super::workbench::Column {
                    name: format!("field_{index}"),
                    data_type: "INT".into(),
                    not_null: Some(true),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        let mut right = left.clone();
        right.columns[599].data_type = "BIGINT".into();
        right.columns[599].comment = "changed comment".into();
        let rows = field_details(
            &left,
            &right,
            CompareOptions {
                ignore_comments: true,
                only_differences: true,
            },
            "field_599",
        )
        .unwrap();
        assert_eq!(rows.values.len(), 1);
        assert_eq!(rows.values[0][1].as_deref(), Some("field_599"));
        assert_eq!(rows.values[0][3].as_deref(), Some("INT"));
        assert_eq!(rows.values[0][4].as_deref(), Some("BIGINT"));
    }
}

#[cfg(test)]
mod classification_tests {
    use super::*;
    fn table(ddl: &str, order: &[&str]) -> TableSnapshot {
        TableSnapshot {
            object: super::super::workbench::Object {
                schema: String::new(),
                name: "items".into(),
                kind: "table".into(),
                parent: String::new(),
                comment: String::new(),
            },
            schema: Schema {
                ddl: ddl.into(),
                columns: order
                    .iter()
                    .map(|name| super::super::workbench::Column {
                        name: (*name).into(),
                        data_type: "INT".into(),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            },
            rows: None,
            more: false,
        }
    }
    #[test]
    fn column_order_is_explicit_and_allocation_counters_are_not_structure() {
        let a = table(
            "CREATE TABLE a(id INT,label INT) ENGINE=InnoDB AUTO_INCREMENT=10",
            &["id", "label"],
        );
        let b = table(
            "CREATE TABLE b(label INT,id INT) ENGINE=InnoDB AUTO_INCREMENT=500",
            &["label", "id"],
        );
        let changes =
            structural_differences(crate::DatabaseKind::MySql, Some(&a), Some(&b), false, false);
        assert!(
            changes
                .iter()
                .any(|change| change.category == "compare.reason.order")
        );
        assert!(
            structural_differences(crate::DatabaseKind::MySql, Some(&a), Some(&b), false, true)
                .is_empty()
        );
    }
    #[test]
    fn defaults_and_indexes_have_visible_reasons_when_metadata_fields_match() {
        let a = table("CREATE TABLE a(id INT DEFAULT 0)", &["id"]);
        let mut b = table("CREATE TABLE b(id INT DEFAULT 1)", &["id"]);
        b.schema.indexes = Rows {
            columns: vec!["Index".into(), "Column".into()],
            values: vec![vec![Some("idx".into()), Some("id".into())]],
        };
        let changes =
            structural_differences(crate::DatabaseKind::MySql, Some(&a), Some(&b), false, false);
        assert!(
            changes
                .iter()
                .any(|change| change.category == "compare.reason.definitions")
        );
        assert!(
            changes
                .iter()
                .any(|change| change.category == "compare.reason.indexes")
        );
    }
    #[test]
    fn attribute_details_locate_default_and_type_without_debug_or_unchanged_fields() {
        let a = table(
            "CREATE TABLE a(id INT DEFAULT 0, same VARCHAR(20), added INT)",
            &["id", "same", "added"],
        );
        let b = table(
            "CREATE TABLE b(id BIGINT DEFAULT 1, same VARCHAR(20), removed INT)",
            &["id", "same", "removed"],
        );
        let rows = structure_details(
            crate::DatabaseKind::MySql,
            &a.schema,
            &b.schema,
            CompareOptions {
                ignore_comments: false,
                only_differences: true,
            },
            true,
            "",
        )
        .unwrap();
        assert!(!rows.values.iter().any(|r| r[1].as_deref() == Some("same")));
        assert_eq!(
            rows.values
                .iter()
                .filter(|r| r[1].as_deref() == Some("added"))
                .count(),
            1
        );
        assert!(rows.values.iter().any(|r| r[1].as_deref() == Some("id")
            && r[2].as_deref() == Some("compare.default_value")
            && r[3].as_deref() == Some("0")
            && r[4].as_deref() == Some("1")));
        assert!(
            rows.values
                .iter()
                .any(|r| r[1].as_deref() == Some("added") && r[0].as_deref() == Some("+"))
        );
        assert!(
            rows.values
                .iter()
                .any(|r| r[1].as_deref() == Some("removed") && r[0].as_deref() == Some("-"))
        );
        assert!(
            !rows
                .values
                .iter()
                .flatten()
                .flatten()
                .any(|v| v.contains("Some("))
        );
    }
    #[test]
    fn order_and_comment_options_apply_to_detailed_rows() {
        let a = table(
            "CREATE TABLE a(id INT COMMENT 'old', label INT)",
            &["id", "label"],
        );
        let b = table(
            "CREATE TABLE b(label INT, id INT COMMENT 'new')",
            &["label", "id"],
        );
        let options = CompareOptions {
            ignore_comments: true,
            only_differences: true,
        };
        assert!(
            structure_details(
                crate::DatabaseKind::MySql,
                &a.schema,
                &b.schema,
                options,
                true,
                ""
            )
            .unwrap()
            .values
            .is_empty()
        );
        assert!(
            structure_details(
                crate::DatabaseKind::MySql,
                &a.schema,
                &b.schema,
                options,
                false,
                ""
            )
            .unwrap()
            .values
            .iter()
            .all(|row| row[2].as_deref() == Some("compare.reason.order"))
        );
    }
    #[test]
    fn adding_an_attribute_to_an_existing_column_is_a_modification() {
        let a = table("CREATE TABLE a(id INT DEFAULT 0)", &["id"]);
        let b = table("CREATE TABLE b(id INT)", &["id"]);
        let rows = structure_details(
            crate::DatabaseKind::MySql,
            &a.schema,
            &b.schema,
            CompareOptions {
                ignore_comments: false,
                only_differences: true,
            },
            true,
            "",
        )
        .unwrap();
        assert_eq!(rows.values.len(), 1);
        assert_eq!(rows.values[0][0].as_deref(), Some("≠"));
        assert_eq!(rows.values[0][2].as_deref(), Some("compare.default_value"));
    }
    #[test]
    fn ordinary_index_is_one_difference_and_has_executable_sync_sql() {
        use super::super::workbench::{Column, IndexColumn};
        let mut a = table("CREATE TABLE a(id INT, KEY idx_id(id))", &["id"]);
        let b = table("CREATE TABLE b(id INT)", &["id"]);
        a.schema.columns = vec![Column {
            name: "id".into(),
            data_type: "int".into(),
            ..Default::default()
        }];
        a.schema.indexes = Rows {
            columns: vec![
                "Index".into(),
                "Column".into(),
                "Non_unique".into(),
                "Type".into(),
                "Comment".into(),
            ],
            values: vec![vec![
                Some("idx_id".into()),
                Some("id".into()),
                Some("1".into()),
                Some("BTREE".into()),
                Some(String::new()),
            ]],
        };
        a.schema.index_columns.insert(
            "idx_id".into(),
            vec![IndexColumn {
                name: "id".into(),
                position: 1,
                descending: Some(false),
                nullable: Some(true),
                prefix: None,
            }],
        );
        let rows = structure_details(
            crate::DatabaseKind::MySql,
            &a.schema,
            &b.schema,
            CompareOptions {
                ignore_comments: false,
                only_differences: true,
            },
            true,
            "",
        )
        .unwrap();
        assert_eq!(rows.values.len(), 1, "{:?}", rows.values);
        assert_eq!(rows.values[0][2].as_deref(), Some("compare.reason.indexes"));
        let plan = sync_ddl(
            crate::DatabaseKind::MySql,
            "target",
            &a.object,
            &b.schema,
            &a.schema,
        )
        .unwrap();
        assert!(
            plan.sql
                .contains("CREATE INDEX `idx_id` ON `target`.`items` (`id`)"),
            "{}",
            plan.sql
        );
        assert!(!plan.incomplete, "{:?}", plan.issues);
    }
    #[test]
    fn comment_only_partial_draft_remains_visible_for_manual_review() {
        let mut draft = SyncDraft {
            sql: "-- MANUAL REVIEW: an unsupported server component remains.\n".into(),
            incomplete: true,
            issues: vec![],
        };
        omit_column_deletions(crate::DatabaseKind::MySql, &mut draft).unwrap();
        assert!(draft.sql.contains("MANUAL REVIEW"));
    }
}

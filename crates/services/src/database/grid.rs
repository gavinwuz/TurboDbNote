use super::*;
use std::collections::BTreeMap;
pub const DEFAULT_GRID_PAGE_SIZE: usize = 200;
pub const MAX_GRID_PAGE_SIZE: usize = 1000;
fn default_page_size() -> usize {
    DEFAULT_GRID_PAGE_SIZE
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PageOptions {
    pub offset: usize,
    #[serde(default = "default_page_size")]
    pub page_size: usize,
    pub after: Option<Vec<String>>,
    pub sort: Option<(String, bool)>,
    pub filter: Option<FilterSet>,
}
impl Default for PageOptions {
    fn default() -> Self {
        Self {
            offset: 0,
            page_size: DEFAULT_GRID_PAGE_SIZE,
            after: None,
            sort: None,
            filter: None,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Filter {
    pub column: String,
    pub operator: String,
    pub value: String,
}
pub const MAX_FILTER_CONDITIONS: usize = 20;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FilterJoin {
    #[default]
    And,
    Or,
}
impl FilterJoin {
    pub fn sql(self) -> &'static str {
        match self {
            Self::And => "AND",
            Self::Or => "OR",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FilterSet {
    Group {
        join: FilterJoin,
        conditions: Vec<Filter>,
    },
    Single(Filter),
}
impl FilterSet {
    pub fn conditions(&self) -> &[Filter] {
        match self {
            Self::Single(f) => std::slice::from_ref(f),
            Self::Group { conditions, .. } => conditions,
        }
    }
    pub fn join(&self) -> FilterJoin {
        match self {
            Self::Single(_) => FilterJoin::And,
            Self::Group { join, .. } => *join,
        }
    }
}
impl From<Filter> for FilterSet {
    fn from(value: Filter) -> Self {
        Self::Single(value)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Change {
    pub original: Option<Vec<Option<String>>>,
    // None deletes the original row. Empty map inserts database defaults.
    pub values: Option<BTreeMap<String, Option<String>>>,
}
fn column<'a>(s: &'a Schema, name: &str) -> Result<&'a Column, String> {
    s.columns
        .iter()
        .find(|c| c.name == name)
        .ok_or_else(|| "列不存在，请刷新表结构".into())
}
fn keys(s: &Schema) -> Vec<&Column> {
    s.columns.iter().filter(|c| c.primary).collect()
}
fn key_supported(c: &Column) -> bool {
    let ty = c.data_type.to_lowercase();
    !["blob", "binary", "bytea", "float", "double", "real"]
        .iter()
        .any(|t| ty.contains(t))
}
pub(in crate::database) fn conditions(
    kind: DatabaseKind,
    s: &Schema,
    filter: Option<&FilterSet>,
) -> Result<Vec<String>, String> {
    compile_conditions(kind, s, filter, false)
}
pub(in crate::database) fn readable_conditions(
    kind: DatabaseKind,
    s: &Schema,
    filter: Option<&FilterSet>,
) -> Result<Vec<String>, String> {
    compile_conditions(kind, s, filter, true)
}
fn compile_conditions(
    kind: DatabaseKind,
    s: &Schema,
    filter: Option<&FilterSet>,
    readable: bool,
) -> Result<Vec<String>, String> {
    let Some(filter) = filter else {
        return Ok(Vec::new());
    };
    let rows = filter.conditions();
    if rows.is_empty() || rows.len() > MAX_FILTER_CONDITIONS {
        return Err("筛选条件数量必须在 1–20 之间".into());
    }
    let parts = rows
        .iter()
        .map(|f| condition(kind, s, f, readable))
        .collect::<Result<Vec<_>, _>>()?;
    let group = if readable && parts.len() == 1 {
        parts[0].clone()
    } else {
        format!("({})", parts.join(&format!(" {} ", filter.join().sql())))
    };
    if group.len() > 60_000 {
        return Err("筛选条件过长，请精简条件".into());
    }
    Ok(vec![group])
}
fn condition(kind: DatabaseKind, s: &Schema, f: &Filter, readable: bool) -> Result<String, String> {
    let c = column(s, &f.column)?;
    let name = quote(kind, &c.name);
    if f.value.len() > 4096 || f.value.contains('\0') {
        return Err("筛选值过长或包含非法字符".into());
    }
    let value = if readable && !matches!(f.operator.as_str(), "null" | "not_null" | "contains") {
        if numeric_column(c) {
            let value = f.value.trim();
            if !valid_number(value) {
                return Err("grid.filter.number_invalid".into());
            }
            value.to_string()
        } else {
            copy_literal(kind, &f.value)
        }
    } else {
        literal_for(kind, &f.value)
    };
    if readable && matches!(f.operator.as_str(), "eq" | "ne" | "gt" | "ge" | "lt" | "le") {
        let op = match f.operator.as_str() {
            "eq" => "=",
            "ne" => "<>",
            "gt" => ">",
            "ge" => ">=",
            "lt" => "<",
            _ => "<=",
        };
        return Ok(format!("{name} {op} {value}"));
    }
    Ok(match f.operator.as_str() {
        "eq" => format!("{name}={value}"),
        "ne" => format!("{name}<>{value}"),
        "gt" => format!("{name}>{value}"),
        "ge" => format!("{name}>={value}"),
        "lt" => format!("{name}<{value}"),
        "le" => format!("{name}<={value}"),
        "null" => format!("{name} IS NULL"),
        "not_null" => format!("{name} IS NOT NULL"),
        "contains" => {
            let pattern = format!(
                "%{}%",
                f.value
                    .replace('!', "!!")
                    .replace('%', "!%")
                    .replace('_', "!_")
            );
            let value = if readable {
                copy_literal(kind, &pattern)
            } else {
                literal_for(kind, &pattern)
            };
            let escape = if readable && !f.value.contains(['!', '%', '_', '\\']) {
                ""
            } else {
                " ESCAPE '!'"
            };
            format!("{name} LIKE {value}{escape}")
        }
        _ => return Err("不支持的筛选操作".into()),
    })
}
fn where_sql(parts: &[String]) -> String {
    if parts.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", parts.join(" AND "))
    }
}
pub(super) async fn page(
    c: &mut impl Access,
    kind: DatabaseKind,
    version: &str,
    db: &str,
    object: &Object,
    options: &PageOptions,
) -> Result<Reply, String> {
    if !(1..=MAX_GRID_PAGE_SIZE).contains(&options.page_size) {
        return Err("每页行数必须在 1–1000 之间".into());
    }
    if options.offset > 10_000_000 {
        return Err("分页偏移过大，请增加筛选条件".into());
    }
    let schema = schema(c, kind, db, object, true).await?;
    let primary = keys(&schema);
    let keyset = c.supports_keyset()
        && !(kind == DatabaseKind::MySql && version.starts_with("4.0"))
        && options.sort.is_none()
        && !primary.is_empty()
        && primary.iter().all(|c| key_supported(c))
        && matches!(
            kind,
            DatabaseKind::MySql | DatabaseKind::PostgreSql | DatabaseKind::Sqlite
        );
    let mut predicates = conditions(kind, &schema, options.filter.as_ref())?;
    if keyset && let Some(after) = &options.after {
        if after.len() != primary.len() || after.iter().any(|v| v.len() > 4096 || v.contains('\0'))
        {
            return Err("分页游标无效，请刷新".into());
        }
        let mut disjunction = Vec::new();
        for i in 0..primary.len() {
            let mut prefix = (0..i)
                .map(|j| {
                    format!(
                        "{}={}",
                        quote(kind, &primary[j].name),
                        literal_for(kind, &after[j])
                    )
                })
                .collect::<Vec<_>>();
            prefix.push(format!(
                "{}>{}",
                quote(kind, &primary[i].name),
                literal_for(kind, &after[i])
            ));
            disjunction.push(format!("({})", prefix.join(" AND ")));
        }
        predicates.push(format!("({})", disjunction.join(" OR ")));
    }
    let mut order = Vec::new();
    if let Some((name, desc)) = &options.sort {
        column(&schema, name)?;
        order.push(format!(
            "{} {}",
            quote(kind, name),
            if *desc { "DESC" } else { "ASC" }
        ));
    }
    order.extend(
        primary
            .iter()
            .filter(|c| {
                options
                    .sort
                    .as_ref()
                    .is_none_or(|(name, _)| *name != c.name)
            })
            .map(|c| quote(kind, &c.name)),
    );
    let sql = format!(
        "SELECT * FROM {}{}{} LIMIT {}{}",
        qualified(kind, db, object),
        where_sql(&predicates),
        if order.is_empty() {
            String::new()
        } else {
            format!(" ORDER BY {}", order.join(","))
        },
        options.page_size + 1,
        if keyset && options.after.is_some() {
            String::new()
        } else {
            format!(" OFFSET {}", options.offset)
        }
    );
    let started = std::time::Instant::now();
    let mut rows = c.query_data(&sql).await?;
    let elapsed_ms = started.elapsed().as_millis() as u64;
    if rows.columns.is_empty() {
        rows.columns = schema.columns.iter().map(|c| c.name.clone()).collect();
    }
    let more = rows.values.len() > options.page_size;
    rows.values.truncate(options.page_size);
    let next = if keyset {
        rows.values.last().and_then(|row| {
            primary
                .iter()
                .map(|c| {
                    rows.columns
                        .iter()
                        .position(|name| *name == c.name)
                        .and_then(|i| row.get(i))
                        .cloned()
                        .flatten()
                })
                .collect::<Option<Vec<_>>>()
        })
    } else {
        None
    };
    Ok(Reply::GridPage {
        rows,
        schema,
        offset: options.offset,
        more,
        sql,
        next,
        elapsed_ms,
        keyset: keyset && (options.offset == 0 || options.after.is_some()),
    })
}
pub(super) async fn count(
    c: &mut impl Access,
    kind: DatabaseKind,
    db: &str,
    object: &Object,
    filter: Option<&FilterSet>,
) -> Result<Reply, String> {
    let s = schema(c, kind, db, object, false).await?;
    let sql = format!(
        "SELECT COUNT(*) FROM {}{}",
        qualified(kind, db, object),
        where_sql(&conditions(kind, &s, filter)?)
    );
    let rows = c.query(&sql).await?;
    let count = rows
        .values
        .first()
        .and_then(|r| r.first())
        .and_then(|v| v.as_ref())
        .and_then(|v| v.parse().ok())
        .ok_or("行数统计结果无效")?;
    Ok(Reply::Count(count))
}
fn identity(kind: DatabaseKind, s: &Schema, row: &[Option<String>]) -> Result<String, String> {
    if row.len() != s.columns.len() {
        return Err("行数据与表结构不一致".into());
    }
    let primary = keys(s);
    if primary.is_empty() || primary.iter().any(|c| !key_supported(c)) {
        return Err("修改或删除需要可可靠定位的主键".into());
    }
    Ok(s.columns
        .iter()
        .enumerate()
        .filter(|(_, c)| c.primary)
        .map(|(i, c)| match &row[i] {
            Some(v) => format!("{}={}", quote(kind, &c.name), literal_for(kind, v)),
            None => format!("{} IS NULL", quote(kind, &c.name)),
        })
        .collect::<Vec<_>>()
        .join(" AND "))
}
pub fn edit_preview(
    kind: DatabaseKind,
    db: &str,
    object: &Object,
    s: &Schema,
    changes: &[Change],
) -> Result<String, String> {
    if object.kind != "table"
        || !matches!(
            kind,
            DatabaseKind::MySql | DatabaseKind::PostgreSql | DatabaseKind::Sqlite
        )
    {
        return Err("当前对象不支持事务编辑".into());
    }
    if changes.is_empty() || changes.len() > 100 {
        return Err("每次提交 1–100 行修改".into());
    }
    let name = qualified(kind, db, object);
    let mut statements = Vec::new();
    for change in changes {
        let predicate = change
            .original
            .as_ref()
            .map(|row| identity(kind, s, row))
            .transpose()?;
        let sql = match (&change.values, predicate) {
            (None, Some(predicate)) => format!("DELETE FROM {name} WHERE {predicate}"),
            (Some(values), predicate) => {
                let mut columns = Vec::new();
                let mut literals = Vec::new();
                for (name, value) in values {
                    let col = column(s, name)?;
                    if !key_supported(col)
                        && ["blob", "binary", "bytea"]
                            .iter()
                            .any(|t| col.data_type.to_lowercase().contains(t))
                    {
                        return Err("二进制列暂不支持文本编辑".into());
                    }
                    if value.as_ref().is_some_and(|v| v.contains('\0')) {
                        return Err("编辑值不能包含 NUL 字符".into());
                    }
                    columns.push(quote(kind, name));
                    literals.push(
                        value
                            .as_ref()
                            .map(|v| literal_for(kind, v))
                            .unwrap_or_else(|| "NULL".into()),
                    );
                }
                if let Some(predicate) = predicate {
                    if columns.is_empty() {
                        continue;
                    }
                    format!(
                        "UPDATE {name} SET {} WHERE {predicate}",
                        columns
                            .iter()
                            .zip(literals)
                            .map(|(c, v)| format!("{c}={v}"))
                            .collect::<Vec<_>>()
                            .join(",")
                    )
                } else if columns.is_empty() {
                    if kind == DatabaseKind::MySql {
                        format!("INSERT INTO {name} () VALUES ()")
                    } else {
                        format!("INSERT INTO {name} DEFAULT VALUES")
                    }
                } else {
                    format!(
                        "INSERT INTO {name} ({}) VALUES ({})",
                        columns.join(","),
                        literals.join(",")
                    )
                }
            }
            _ => return Err("删除目标缺失".into()),
        };
        statements.push(sql);
    }
    let sql = statements.join(";\n");
    if sql.len() > 60_000 {
        return Err("修改超过 60 KB，请减少本次修改量".into());
    }
    Ok(sql)
}
pub(super) async fn commit(
    c: &mut impl Access,
    kind: DatabaseKind,
    db: &str,
    object: &Object,
    ddl: &str,
    changes: &[Change],
) -> Result<Reply, String> {
    commit_columns(c, kind, db, object, (ddl, changes), None).await
}
pub(in crate::database) async fn commit_columns(
    c: &mut impl Access,
    kind: DatabaseKind,
    db: &str,
    object: &Object,
    edit: (&str, &[Change]),
    columns: Option<&[String]>,
) -> Result<Reply, String> {
    let (ddl, changes) = edit;
    let mut s = schema(c, kind, db, object, true).await?;
    if let Some(columns) = columns {
        if columns.len() != s.columns.len() && changes.iter().any(|c| c.values.is_none()) {
            return Err("删除查询结果行需要包含表的全部列，请使用完整列查询".into());
        }
        if !s.columns.iter().any(|c| c.primary)
            || s.columns
                .iter()
                .filter(|c| c.primary)
                .any(|c| !columns.contains(&c.name))
        {
            return Err("查询结果必须包含完整主键才能编辑".into());
        }
        let mut seen = std::collections::HashSet::new();
        s.columns = columns
            .iter()
            .map(|name| {
                if !seen.insert(name) {
                    return Err("查询结果包含重复列".to_string());
                }
                s.columns
                    .iter()
                    .find(|c| &c.name == name)
                    .cloned()
                    .ok_or_else(|| "查询列与表结构不匹配".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
    }
    if s.ddl != ddl {
        return Err("表结构已变化，请刷新后重新编辑".into());
    }
    edit_preview(kind, db, object, &s, changes)?;
    if kind == DatabaseKind::MySql && !s.ddl.to_lowercase().contains("engine=innodb") {
        return Err("批量编辑需要 InnoDB 事务表".into());
    }
    c.query(if kind == DatabaseKind::Sqlite {
        "BEGIN IMMEDIATE"
    } else {
        "BEGIN"
    })
    .await?;
    let result = async {
        c.query(&format!(
            "SELECT * FROM {} WHERE 1=0",
            qualified(kind, db, object)
        ))
        .await?;
        if schema(c, kind, db, object, true).await?.ddl != ddl {
            return Err("表结构已变化，已取消提交".into());
        }
        for change in changes {
            if let Some(original) = &change.original {
                let sql = format!(
                    "SELECT {} FROM {} WHERE {}{}",
                    s.columns
                        .iter()
                        .map(|c| quote(kind, &c.name))
                        .collect::<Vec<_>>()
                        .join(","),
                    qualified(kind, db, object),
                    identity(kind, &s, original)?,
                    if kind == DatabaseKind::Sqlite {
                        ""
                    } else {
                        " FOR UPDATE"
                    }
                );
                let current = c.query(&sql).await?;
                if current.values.len() != 1 || &current.values[0] != original {
                    return Err("数据已被其他操作修改，已取消提交；请刷新后重试".into());
                }
            }
            let sql = edit_preview(kind, db, object, &s, std::slice::from_ref(change))?;
            if !sql.is_empty() {
                c.query(&sql).await?;
            }
        }
        c.query("COMMIT").await?;
        Ok(Reply::Executed)
    }
    .await;
    if result.is_err() && c.query("ROLLBACK").await.is_err() {
        return Err("提交失败且回滚状态未知，请重新连接并核对数据".into());
    }
    result
}
fn numeric_column(column: &Column) -> bool {
    let data_type = column.data_type.to_ascii_lowercase();
    let base = data_type
        .strip_prefix("nullable(")
        .unwrap_or(&data_type)
        .split(['(', ' '])
        .next()
        .unwrap_or("");
    matches!(
        base,
        "tinyint"
            | "smallint"
            | "mediumint"
            | "bigint"
            | "int"
            | "integer"
            | "decimal"
            | "numeric"
            | "float"
            | "double"
            | "real"
            | "serial"
            | "bigserial"
            | "smallserial"
            | "number"
    ) || base
        .strip_prefix("uint")
        .or_else(|| base.strip_prefix("int"))
        .or_else(|| base.strip_prefix("float"))
        .is_some_and(|bits| matches!(bits, "8" | "16" | "32" | "64" | "128" | "256"))
}
fn valid_number(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || b"+-.eE".contains(&b))
        && value.parse::<f64>().is_ok_and(|n| n.is_finite())
}
/// Human-readable copy only; execution retains the mode-independent encoder.
fn copy_literal(kind: DatabaseKind, value: &str) -> String {
    if kind == DatabaseKind::MySql && !value.chars().any(|c| c == '\\' || c.is_control()) {
        format!("'{}'", value.replace('\'', "''"))
    } else {
        literal_for(kind, value)
    }
}
pub fn copy_row(
    kind: DatabaseKind,
    db: &str,
    object: &Object,
    rows: &Rows,
    index: usize,
    format: &str,
) -> Result<String, String> {
    let row = rows.values.get(index).ok_or("未选择数据行")?;
    match format {
        "csv" => export_csv(&Rows {
            columns: rows.columns.clone(),
            values: vec![row.clone()],
        }),
        "json"
            if rows
                .columns
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                != rows.columns.len() =>
        {
            serde_json::to_string_pretty(&serde_json::json!({"columns":rows.columns,"values":row}))
                .map_err(|_| "复制失败".into())
        }
        "json" => {
            serde_json::to_string_pretty(&rows.columns.iter().zip(row).collect::<BTreeMap<_, _>>())
                .map_err(|_| "复制失败".into())
        }
        "sql" | "insert" => Ok(format!(
            "INSERT INTO {} ({}) VALUES ({});",
            qualified(kind, db, object),
            rows.columns
                .iter()
                .map(|c| quote(kind, c))
                .collect::<Vec<_>>()
                .join(","),
            row.iter()
                .map(|v| v
                    .as_ref()
                    .map(|v| copy_literal(kind, v))
                    .unwrap_or_else(|| "NULL".into()))
                .collect::<Vec<_>>()
                .join(",")
        )),
        "md" => {
            let escape = |s: &str| s.replace('|', "\\|").replace('\n', "<br>");
            Ok(format!(
                "| {} |\n| {} |\n| {} |",
                rows.columns
                    .iter()
                    .map(|s| escape(s))
                    .collect::<Vec<_>>()
                    .join(" | "),
                vec!["---"; rows.columns.len()].join(" | "),
                row.iter()
                    .map(|v| escape(v.as_deref().unwrap_or("NULL")))
                    .collect::<Vec<_>>()
                    .join(" | ")
            ))
        }
        _ => Ok(row
            .iter()
            .map(|v| v.as_deref().unwrap_or("NULL"))
            .collect::<Vec<_>>()
            .join("\t")),
    }
}

/// Copy only. Use original complete primary keys so edits cannot retarget the predicate.
pub fn copy_update(
    kind: DatabaseKind,
    db: &str,
    object: &Object,
    schema: &Schema,
    rows: &Rows,
    original: &Rows,
    index: usize,
) -> Result<String, String> {
    let row = rows
        .values
        .get(index)
        .ok_or("grid.copy.update_requires_key")?;
    let before = original
        .values
        .get(index)
        .ok_or("grid.copy.update_requires_key")?;
    let primary = keys(schema);
    if primary.is_empty()
        || row.len() != rows.columns.len()
        || before.len() != original.columns.len()
        || rows.columns.is_empty()
        || rows
            .columns
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len()
            != rows.columns.len()
    {
        return Err("grid.copy.update_requires_key".into());
    }
    if rows
        .columns
        .iter()
        .any(|name| !schema.columns.iter().any(|column| &column.name == name))
    {
        return Err("grid.copy.update_requires_key".into());
    }
    let mut predicates = Vec::new();
    for column in primary {
        let position = original
            .columns
            .iter()
            .position(|name| name == &column.name)
            .ok_or("grid.copy.update_requires_key")?;
        let value = before[position]
            .as_ref()
            .ok_or("grid.copy.update_requires_key")?;
        predicates.push(format!(
            "{}={}",
            quote(kind, &column.name),
            copy_literal(kind, value)
        ));
    }
    let assignments = rows
        .columns
        .iter()
        .zip(row)
        .map(|(name, value)| {
            format!(
                "{}={}",
                quote(kind, name),
                value
                    .as_ref()
                    .map(|v| copy_literal(kind, v))
                    .unwrap_or_else(|| "NULL".into())
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    Ok(format!(
        "UPDATE {} SET {} WHERE {};",
        qualified(kind, db, object),
        assignments,
        predicates.join(" AND ")
    ))
}

/// Copy-only predicate for a single cell; quoting follows the selected driver.
pub fn cell_where(
    kind: DatabaseKind,
    column: &Column,
    value: Option<&str>,
) -> Result<String, String> {
    let name = quote(kind, &column.name);
    let Some(value) = value else {
        return Ok(format!("WHERE {name} IS NULL"));
    };
    if ["binary", "blob", "bytea"]
        .iter()
        .any(|ty| column.data_type.to_lowercase().contains(ty))
    {
        return Err("二进制预览不能生成无损 WHERE 条件".into());
    }
    if value.len() > 30_000 || value.contains('\0') {
        return Err("值过长或含有零字符，请使用主键条件".into());
    }
    let numeric = numeric_column(column);
    if numeric {
        if valid_number(value) {
            return Ok(format!("WHERE {name}= {value}"));
        }
        return Err("数值内容无效，无法生成 WHERE 条件".into());
    }
    // Ordinary text stays readable. Backslashes/control characters retain the
    // mode-independent expression because MySQL escaping depends on sql_mode.
    let literal = if kind == DatabaseKind::MySql
        && !value.contains('\\')
        && !value.chars().any(char::is_control)
    {
        format!("'{}'", value.replace('\'', "''"))
    } else {
        literal_for(kind, value)
    };
    Ok(format!("WHERE {name}={literal}"))
}

#[cfg(test)]
mod copy_literal_tests {
    use super::*;
    #[test]
    fn copied_update_uses_original_key_and_preserves_other_rows() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE items(id INTEGER PRIMARY KEY,name TEXT);INSERT INTO items VALUES(1,'before'),(2,'untouched');").unwrap();
        let schema = Schema {
            columns: vec![
                Column {
                    name: "id".into(),
                    data_type: "INTEGER".into(),
                    primary: true,
                    comment: String::new(),
                    ..Default::default()
                },
                Column {
                    name: "name".into(),
                    data_type: "TEXT".into(),
                    primary: false,
                    comment: String::new(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let object = Object {
            schema: String::new(),
            name: "items".into(),
            kind: "table".into(),
            comment: String::new(),
            parent: String::new(),
        };
        let original = Rows {
            columns: vec!["id".into(), "name".into()],
            values: vec![vec![Some("1".into()), Some("before".into())]],
        };
        let changed = Rows {
            columns: original.columns.clone(),
            values: vec![vec![Some("3".into()), Some("魔域'先锋".into())]],
        };
        let sql = copy_update(
            DatabaseKind::Sqlite,
            "main",
            &object,
            &schema,
            &changed,
            &original,
            0,
        )
        .unwrap();
        db.execute_batch(&sql).unwrap();
        assert_eq!(
            db.query_row("SELECT name FROM items WHERE id=3", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "魔域'先锋"
        );
        assert_eq!(
            db.query_row("SELECT name FROM items WHERE id=2", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "untouched"
        );
        assert!(
            copy_update(
                DatabaseKind::Sqlite,
                "main",
                &object,
                &Schema::default(),
                &changed,
                &original,
                0
            )
            .is_err()
        );
        assert!(
            copy_update(
                DatabaseKind::Sqlite,
                "main",
                &object,
                &schema,
                &changed,
                &Rows::default(),
                0
            )
            .is_err()
        );
    }
    #[test]
    fn readable_copy_preserves_quotes_and_safe_fallback() {
        assert_eq!(copy_literal(DatabaseKind::MySql, "魔域先锋"), "'魔域先锋'");
        assert_eq!(copy_literal(DatabaseKind::MySql, "538"), "'538'");
        assert_eq!(copy_literal(DatabaseKind::MySql, "a'b"), "'a''b'");
        for value in ["a\\b", "a\nb", "a\0b"] {
            assert_eq!(
                copy_literal(DatabaseKind::MySql, value),
                literal_for(DatabaseKind::MySql, value)
            );
        }
    }
}

#[cfg(test)]
mod readable_filter_tests {
    use super::*;
    #[test]
    fn generated_filters_are_typed_readable_and_keep_literal_wildcards() {
        let schema = Schema {
            columns: vec![
                Column {
                    name: "price".into(),
                    data_type: "decimal(10,2)".into(),
                    ..Default::default()
                },
                Column {
                    name: "name".into(),
                    data_type: "varchar(80)".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let condition = |column: &str, operator: &str, value: &str| Filter {
            column: column.into(),
            operator: operator.into(),
            value: value.into(),
        };
        assert_eq!(
            super::condition(
                DatabaseKind::MySql,
                &schema,
                &condition("price", "gt", "1"),
                true
            )
            .unwrap(),
            "`price` > 1"
        );
        assert_eq!(
            super::condition(
                DatabaseKind::MySql,
                &schema,
                &condition("name", "contains", "S"),
                true
            )
            .unwrap(),
            "`name` LIKE '%S%'"
        );
        assert!(
            super::condition(
                DatabaseKind::MySql,
                &schema,
                &condition("price", "eq", "1 OR 1=1"),
                true
            )
            .is_err()
        );
        let wildcard = super::condition(
            DatabaseKind::MySql,
            &schema,
            &condition("name", "contains", "a%_!"),
            true,
        )
        .unwrap();
        assert!(wildcard.contains("'%a!%!_!!%' ESCAPE '!'"));
        let special = super::condition(
            DatabaseKind::MySql,
            &schema,
            &condition("name", "eq", "a\\b"),
            true,
        )
        .unwrap();
        assert!(special.contains("CONVERT("));
        let ordinary = super::condition(
            DatabaseKind::MySql,
            &schema,
            &condition("name", "eq", "O'Reilly"),
            true,
        )
        .unwrap();
        assert!(ordinary.contains("'O''Reilly'"));
    }
}

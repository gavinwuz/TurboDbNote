use super::*;
use workbench::{Change, Column, FilterSet, PageOptions, Reply, Schema};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GridSource {
    pub object: Option<Object>,
    pub schema: Schema,
    pub editable: bool,
    pub complete_row: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum GridAction {
    Page {
        options: PageOptions,
        columns: Vec<String>,
    },
    Count {
        filter: Option<FilterSet>,
        columns: Vec<String>,
    },
    Commit {
        ddl: String,
        columns: Vec<String>,
        changes: Vec<Change>,
    },
}
pub(super) fn readonly(columns: &[String]) -> GridSource {
    GridSource {
        object: None,
        schema: Schema {
            columns: columns
                .iter()
                .map(|name| Column {
                    name: name.clone(),
                    data_type: "unknown".into(),
                    comment: String::new(),
                    primary: false,
                })
                .collect(),
            ..Default::default()
        },
        editable: false,
        complete_row: false,
    }
}
fn simple_select(stmt: &Statement) -> Option<(&ast::Select, &ast::ObjectName)> {
    let Statement::Query(q) = stmt else {
        return None;
    };
    if q.with.is_some() || !q.pipe_operators.is_empty() {
        return None;
    }
    let ast::SetExpr::Select(s) = q.body.as_ref() else {
        return None;
    };
    if s.from.len() != 1
        || !s.from[0].joins.is_empty()
        || s.distinct.is_some()
        || s.having.is_some()
        || s.qualify.is_some()
        || s.top.is_some()
        || !s.lateral_views.is_empty()
        || !s.named_window.is_empty()
        || !s.connect_by.is_empty()
        || !matches!(&s.group_by, ast::GroupByExpr::Expressions(exprs, modifiers) if exprs.is_empty() && modifiers.is_empty())
        || s.projection.iter().any(|p| {
            !matches!(
                p,
                ast::SelectItem::Wildcard(_)
                    | ast::SelectItem::QualifiedWildcard(_, _)
                    | ast::SelectItem::UnnamedExpr(
                        Expr::Identifier(_) | Expr::CompoundIdentifier(_)
                    )
            )
        })
    {
        return None;
    }
    let ast::TableFactor::Table {
        name,
        args: None,
        alias,
        version: None,
        json_path: None,
        sample: None,
        with_ordinality: false,
        ..
    } = &s.from[0].relation
    else {
        return None;
    };
    if alias.as_ref().is_some_and(|a| !a.columns.is_empty()) {
        return None;
    }
    if s.projection.iter().any(|p| match p {
        ast::SelectItem::Wildcard(options) => !options.to_string().is_empty(),
        ast::SelectItem::QualifiedWildcard(
            ast::SelectItemQualifiedWildcardKind::ObjectName(_),
            options,
        ) => !options.to_string().is_empty(),
        ast::SelectItem::QualifiedWildcard(_, _) => true,
        _ => false,
    }) {
        return None;
    }
    Some((s, name))
}
fn identifier(kind: DatabaseKind, id: &ast::Ident) -> String {
    if kind == DatabaseKind::PostgreSql && id.quote_style.is_none() {
        id.value.to_lowercase()
    } else {
        id.value.clone()
    }
}
pub(super) async fn source_info(
    c: &mut impl Access,
    kind: DatabaseKind,
    database: &str,
    sql: &str,
    columns: &[String],
) -> Result<GridSource, String> {
    let stmt = parse(kind, sql)?;
    let Some((select, name)) = simple_select(&stmt) else {
        return Ok(readonly(columns));
    };
    // A subquery in WHERE must not silently broaden an editable query's ownership.
    if inspect(&stmt, kind)?.tables.len() != 1 {
        return Ok(readonly(columns));
    }
    let parts = name
        .0
        .iter()
        .map(|part| match part {
            ast::ObjectNamePart::Identifier(id) => Ok(identifier(kind, id)),
            _ => Err("表名无法定位"),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let alias = match &select.from[0].relation {
        ast::TableFactor::Table { alias, .. } => alias.as_ref().map(|a| identifier(kind, &a.name)),
        _ => None,
    };
    let qualifier_ok = |qualifier: &[String]| {
        qualifier == parts
            || qualifier == &parts[parts.len() - 1..]
            || alias.as_ref().is_some_and(|a| qualifier == [a.clone()])
    };
    for item in &select.projection {
        let qualifier = match item {
            ast::SelectItem::UnnamedExpr(Expr::CompoundIdentifier(ids)) => Some(
                ids[..ids.len().saturating_sub(1)]
                    .iter()
                    .map(|i| identifier(kind, i))
                    .collect::<Vec<_>>(),
            ),
            ast::SelectItem::QualifiedWildcard(
                ast::SelectItemQualifiedWildcardKind::ObjectName(name),
                _,
            ) => Some(
                name.0
                    .iter()
                    .filter_map(|p| match p {
                        ast::ObjectNamePart::Identifier(id) => Some(identifier(kind, id)),
                        _ => None,
                    })
                    .collect(),
            ),
            _ => None,
        };
        if qualifier.as_ref().is_some_and(|q| !qualifier_ok(q)) {
            return Ok(readonly(columns));
        }
    }
    let object = resolved_object(c, kind, database, &parts).await?;
    let mut schema = workbench::schema(c, kind, database, &object, true).await?;
    let mut expected = Vec::new();
    for item in &select.projection {
        match item {
            ast::SelectItem::Wildcard(_) | ast::SelectItem::QualifiedWildcard(_, _) => {
                expected.extend(schema.columns.iter().map(|c| c.name.clone()))
            }
            ast::SelectItem::UnnamedExpr(Expr::Identifier(id)) => {
                expected.push(identifier(kind, id))
            }
            ast::SelectItem::UnnamedExpr(Expr::CompoundIdentifier(ids)) => {
                expected.push(identifier(kind, ids.last().ok_or("字段名无效")?))
            }
            _ => return Ok(readonly(columns)),
        }
    }
    if (!columns.is_empty() && columns != expected)
        || expected.iter().collect::<BTreeSet<_>>().len() != expected.len()
    {
        return Ok(readonly(columns));
    }
    let editable = schema.columns.iter().any(|c| c.primary)
        && schema
            .columns
            .iter()
            .filter(|c| c.primary)
            .all(|c| expected.contains(&c.name));
    let complete_row = expected.len() == schema.columns.len();
    schema.columns = expected
        .iter()
        .map(|name| {
            schema
                .columns
                .iter()
                .find(|c| &c.name == name)
                .cloned()
                .ok_or_else(|| "查询列无法映射回原表".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(GridSource {
        object: Some(object),
        schema,
        editable,
        complete_row,
    })
}
fn filtered_sql(
    kind: DatabaseKind,
    sql: &str,
    source: &GridSource,
    options: &PageOptions,
) -> Result<String, String> {
    if options.filter.is_none() && options.sort.is_none() {
        return Ok(sql.into());
    }
    if source.object.is_none() {
        return Err("关联、聚合或计算结果仅支持只读浏览".into());
    }
    let predicates = workbench::grid::conditions(kind, &source.schema, options.filter.as_ref())?;
    let condition = if predicates.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", predicates.join(" AND "))
    };
    let mut order = Vec::new();
    if let Some((name, desc)) = &options.sort {
        if !source.schema.columns.iter().any(|c| &c.name == name) {
            return Err("排序列不存在".into());
        }
        order.push(format!(
            "{} {}",
            workbench::quote(kind, name),
            if *desc { "DESC" } else { "ASC" }
        ));
    } else if let Statement::Query(q) = parse(kind, sql)?
        && let Some(order_by) = q.order_by
    {
        let ast::OrderByKind::Expressions(expressions) = order_by.kind else {
            return Err("请明确选择结果列排序".into());
        };
        for expr in expressions {
            let name = match expr.expr {
                Expr::Identifier(id) => identifier(kind, &id),
                Expr::CompoundIdentifier(ids) => identifier(kind, ids.last().ok_or("排序列无效")?),
                _ => return Err("当前表达式排序不支持附加筛选，请先选择结果列排序".into()),
            };
            if !source.schema.columns.iter().any(|c| c.name == name) {
                return Err("原排序列未包含在查询结果中，请先选择结果列排序".into());
            }
            if matches!(expr.options.sort, Some(ast::OrderBySort::Using(_))) {
                return Err("当前自定义排序不支持附加筛选".into());
            }
            order.push(format!(
                "{} {}{}",
                workbench::quote(kind, &name),
                if matches!(expr.options.sort, Some(ast::OrderBySort::Desc)) {
                    "DESC"
                } else {
                    "ASC"
                },
                match expr.options.nulls_first {
                    Some(true) => " NULLS FIRST",
                    Some(false) => " NULLS LAST",
                    None => "",
                }
            ));
        }
    }
    let order = if order.is_empty() {
        String::new()
    } else {
        format!(" ORDER BY {}", order.join(","))
    };
    Ok(format!(
        "SELECT * FROM ({}\n) AS __note_result{condition}{order}",
        statement_body(kind, sql)?
    ))
}
pub async fn grid_request(
    c: &mut impl Access,
    kind: DatabaseKind,
    database: &str,
    version: &str,
    plan: &Plan,
    confirmed: bool,
    action: &GridAction,
) -> Result<Reply, String> {
    if !plan.read_only || inspect(&parse(kind, &plan.sql)?, kind)?.writes {
        return Err("非查询语句不能作为结果表格重放".into());
    }
    let columns = match action {
        GridAction::Page { columns, .. }
        | GridAction::Count { columns, .. }
        | GridAction::Commit { columns, .. } => columns,
    };
    let source = source_info(c, kind, database, &plan.sql, columns)
        .await
        .unwrap_or_else(|_| readonly(columns));
    match action {
        GridAction::Commit {
            ddl,
            columns,
            changes,
        } => {
            if !confirmed || !source.editable {
                return Err("此查询结果不可写入或尚未确认".into());
            }
            if kind == DatabaseKind::MySql && !crate::mysql::modern_version(version) {
                return Err("旧 MySQL 查询结果暂不支持编辑".into());
            }
            workbench::grid::commit_columns(
                c,
                kind,
                database,
                source.object.as_ref().ok_or("结果无法定位原表")?,
                (ddl, changes),
                Some(columns),
            )
            .await
        }
        GridAction::Page { options, .. } => {
            let sql = filtered_sql(kind, &plan.sql, &source, options)?;
            if sql != plan.sql && kind == DatabaseKind::MySql && version.starts_with("4.0") {
                return Err("MySQL 4.0 不支持派生表筛选".into());
            }
            let scoped = Plan {
                sql: sql.clone(),
                ..plan.clone()
            };
            let mut page = execute(
                c,
                kind,
                database,
                version,
                &scoped,
                confirmed,
                (options.offset, options.page_size),
            )
            .await?;
            if page.rows.columns.is_empty() {
                page.rows.columns = source
                    .schema
                    .columns
                    .iter()
                    .map(|c| c.name.clone())
                    .collect();
            }
            Ok(Reply::GridPage {
                rows: page.rows,
                schema: source.schema,
                offset: page.offset,
                more: page.more,
                sql,
                next: None,
                elapsed_ms: page.elapsed_ms,
                keyset: false,
            })
        }
        GridAction::Count { filter, .. } => {
            if kind == DatabaseKind::MySql && version.starts_with("4.0") {
                return Err("MySQL 4.0 不支持派生查询统计".into());
            }
            let options = PageOptions {
                filter: filter.clone(),
                ..Default::default()
            };
            let sql = filtered_sql(kind, &plan.sql, &source, &options)?;
            let scoped = Plan {
                sql: format!(
                    "SELECT COUNT(*) FROM ({}\n) AS __note_count",
                    statement_body(kind, &sql)?
                ),
                ..plan.clone()
            };
            let page = execute(c, kind, database, version, &scoped, confirmed, (0, 1)).await?;
            let count = page
                .rows
                .values
                .first()
                .and_then(|r| r.first())
                .and_then(|v| v.as_ref())
                .and_then(|v| v.parse().ok())
                .ok_or("查询行数无效")?;
            Ok(Reply::Count(count))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn renamed_table_columns_and_composite_wildcards_are_not_editable_sources() {
        for sql in [
            "SELECT x.id,x.name FROM t AS x(name,id)",
            "SELECT (payload).* FROM t",
        ] {
            if let Ok(statement) = parse(DatabaseKind::PostgreSql, sql) {
                assert!(simple_select(&statement).is_none());
            }
        }
    }
    #[test]
    fn sqlite_projected_edits_filters_and_counts_keep_the_original_query_scope() {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            use sqlx::Connection;
            let mut db=sqlx::SqliteConnection::connect("sqlite::memory:").await.unwrap();
            sqlx::raw_sql("CREATE TABLE items(id INTEGER PRIMARY KEY,name TEXT,hidden TEXT DEFAULT 'kept'); WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<45) INSERT INTO items(id,name) SELECT i,'row '||i FROM n;").execute(&mut db).await.unwrap();
            let kind=DatabaseKind::Sqlite;
            let plan=prepare(&mut db,kind,"main","3","SELECT name,id FROM items WHERE id>5 ORDER BY id DESC LIMIT 25;").await.unwrap();
            let first=execute(&mut db,kind,"main","3",&plan,false,(0,20)).await.unwrap();
            let meta=first.grid.unwrap();
            assert!(meta.editable);
            assert_eq!(meta.schema.columns[1].name,"id");
            let columns=first.rows.columns;
            let filter=Some(workbench::Filter{column:"id".into(),operator:"lt".into(),value:"40".into()}.into());
            let options=PageOptions{filter:filter.clone(),sort:Some(("id".into(),false)),page_size:20,..Default::default()};
            let Reply::GridPage{rows,more,..}=grid_request(&mut db,kind,"main","3",&plan,false,&GridAction::Page{columns:columns.clone(),options}).await.unwrap() else {panic!("grid expected")};
            assert_eq!(rows.values.len(),19);assert!(!more);assert_eq!(rows.values[0][1].as_deref(),Some("21"));
            assert!(matches!(grid_request(&mut db,kind,"main","3",&plan,false,&GridAction::Count{columns:columns.clone(),filter}).await.unwrap(),Reply::Count(19)));
            let empty_options=PageOptions{filter:Some(workbench::Filter{column:"id".into(),operator:"eq".into(),value:"999".into()}.into()),..Default::default()};
            let Reply::GridPage{rows:empty,..}=grid_request(&mut db,kind,"main","3",&plan,false,&GridAction::Page{columns:columns.clone(),options:empty_options}).await.unwrap() else {panic!("grid expected")};
            assert!(empty.values.is_empty()); assert_eq!(empty.columns,columns);
            let change=Change{original:Some(rows.values[0].clone()),values:Some(std::collections::BTreeMap::from([("name".into(),Some("edited".into()))]))};
            let commit=GridAction::Commit{columns,ddl:meta.schema.ddl,changes:vec![change]};
            assert!(grid_request(&mut db,kind,"main","3",&plan,false,&commit).await.is_err());
            assert!(matches!(grid_request(&mut db,kind,"main","3",&plan,true,&commit).await.unwrap(),Reply::Executed));
            let current=workbench::Access::query(&mut db,"SELECT name,hidden FROM items WHERE id=21").await.unwrap();
            assert_eq!(current.values[0],vec![Some("edited".into()),Some("kept".into())]);
            assert!(grid_request(&mut db,kind,"main","3",&plan,true,&commit).await.is_err());
            sqlx::query("ALTER TABLE items ADD COLUMN extra TEXT").execute(&mut db).await.unwrap();
            assert!(grid_request(&mut db,kind,"main","3",&plan,true,&commit).await.is_err());
        });
    }
    #[test]
    fn joins_aggregates_aliases_and_missing_keys_never_enable_result_writes() {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            use sqlx::Connection;
            let mut db=sqlx::SqliteConnection::connect("sqlite::memory:").await.unwrap();
            sqlx::raw_sql("CREATE TABLE t(id INTEGER PRIMARY KEY,name TEXT); INSERT INTO t VALUES(1,'one');").execute(&mut db).await.unwrap();
            for sql in ["SELECT a.id,a.name FROM t a JOIN t b ON a.id=b.id", "SELECT count(*) AS id FROM t", "SELECT DISTINCT id,name FROM t", "SELECT name FROM t", "SELECT id AS other,name FROM t", "SELECT id,id FROM t"] {
                let plan=prepare(&mut db,DatabaseKind::Sqlite,"main","3",sql).await.unwrap();
                let result=execute(&mut db,DatabaseKind::Sqlite,"main","3",&plan,false,(0,20)).await.unwrap();
                assert!(!result.grid.unwrap().editable,"{sql}");
                let commit=GridAction::Commit{columns:result.rows.columns,ddl:String::new(),changes:vec![]};
                assert!(grid_request(&mut db,DatabaseKind::Sqlite,"main","3",&plan,true,&commit).await.is_err());
            }
        });
    }
}

//! Interactive directory queries; deliberately independent from export preflight.
use super::workbench::{Access, Object, Reply, Schema, metadata_literal as literal_for, qualified};
use crate::DatabaseKind;
pub const PAGE: usize = 200;
pub const MAX_CACHE: usize = 5000;
fn source(kind: DatabaseKind, version: &str, db: &str, category: &str) -> Result<String, String> {
    let d = literal_for(kind, db);
    let sql=match kind {
 DatabaseKind::MySql=>match category {
 "relations"=>format!("SELECT TABLE_NAME AS name,CASE WHEN TABLE_TYPE='VIEW' THEN 'view' ELSE 'table' END AS kind,TABLE_COMMENT AS comment,'' AS parent,'' AS scope FROM information_schema.TABLES WHERE TABLE_SCHEMA={d}"),
 "index"=>format!("SELECT DISTINCT INDEX_NAME AS name,'index' AS kind,'' AS comment,TABLE_NAME AS parent,'' AS scope FROM information_schema.STATISTICS WHERE TABLE_SCHEMA={d}"),
 "trigger"=>format!("SELECT TRIGGER_NAME AS name,'trigger' AS kind,'' AS comment,EVENT_OBJECT_TABLE AS parent,'' AS scope FROM information_schema.TRIGGERS WHERE TRIGGER_SCHEMA={d}"),
 "procedure"|"function"=>format!("SELECT ROUTINE_NAME AS name,LOWER(ROUTINE_TYPE) AS kind,ROUTINE_COMMENT AS comment,'' AS parent,'' AS scope FROM information_schema.ROUTINES WHERE ROUTINE_SCHEMA={d} AND ROUTINE_TYPE={}",literal_for(kind,&category.to_ascii_uppercase())),
 "event"=>format!("SELECT EVENT_NAME AS name,'event' AS kind,EVENT_COMMENT AS comment,'' AS parent,'' AS scope FROM information_schema.EVENTS WHERE EVENT_SCHEMA={d}"),
 _=>return Err("browser.unsupported".into())},
 DatabaseKind::Sqlite=>{
 let types=match category {"relations"=>"'table','view'","index"=>"'index'","trigger"=>"'trigger'",_=>return Ok("SELECT '' AS name,'' AS kind,'' AS comment,'' AS parent,'' AS scope WHERE 0".into())};
 format!("SELECT name,type AS kind,'' AS comment,CASE WHEN type IN ('table','view') THEN '' ELSE tbl_name END AS parent,'' AS scope FROM sqlite_master WHERE type IN ({types}) AND name NOT LIKE 'sqlite_%'")},
 DatabaseKind::PostgreSql=>match category {
 "relations"|"index"=>format!("SELECT c.relname AS name,CASE WHEN c.relkind IN ('v','m') THEN 'view' WHEN c.relkind='i' THEN 'index' ELSE 'table' END AS kind,COALESCE(obj_description(c.oid),'') AS comment,'' AS parent,n.nspname AS scope FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE c.relkind IN ({}) AND n.nspname NOT IN ('pg_catalog','information_schema') AND n.nspname NOT LIKE 'pg_toast%'",if category=="index" {"'i'"} else {"'r','p','v','m','f'"}),
 "trigger"=>"SELECT t.tgname AS name,'trigger' AS kind,'' AS comment,c.relname AS parent,n.nspname AS scope FROM pg_trigger t JOIN pg_class c ON c.oid=t.tgrelid JOIN pg_namespace n ON n.oid=c.relnamespace WHERE NOT t.tgisinternal".into(),
 "procedure"|"function"=>format!("SELECT p.proname||'('||pg_get_function_identity_arguments(p.oid)||')' AS name,{} AS kind,COALESCE(obj_description(p.oid),'') AS comment,'' AS parent,n.nspname AS scope FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname NOT IN ('pg_catalog','information_schema') AND p.prokind={}",literal_for(kind,category),if category=="procedure" {"'p'"} else {"'f'"}),
 _=>return Ok("SELECT '' AS name,'' AS kind,'' AS comment,'' AS parent,'' AS scope WHERE false".into())},
 DatabaseKind::ClickHouse=>match category {
 "relations"=>format!("SELECT name,if(position(engine,'View')>0,'view','table') AS kind,comment,'' AS parent,'' AS scope FROM system.tables WHERE database={d}"),
 "index"=>format!("SELECT name,'index' AS kind,expr AS comment,table AS parent,'' AS scope FROM system.data_skipping_indices WHERE database={d}"),
 _=>return Ok("SELECT '' AS name,'' AS kind,'' AS comment,'' AS parent,'' AS scope WHERE 0".into())},
 _=>return Err("browser.unsupported".into()),
 };
    if kind == DatabaseKind::MySql && version.starts_with("4.") {
        return Err("browser.unsupported".into());
    }
    Ok(sql)
}
pub fn query(
    kind: DatabaseKind,
    version: &str,
    db: &str,
    category: &str,
    search: &str,
    offset: usize,
    limit: usize,
) -> Result<String, String> {
    query_with_comments(kind, version, db, category, search, offset, limit, false)
}
#[allow(clippy::too_many_arguments)]
pub fn query_with_comments(
    kind: DatabaseKind,
    version: &str,
    db: &str,
    category: &str,
    search: &str,
    offset: usize,
    limit: usize,
    include_comments: bool,
) -> Result<String, String> {
    if search.len() > 512
        || search.contains('\0')
        || offset > i64::MAX as usize
        || !(1..=PAGE).contains(&limit)
    {
        return Err("browser.invalid".into());
    }
    if kind == DatabaseKind::MySql && !version.starts_with("4.") && !include_comments {
        // MySQL 5.x can read names/types from directory entries without opening each
        // table for TABLE_COMMENT. Push the name predicate into information_schema.
        let db = literal_for(kind, db);
        let (columns, table, schema, name, extra) = match category {
            "relations" => (
                "TABLE_NAME,CASE WHEN TABLE_TYPE='VIEW' THEN 'view' ELSE 'table' END,'','',''",
                "TABLES",
                "TABLE_SCHEMA",
                "TABLE_NAME",
                "",
            ),
            "index" => (
                "DISTINCT INDEX_NAME,'index','',TABLE_NAME,''",
                "STATISTICS",
                "TABLE_SCHEMA",
                "INDEX_NAME",
                "",
            ),
            "trigger" => (
                "TRIGGER_NAME,'trigger','',EVENT_OBJECT_TABLE,''",
                "TRIGGERS",
                "TRIGGER_SCHEMA",
                "TRIGGER_NAME",
                "",
            ),
            "procedure" => (
                "ROUTINE_NAME,'procedure','','',''",
                "ROUTINES",
                "ROUTINE_SCHEMA",
                "ROUTINE_NAME",
                " AND ROUTINE_TYPE='PROCEDURE'",
            ),
            "function" => (
                "ROUTINE_NAME,'function','','',''",
                "ROUTINES",
                "ROUTINE_SCHEMA",
                "ROUTINE_NAME",
                " AND ROUTINE_TYPE='FUNCTION'",
            ),
            "event" => (
                "EVENT_NAME,'event','','',''",
                "EVENTS",
                "EVENT_SCHEMA",
                "EVENT_NAME",
                "",
            ),
            _ => return Err("browser.unsupported".into()),
        };
        let parent = match category {
            "index" => Some("TABLE_NAME"),
            "trigger" => Some("EVENT_OBJECT_TABLE"),
            _ => None,
        };
        let predicate = mysql_name_predicate(name, search, parent);
        let order = if category == "index" {
            "INDEX_NAME,TABLE_NAME"
        } else {
            name
        };
        return Ok(format!(
            "SELECT {columns} FROM information_schema.{table} WHERE {schema}={db}{extra}{predicate} ORDER BY {order} LIMIT {} OFFSET {offset}",
            limit + 1
        ));
    }
    let source = source(kind, version, db, category)?;
    let needle = literal_for(kind, &search.to_lowercase());
    let predicate = if search.is_empty() {
        String::new()
    } else {
        let haystack = match kind {
            DatabaseKind::MySql | DatabaseKind::ClickHouse => {
                "concat(scope,' ',name,' ',comment,' ',parent)"
            }
            _ => "scope||' '||name||' '||comment||' '||parent",
        };
        let condition = match kind {
            DatabaseKind::MySql => {
                format!("LOCATE(CAST({needle} AS BINARY),CAST(LOWER({haystack}) AS BINARY))>0")
            }
            DatabaseKind::PostgreSql => format!("strpos(lower({haystack}),{needle})>0"),
            DatabaseKind::ClickHouse => {
                format!("positionCaseInsensitiveUTF8({haystack},{needle})>0")
            }
            _ => format!("instr(lower({haystack}),{needle})>0"),
        };
        format!(" WHERE ({condition})")
    };
    Ok(format!(
        "SELECT name,kind,comment,parent,scope FROM ({source}) AS browser_objects{predicate} ORDER BY scope,name,kind,parent LIMIT {} OFFSET {offset}",
        limit + 1
    ))
}
#[allow(clippy::too_many_arguments)]
pub async fn browse(
    c: &mut impl Access,
    kind: DatabaseKind,
    version: &str,
    db: &str,
    category: &str,
    search: &str,
    offset: usize,
    limit: usize,
) -> Result<Reply, String> {
    browse_with_comments(c, kind, version, db, category, search, offset, limit, false).await
}
#[allow(clippy::too_many_arguments)]
pub async fn browse_with_comments(
    c: &mut impl Access,
    kind: DatabaseKind,
    version: &str,
    db: &str,
    category: &str,
    search: &str,
    offset: usize,
    limit: usize,
    include_comments: bool,
) -> Result<Reply, String> {
    if kind == DatabaseKind::MySql && version.starts_with("4.") {
        if category != "relations"
            || !search.is_empty()
            || !(1..=PAGE).contains(&limit)
            || offset > i64::MAX as usize
        {
            return Err("browser.unsupported".into());
        }
        let sql = format!("SHOW TABLES FROM {}", super::workbench::quote(kind, db));
        let rows = c.directory_page(&sql, offset, limit).await?;
        let more = rows.values.len() > limit;
        let objects = rows
            .values
            .into_iter()
            .take(limit)
            .map(|r| {
                Ok(Object {
                    name: r
                        .first()
                        .and_then(Clone::clone)
                        .ok_or("browser.response_failed")?,
                    kind: "table".into(),
                    schema: String::new(),
                    comment: String::new(),
                    parent: String::new(),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        return Ok(Reply::CatalogPage { objects, more });
    }
    let sql = query_with_comments(
        kind,
        version,
        db,
        category,
        search,
        offset,
        limit,
        include_comments,
    )?;
    if kind == DatabaseKind::MySql && category == "relations" && !include_comments {
        return mysql_relations(c, db, search, offset, limit).await;
    }
    let rows = c.query(&sql).await?;
    let more = rows.values.len() > limit;
    let value = |r: &Vec<Option<String>>, i| r.get(i).and_then(Clone::clone).unwrap_or_default();
    let objects = rows
        .values
        .iter()
        .take(limit)
        .map(|r| Object {
            name: value(r, 0),
            kind: value(r, 1),
            comment: value(r, 2),
            parent: value(r, 3),
            schema: value(r, 4),
        })
        .collect();
    Ok(Reply::CatalogPage { objects, more })
}
fn mysql_name_predicate(name: &str, search: &str, parent: Option<&str>) -> String {
    if search.is_empty() {
        return String::new();
    }
    let escaped = search
        .replace('=', "==")
        .replace('%', "=%")
        .replace('_', "=_");
    let pattern = literal_for(DatabaseKind::MySql, &format!("%{escaped}%"));
    if let Some(parent) = parent {
        format!(" AND ({name} LIKE {pattern} ESCAPE '=' OR {parent} LIKE {pattern} ESCAPE '=')")
    } else {
        format!(" AND {name} LIKE {pattern} ESCAPE '='")
    }
}
async fn mysql_relations(
    c: &mut impl Access,
    db: &str,
    search: &str,
    offset: usize,
    limit: usize,
) -> Result<Reply, String> {
    let db = literal_for(DatabaseKind::MySql, db);
    let filter = mysql_name_predicate("TABLE_NAME", search, None);
    // TABLE_TYPE is deliberately absent: 5.x can enumerate names without opening
    // every .frm in a large schema. Only classify the selected, bounded page.
    let rows = c.query(&format!("SELECT TABLE_NAME FROM information_schema.TABLES WHERE TABLE_SCHEMA={db}{filter} ORDER BY TABLE_NAME LIMIT {} OFFSET {offset}",limit+1)).await?;
    let more = rows.values.len() > limit;
    let names = rows
        .values
        .iter()
        .take(limit)
        .map(|r| {
            r.first()
                .and_then(Clone::clone)
                .ok_or_else(|| "browser.response_failed".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    if names.is_empty() {
        return Ok(Reply::CatalogPage {
            objects: vec![],
            more,
        });
    }
    let targets = names
        .iter()
        .map(|name| format!("TABLE_NAME={}", literal_for(DatabaseKind::MySql, name)))
        .collect::<Vec<_>>()
        .join(" OR ");
    let types = c.query(&format!("SELECT TABLE_NAME,CASE WHEN TABLE_TYPE='VIEW' THEN 'view' ELSE 'table' END FROM information_schema.TABLES WHERE TABLE_SCHEMA={db} AND ({targets})")).await?;
    let mut kinds = std::collections::HashMap::new();
    for row in &types.values {
        if let (Some(Some(name)), Some(Some(kind))) = (row.first(), row.get(1)) {
            kinds.insert(name.as_str(), kind.as_str());
        }
    }
    let objects = names
        .into_iter()
        .map(|name| {
            let kind = kinds
                .get(name.as_str())
                .filter(|k| matches!(**k, "table" | "view"))
                .ok_or("browser.response_failed")?
                .to_string();
            Ok(Object {
                name,
                kind,
                comment: String::new(),
                parent: String::new(),
                schema: String::new(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Reply::CatalogPage { objects, more })
}
pub async fn section(
    c: &mut impl Access,
    kind: DatabaseKind,
    db: &str,
    object: &Object,
    section: &str,
) -> Result<Reply, String> {
    let mut schema = Schema::default();
    let name = qualified(kind, db, object);
    let sql = match (kind, section) {
        (DatabaseKind::MySql, "dbwork.index") => format!("SHOW INDEX FROM {name}"),
        (DatabaseKind::MySql, "dbwork.foreign") => format!(
            "SELECT CONSTRAINT_NAME,COLUMN_NAME,REFERENCED_TABLE_NAME,REFERENCED_COLUMN_NAME FROM information_schema.KEY_COLUMN_USAGE WHERE TABLE_SCHEMA={} AND TABLE_NAME={} AND REFERENCED_TABLE_NAME IS NOT NULL ORDER BY CONSTRAINT_NAME,ORDINAL_POSITION LIMIT 201",
            literal_for(kind, db),
            literal_for(kind, &object.name)
        ),
        (DatabaseKind::MySql, "dbwork.trigger") => format!(
            "SELECT TRIGGER_NAME,ACTION_TIMING,EVENT_MANIPULATION FROM information_schema.TRIGGERS WHERE TRIGGER_SCHEMA={} AND EVENT_OBJECT_TABLE={} ORDER BY TRIGGER_NAME LIMIT 201",
            literal_for(kind, db),
            literal_for(kind, &object.name)
        ),
        (DatabaseKind::Sqlite, "dbwork.index") => format!(
            "SELECT name,\"unique\",origin FROM pragma_index_list({}) LIMIT 201",
            literal_for(kind, &object.name)
        ),
        (DatabaseKind::Sqlite, "dbwork.foreign") => format!(
            "SELECT id,\"table\",\"from\",\"to\" FROM pragma_foreign_key_list({}) LIMIT 201",
            literal_for(kind, &object.name)
        ),
        (DatabaseKind::Sqlite, "dbwork.trigger") => format!(
            "SELECT name FROM sqlite_master WHERE type='trigger' AND tbl_name={} ORDER BY name LIMIT 201",
            literal_for(kind, &object.name)
        ),
        (DatabaseKind::PostgreSql, "dbwork.index") => format!(
            "SELECT indexname,indexdef FROM pg_indexes WHERE schemaname={} AND tablename={} ORDER BY indexname LIMIT 201",
            literal_for(kind, &object.schema),
            literal_for(kind, &object.name)
        ),
        (DatabaseKind::PostgreSql, "dbwork.foreign") => format!(
            "SELECT conname,pg_get_constraintdef(oid) FROM pg_constraint WHERE conrelid={}::regclass AND contype='f' ORDER BY conname LIMIT 201",
            literal_for(kind, &name)
        ),
        (DatabaseKind::PostgreSql, "dbwork.trigger") => format!(
            "SELECT tgname FROM pg_trigger WHERE tgrelid={}::regclass AND NOT tgisinternal ORDER BY tgname LIMIT 201",
            literal_for(kind, &name)
        ),
        (DatabaseKind::ClickHouse, "dbwork.index") => format!(
            "SELECT name,expr,type FROM system.data_skipping_indices WHERE database={} AND table={} ORDER BY name LIMIT 201",
            literal_for(kind, db),
            literal_for(kind, &object.name)
        ),
        (DatabaseKind::ClickHouse, _) => return Ok(Reply::Schema(schema)),
        _ => return Err("browser.unsupported".into()),
    };
    let mut rows = c.query(&sql).await?;
    if kind == DatabaseKind::MySql && section == "dbwork.index" {
        rows = super::workbench::Rows {
            columns: vec!["Index".into(), "Column".into(), "Non_unique".into()],
            values: rows
                .values
                .iter()
                .map(|r| {
                    vec![
                        r.get(2).cloned().flatten(),
                        r.get(4).cloned().flatten(),
                        r.get(1).cloned().flatten(),
                    ]
                })
                .collect(),
        };
    }
    if rows.values.len() > 200 {
        return Err("browser.section_limit".into());
    }
    match section {
        "dbwork.index" => schema.indexes = rows,
        "dbwork.foreign" => schema.foreign_keys = rows,
        "dbwork.trigger" => schema.triggers = rows,
        _ => return Err("browser.invalid".into()),
    };
    Ok(Reply::Schema(schema))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::workbench::{Access, Action, Query, Request};
    use sqlx::Connection;
    struct Probe {
        connection: sqlx::SqliteConnection,
        sql: Vec<String>,
    }
    impl Access for Probe {
        fn query<'a>(&'a mut self, sql: &'a str) -> Query<'a> {
            Box::pin(async move {
                self.sql.push(sql.into());
                Access::query(&mut self.connection, sql).await
            })
        }
    }
    #[tokio::test]
    async fn directories_search_beyond_first_500_and_defer_columns_and_ddl() {
        let mut connection = sqlx::SqliteConnection::connect(":memory:").await.unwrap();
        for n in 0..1005 {
            sqlx::raw_sql(&format!(
                "CREATE TABLE item_{n:04}(id INTEGER PRIMARY KEY, label TEXT NOT NULL DEFAULT '')"
            ))
            .execute(&mut connection)
            .await
            .unwrap();
        }
        let mut probe = Probe {
            connection,
            sql: vec![],
        };
        let Reply::CatalogPage { objects, more } = browse(
            &mut probe,
            DatabaseKind::Sqlite,
            "",
            "main",
            "relations",
            "",
            0,
            200,
        )
        .await
        .unwrap() else {
            panic!()
        };
        assert_eq!(objects.len(), 200);
        assert!(more);
        assert_eq!(objects[0].name, "item_0000");
        let Reply::CatalogPage { objects, more } = browse(
            &mut probe,
            DatabaseKind::Sqlite,
            "",
            "main",
            "relations",
            "item_1000",
            0,
            200,
        )
        .await
        .unwrap() else {
            panic!()
        };
        assert_eq!(objects.len(), 1);
        assert!(!more);
        assert_eq!(objects[0].name, "item_1000");
        assert!(probe.sql.iter().all(|s| !s.contains("pragma_table")
            && !s.contains("SHOW CREATE")
            && !s.contains("SELECT *")));
        let object = objects[0].clone();
        let Reply::Schema(schema) = crate::database::workbench::run(
            &mut probe,
            DatabaseKind::Sqlite,
            "",
            &Request {
                database: "main".into(),
                action: Action::BrowseColumns(object.clone()),
            },
        )
        .await
        .unwrap() else {
            panic!()
        };
        assert_eq!(schema.columns.len(), 2);
        assert!(schema.ddl.is_empty());
        assert!(schema.indexes.values.is_empty());
        assert_eq!(
            probe
                .sql
                .last()
                .unwrap()
                .matches("pragma_table_xinfo")
                .count(),
            1
        );
        let Reply::CatalogPage { objects, .. } = browse(
            &mut probe,
            DatabaseKind::Sqlite,
            "",
            "main",
            "relations",
            "",
            200,
            200,
        )
        .await
        .unwrap() else {
            panic!()
        };
        assert_eq!(objects[0].name, "item_0200");
        let Reply::CatalogPage { objects, .. } = browse(
            &mut probe,
            DatabaseKind::Sqlite,
            "",
            "main",
            "relations",
            "%' OR 1=1 --",
            0,
            200,
        )
        .await
        .unwrap() else {
            panic!()
        };
        assert!(objects.is_empty());
    }
    #[test]
    fn bounds_and_categories_fail_before_querying() {
        let large = query(
            DatabaseKind::MySql,
            "5.6.21",
            "db",
            "relations",
            "",
            60_000,
            200,
        )
        .unwrap();
        assert!(large.ends_with("LIMIT 201 OFFSET 60000"));
        assert!(
            query(
                DatabaseKind::MySql,
                "5.6.21",
                "db",
                "relations",
                "",
                usize::MAX,
                200
            )
            .is_err()
        );
        assert!(query(DatabaseKind::MySql, "5.6.21", "db", "relations", "", 0, 201).is_err());
        assert!(query(DatabaseKind::MySql, "5.6.21", "db", "malicious", "", 0, 200).is_err());
        assert!(
            query(
                DatabaseKind::MySql,
                "5.6.21",
                "db",
                "relations",
                "x\0",
                0,
                200
            )
            .is_err()
        );
        let sql = query(
            DatabaseKind::MySql,
            "5.6.21",
            "db",
            "relations",
            "x' OR 1=1 --",
            0,
            200,
        )
        .unwrap();
        assert!(!sql.contains("x' OR"));
        assert!(sql.ends_with("LIMIT 201 OFFSET 0"));
    }
    #[test]
    fn mysql_names_search_avoids_table_open_metadata_and_escapes_like_literals() {
        for category in [
            "relations",
            "index",
            "trigger",
            "procedure",
            "function",
            "event",
        ] {
            let sql = query(
                DatabaseKind::MySql,
                "5.6.21",
                "db",
                category,
                "cq_%=",
                0,
                200,
            )
            .unwrap();
            assert!(!sql.contains("TABLE_COMMENT"));
            assert!(!sql.contains("LOWER("));
            assert!(!sql.contains("FROM ("));
            assert!(sql.contains(" ESCAPE '='"));
            assert!(sql.contains(&literal_for(DatabaseKind::MySql, "%cq=_=%==%")));
        }
        let broad = query_with_comments(
            DatabaseKind::MySql,
            "5.6.21",
            "db",
            "relations",
            "中文注释",
            0,
            200,
            true,
        )
        .unwrap();
        assert!(broad.contains("TABLE_COMMENT"));
        assert!(broad.contains("LOCATE("));
    }
    #[tokio::test]
    async fn mysql_classifies_only_the_name_page_and_preserves_views_and_order() {
        struct Replies {
            sql: Vec<String>,
            replies: std::collections::VecDeque<super::super::workbench::Rows>,
        }
        impl Access for Replies {
            fn query<'a>(&'a mut self, sql: &'a str) -> Query<'a> {
                self.sql.push(sql.into());
                Box::pin(async move {
                    self.replies
                        .pop_front()
                        .ok_or_else(|| "unexpected query".into())
                })
            }
        }
        let mut c = Replies {
            sql: vec![],
            replies: [
                super::super::workbench::Rows {
                    columns: vec![],
                    values: vec![
                        vec![Some("table_500".into())],
                        vec![Some("view_600".into())],
                        vec![Some("sentinel".into())],
                    ],
                },
                super::super::workbench::Rows {
                    columns: vec![],
                    values: vec![
                        vec![Some("view_600".into()), Some("view".into())],
                        vec![Some("table_500".into()), Some("table".into())],
                    ],
                },
            ]
            .into(),
        };
        let Reply::CatalogPage { objects, more } = browse(
            &mut c,
            DatabaseKind::MySql,
            "5.6.21",
            "db",
            "relations",
            "",
            0,
            2,
        )
        .await
        .unwrap() else {
            panic!()
        };
        assert!(more);
        assert_eq!(
            objects
                .iter()
                .map(|o| (o.name.as_str(), o.kind.as_str()))
                .collect::<Vec<_>>(),
            vec![("table_500", "table"), ("view_600", "view")]
        );
        assert!(!c.sql[0].contains("TABLE_TYPE"));
        assert!(c.sql[0].ends_with("LIMIT 3 OFFSET 0"));
        assert!(c.sql[1].contains(&literal_for(DatabaseKind::MySql, "table_500")));
        assert!(c.sql[1].contains(&literal_for(DatabaseKind::MySql, "view_600")));
        assert!(!c.sql[1].contains(&literal_for(DatabaseKind::MySql, "sentinel")));
        assert!(c.sql.iter().all(|s| !s.contains("TABLE_COMMENT")));
    }
}

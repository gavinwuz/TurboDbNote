//! Bounded relational workbench operations shared by native and legacy adapters.
use crate::DatabaseKind;
use serde::{Deserialize, Serialize};
use std::{future::Future, pin::Pin};

#[path = "grid.rs"]
pub(super) mod grid;
#[path = "script_context.rs"]
pub mod script_context;
pub use grid::{
    Change, DEFAULT_GRID_PAGE_SIZE, Filter, FilterJoin, FilterSet, MAX_FILTER_CONDITIONS,
    MAX_GRID_PAGE_SIZE, PageOptions, cell_where, copy_row, copy_update, edit_preview,
};

#[path = "account_admin.rs"]
pub mod account_admin;

pub const PAGE_SIZE: usize = 100;
fn legacy_page_size() -> usize {
    PAGE_SIZE
}
pub const MAX_BYTES: usize = 768 * 1024;
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Rows {
    pub columns: Vec<String>,
    pub values: Vec<Vec<Option<String>>>,
}
pub type Query<'a> = Pin<Box<dyn Future<Output = Result<Rows, String>> + 'a>>;
pub type CountedQuery<'a> = Pin<Box<dyn Future<Output = Result<(Rows, Option<u64>), String>> + 'a>>;
pub trait Access {
    fn supports_keyset(&self) -> bool {
        true
    }
    fn query_data<'a>(&'a mut self, sql: &'a str) -> Query<'a> {
        self.query(sql)
    }
    fn query_data_counted<'a>(&'a mut self, sql: &'a str) -> CountedQuery<'a> {
        self.query_counted(sql)
    }

    /// Bounded names from a SHOW directory on servers without information_schema.
    fn directory_page<'a>(&'a mut self, sql: &'a str, offset: usize, limit: usize) -> Query<'a> {
        Box::pin(async move {
            let mut rows = self.query(sql).await?;
            rows.values = rows
                .values
                .into_iter()
                .skip(offset)
                .take(limit + 1)
                .collect();
            Ok(rows)
        })
    }
    fn query<'a>(&'a mut self, sql: &'a str) -> Query<'a>;
    fn query_counted<'a>(&'a mut self, sql: &'a str) -> CountedQuery<'a> {
        Box::pin(async move { self.query(sql).await.map(|rows| (rows, None)) })
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Object {
    pub schema: String,
    pub name: String,
    pub kind: String,
    pub comment: String,
    pub parent: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Column {
    #[serde(default)]
    pub not_null: Option<bool>,
    #[serde(default)]
    pub auto_increment: Option<bool>,
    #[serde(default)]
    pub extra: String,
    pub name: String,
    pub data_type: String,
    pub comment: String,
    pub primary: bool,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Schema {
    pub columns: Vec<Column>,
    pub indexes: Rows,
    pub foreign_keys: Rows,
    pub triggers: Rows,
    pub ddl: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub database: String,
    pub action: Action,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Action {
    AccountGrants {
        user: String,
        host: String,
    },
    AccountApplyGrants {
        plan: account_admin::GrantPlan,
        confirmed: bool,
    },
    AccountMode,
    AccountCreate {
        draft: account_admin::Draft,
        sql_mode: String,
        confirmed: bool,
    },
    AccountPermissions {
        user: String,
        host: String,
        search: String,
        offset: usize,
    },
    Accounts {
        offset: usize,
        limit: usize,
    },
    BrowseCatalog {
        category: String,
        search: String,
        offset: usize,
        limit: usize,
        #[serde(default)]
        include_comments: bool,
    },
    BrowseColumns(Object),
    BrowseSection {
        object: Object,
        section: String,
    },
    MongoWrite(turbodbn_core::mongo::Mutation),
    RedisWrite(super::redis_value::Mutation),
    RedisScan {
        cursor: Option<String>,
        pattern: String,
    },
    RedisValue {
        key: Vec<u8>,
        cursor: String,
        offset: usize,
        size: usize,
    },
    NoteGrid {
        plan: super::notebook_query::Plan,
        confirmed: bool,
        action: super::notebook_query::GridAction,
    },
    NoteExplain {
        sql: String,
    },
    NotePrepare {
        sql: String,
    },
    ScriptDiagnostics,
    ScriptPrepare {
        sql: String,
    },
    NoteExecute {
        plan: super::notebook_query::Plan,
        confirmed: bool,
        offset: usize,
        page_size: usize,
    },
    ScriptExecute {
        plan: super::notebook_query::Plan,
        confirmed: bool,
    },
    GridPage {
        object: Object,
        options: PageOptions,
    },
    Count {
        object: Object,
        filter: Option<FilterSet>,
    },
    Commit {
        object: Object,
        ddl: String,
        changes: Vec<Change>,
    },
    Catalog,
    Describe(Object),
    Ddl(Object),
    NoteColumns(Object),
    Page {
        object: Object,
        offset: usize,
        #[serde(default = "legacy_page_size")]
        page_size: usize,
    },
    Execute {
        sql: String,
        guard: Option<(Object, String)>,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Reply {
    AccountGrants(Vec<account_admin::Grant>),
    CatalogPage {
        objects: Vec<Object>,
        more: bool,
    },
    MongoPage(turbodbn_core::mongo::Page),
    MongoWritten {
        inserted: bool,
    },
    RedisValue(super::redis_value::ValuePage),
    ScriptDiagnostics(super::notebook_query::ScriptDiagnostics),
    NotePrepared(super::notebook_query::Plan),
    NoteResult(super::notebook_query::ResultPage),
    GridPage {
        rows: Rows,
        schema: Schema,
        offset: usize,
        more: bool,
        sql: String,
        next: Option<Vec<String>>,
        elapsed_ms: u64,
        keyset: bool,
    },
    Count(u64),
    Catalog {
        objects: Vec<Object>,
        notice: String,
    },
    Schema(Schema),
    Page {
        rows: Rows,
        offset: usize,
        more: bool,
        sql: String,
    },
    Executed,
}
fn value(row: &[Option<String>], i: usize) -> String {
    row.get(i).and_then(Clone::clone).unwrap_or_default()
}
pub fn quote(kind: DatabaseKind, name: &str) -> String {
    let ch = if matches!(kind, DatabaseKind::MySql | DatabaseKind::ClickHouse) {
        '`'
    } else {
        '"'
    };
    format!("{ch}{}{ch}", name.replace(ch, &format!("{ch}{ch}")))
}
pub fn literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}
pub fn qualified(kind: DatabaseKind, database: &str, object: &Object) -> String {
    let scope = match kind {
        DatabaseKind::MySql | DatabaseKind::ClickHouse => database,
        DatabaseKind::PostgreSql => &object.schema,
        _ => "",
    };
    if scope.is_empty() {
        quote(kind, &object.name)
    } else {
        format!("{}.{}", quote(kind, scope), quote(kind, &object.name))
    }
}
async fn catalog(
    c: &mut impl Access,
    kind: DatabaseKind,
    db: &str,
    version: &str,
) -> Result<Reply, String> {
    let mut objects = Vec::new();
    let mut notices = Vec::new();
    let mut queries: Vec<(&str, String)> = Vec::new();
    match kind {
        DatabaseKind::External => return Err("drivers.operation_unsupported".into()),
 DatabaseKind::MySql if version.starts_with("4.")=>{
   for r in c.query(&format!("SHOW TABLE STATUS FROM {}",quote(kind,db))).await?.values { objects.push(Object{schema:String::new(),name:value(&r,0),kind:"table".into(),comment:r.last().cloned().flatten().unwrap_or_default(),parent:String::new()}); }
   return Ok(Reply::Catalog{objects,notice:"MySQL 4.x：可查看表、列、主键和索引；该版本不支持视图、触发器、存储过程和事件".into()});
 }
 DatabaseKind::MySql=>{
  queries.push(("",format!("SELECT TABLE_NAME,CASE WHEN TABLE_TYPE='VIEW' THEN 'view' ELSE 'table' END,TABLE_COMMENT,'' FROM information_schema.TABLES WHERE TABLE_SCHEMA={} ORDER BY TABLE_NAME LIMIT 501",literal_for(kind,db))));
  queries.push(("index",format!("SELECT DISTINCT INDEX_NAME,'index','',TABLE_NAME FROM information_schema.STATISTICS WHERE TABLE_SCHEMA={} LIMIT 501",literal_for(kind,db))));
  queries.push(("trigger",format!("SELECT TRIGGER_NAME,'trigger','',EVENT_OBJECT_TABLE FROM information_schema.TRIGGERS WHERE TRIGGER_SCHEMA={} LIMIT 501",literal_for(kind,db))));
  queries.push(("procedure",format!("SELECT ROUTINE_NAME,LOWER(ROUTINE_TYPE),ROUTINE_COMMENT,'' FROM information_schema.ROUTINES WHERE ROUTINE_SCHEMA={} LIMIT 501",literal_for(kind,db))));
  queries.push(("event",format!("SELECT EVENT_NAME,'event',EVENT_COMMENT,'' FROM information_schema.EVENTS WHERE EVENT_SCHEMA={} LIMIT 501",literal_for(kind,db))));
 }
 DatabaseKind::Sqlite=>queries.push(("","SELECT name,type,'',CASE WHEN name=tbl_name THEN '' ELSE tbl_name END FROM sqlite_master WHERE type IN ('table','view','index','trigger') AND name NOT LIKE 'sqlite_%' ORDER BY type,name LIMIT 501".into())),
 DatabaseKind::PostgreSql=>{
 queries.push(("", "SELECT c.relname,CASE c.relkind WHEN 'v' THEN 'view' WHEN 'm' THEN 'view' WHEN 'i' THEN 'index' ELSE 'table' END,COALESCE(obj_description(c.oid),''),'',n.nspname FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE c.relkind IN ('r','p','v','m','i','f') AND n.nspname NOT IN ('pg_catalog','information_schema') AND n.nspname NOT LIKE 'pg_toast%' ORDER BY n.nspname,c.relname LIMIT 501".into()));
 queries.push(("trigger","SELECT t.tgname,'trigger','',c.relname,n.nspname FROM pg_trigger t JOIN pg_class c ON c.oid=t.tgrelid JOIN pg_namespace n ON n.oid=c.relnamespace WHERE NOT t.tgisinternal LIMIT 501".into()));
 queries.push(("procedure","SELECT p.proname||'('||pg_get_function_identity_arguments(p.oid)||')',CASE p.prokind WHEN 'p' THEN 'procedure' ELSE 'function' END,COALESCE(obj_description(p.oid),''),'',n.nspname FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname NOT IN ('pg_catalog','information_schema') LIMIT 501".into()));
 }
 DatabaseKind::ClickHouse=>{
 queries.push(("",format!("SELECT name,if(position(engine,'View')>0,'view','table'),comment,'' FROM system.tables WHERE database={} ORDER BY name LIMIT 501",literal_for(kind,db))));
 queries.push(("index",format!("SELECT name,'index',expr,table FROM system.data_skipping_indices WHERE database={} LIMIT 501",literal_for(kind,db))));
 }
 DatabaseKind::Redis=>return Err("Redis 使用 Key 浏览器".into()),
 }
    for (category, sql) in queries {
        match c.query(&sql).await {
            Ok(rows) => {
                if rows.values.len() > 500 {
                    notices.push(format!("{category}：目录超过 500 项"));
                }
                for r in rows.values.into_iter().take(500) {
                    objects.push(Object {
                        schema: value(&r, 4),
                        name: value(&r, 0),
                        kind: value(&r, 1),
                        comment: value(&r, 2),
                        parent: value(&r, 3),
                    });
                }
            }
            Err(_) if !category.is_empty() => notices.push(format!(
                "{category}: 元数据不可读取，可能缺少权限；可取消该类型后继续导出"
            )),
            Err(e) => return Err(e),
        }
    }
    if kind == DatabaseKind::MySql && !version.starts_with("4.") {
        for (category, sql_kind) in [
            ("view", "VIEW"),
            ("trigger", "TRIGGER"),
            ("procedure", "PROCEDURE"),
            ("function", "FUNCTION"),
            ("event", "EVENT"),
        ] {
            if let Some(object) = objects.iter().find(|o| o.kind == category)
                && c.query(&format!(
                    "SHOW CREATE {sql_kind} {}",
                    qualified(kind, db, object)
                ))
                .await
                .is_err()
            {
                notices.push(format!(
                    "{category}: 部分对象定义不可读取，可能缺少权限；可取消该类型后继续导出"
                ));
            }
        }
    }
    Ok(Reply::Catalog {
        objects,
        notice: notices.join(" · "),
    })
}
pub(super) async fn schema(
    c: &mut impl Access,
    kind: DatabaseKind,
    db: &str,
    object: &Object,
    details: bool,
) -> Result<Schema, String> {
    let name = qualified(kind, db, object);
    let mut s = Schema::default();
    match kind {
        DatabaseKind::External => return Err("drivers.operation_unsupported".into()),
        DatabaseKind::MySql => {
            for r in c
                .query(&format!("SHOW FULL COLUMNS FROM {name}"))
                .await?
                .values
            {
                // 4.0 omits Collation; locate fields using the returned headers in future server variants.
                let modern = r.len() >= 9;
                let key = if modern { 4 } else { 3 };
                s.columns.push(Column {
                    name: value(&r, 0),
                    data_type: value(&r, 1),
                    primary: value(&r, key) == "PRI",
                    not_null: Some(value(&r, if modern { 3 } else { 2 }) == "NO"),
                    auto_increment: Some(
                        value(&r, key + 2)
                            .split_whitespace()
                            .any(|v| v.eq_ignore_ascii_case("auto_increment")),
                    ),
                    extra: format!(
                        "{}{}",
                        r.get(key + 1)
                            .and_then(|v| v.as_ref())
                            .map(|v| format!("DEFAULT {v}; "))
                            .unwrap_or_default(),
                        value(&r, key + 2)
                    ),

                    comment: if modern {
                        value(&r, 8)
                    } else if r.len() >= 8 {
                        value(&r, 7)
                    } else {
                        String::new()
                    },
                });
            }
            if !details {
                return Ok(s);
            }
            let indexes = c.query(&format!("SHOW INDEX FROM {name}")).await?;
            s.indexes = Rows {
                columns: vec!["Index".into(), "Column".into(), "Non_unique".into()],
                values: indexes
                    .values
                    .iter()
                    .map(|r| vec![Some(value(r, 2)), Some(value(r, 4)), Some(value(r, 1))])
                    .collect(),
            };
            s.ddl = c
                .query(&format!("SHOW CREATE TABLE {name}"))
                .await?
                .values
                .first()
                .map(|r| value(r, 1))
                .unwrap_or_default();
            // Foreign keys are retained verbatim in SHOW CREATE, including old servers.
            s.foreign_keys = Rows {
                columns: vec!["Definition".into()],
                values: s
                    .ddl
                    .lines()
                    .filter(|l| l.contains("FOREIGN KEY"))
                    .map(|l| vec![Some(l.trim().into())])
                    .collect(),
            };
        }
        DatabaseKind::Sqlite => {
            for r in c
                .query(&format!(
                    "SELECT name,type,pk,\"notnull\",dflt_value,hidden FROM pragma_table_xinfo({}) WHERE hidden<>1",
                    literal_for(kind, &object.name)
                ))
                .await?
                .values
            {
                s.columns.push(Column {
                    name: value(&r, 0),
                    data_type: value(&r, 1),
                    primary: value(&r, 2) != "0",
                    not_null: Some(value(&r,3)=="1"),
                    extra: format!("{}{}", r.get(4).and_then(|v|v.as_ref()).map(|v|format!("DEFAULT {v}; ")).unwrap_or_default(), if matches!(value(&r,5).as_str(),"2"|"3") {"GENERATED"} else {""}),
                    comment: String::new(),
                    ..Default::default()
                });
            }
            if !details {
                return Ok(s);
            }
            s.indexes = c
                .query(&format!(
                    "SELECT name,\"unique\",origin FROM pragma_index_list({})",
                    literal_for(kind, &object.name)
                ))
                .await?;
            s.foreign_keys = c
                .query(&format!(
                    "SELECT \"table\",\"from\",\"to\" FROM pragma_foreign_key_list({})",
                    literal_for(kind, &object.name)
                ))
                .await?;
            s.triggers = c
                .query(&format!(
                    "SELECT name,sql FROM sqlite_master WHERE type='trigger' AND tbl_name={}",
                    literal_for(kind, &object.name)
                ))
                .await?;
            s.ddl = c
                .query(&format!(
                    "SELECT sql FROM sqlite_master WHERE name={} AND type IN ('table','view')",
                    literal_for(kind, &object.name)
                ))
                .await?
                .values
                .first()
                .map(|r| value(r, 0))
                .unwrap_or_default();
            if let Ok(statements) =
                sqlparser::parser::Parser::parse_sql(&sqlparser::dialect::SQLiteDialect {}, &s.ddl)
                && let Some(sqlparser::ast::Statement::CreateTable(table)) = statements.first()
            {
                for column in &mut s.columns {
                    if let Some(definition) =
                        table.columns.iter().find(|c| c.name.value == column.name)
                    {
                        column.auto_increment = Some(definition.options.iter().any(|o| matches!(&o.option,
                            sqlparser::ast::ColumnOption::DialectSpecific(tokens) if tokens.iter().any(|t| t.to_string().eq_ignore_ascii_case("AUTOINCREMENT")))));
                    }
                }
            }
        }
        DatabaseKind::PostgreSql => {
            let relation = literal_for(kind, &name);
            for r in c.query(&format!("SELECT a.attname,format_type(a.atttypid,a.atttypmod),COALESCE(col_description(a.attrelid,a.attnum),''),EXISTS(SELECT 1 FROM pg_index i WHERE i.indrelid=a.attrelid AND i.indisprimary AND a.attnum=ANY(i.indkey)),a.attnotnull,COALESCE((SELECT pg_get_expr(d.adbin,d.adrelid) FROM pg_attrdef d WHERE d.adrelid=a.attrelid AND d.adnum=a.attnum),''),a.attidentity FROM pg_attribute a WHERE a.attrelid={relation}::regclass AND a.attnum>0 AND NOT a.attisdropped ORDER BY a.attnum")).await?.values {s.columns.push(Column{name:value(&r,0),data_type:value(&r,1),comment:value(&r,2),primary:matches!(value(&r,3).as_str(),"t"|"true"),not_null:Some(matches!(value(&r,4).as_str(),"t"|"true")),extra:value(&r,5),auto_increment:Some(!value(&r,6).is_empty() || value(&r,5).starts_with("nextval(")) });}
            if !details {
                return Ok(s);
            }
            s.indexes=c.query(&format!("SELECT indexname,indexdef FROM pg_indexes WHERE schemaname={} AND tablename={}",literal_for(kind,&object.schema),literal_for(kind,&object.name))).await?;
            s.foreign_keys=c.query(&format!("SELECT conname,pg_get_constraintdef(oid) FROM pg_constraint WHERE conrelid={relation}::regclass AND contype='f'")).await?;
            s.triggers=c.query(&format!("SELECT tgname,pg_get_triggerdef(oid) FROM pg_trigger WHERE tgrelid={relation}::regclass AND NOT tgisinternal")).await?;
            if object.kind == "view" {
                s.ddl = format!(
                    "CREATE VIEW {name} AS {};",
                    c.query(&format!("SELECT pg_get_viewdef({relation}::regclass,true)"))
                        .await?
                        .values
                        .first()
                        .map(|r| value(r, 0))
                        .unwrap_or_default()
                );
            } else {
                let rows=c.query(&format!("SELECT quote_ident(a.attname)||' '||format_type(a.atttypid,a.atttypmod)||CASE WHEN a.attnotnull THEN ' NOT NULL' ELSE '' END||CASE WHEN a.attidentity='a' THEN ' GENERATED ALWAYS AS IDENTITY' WHEN a.attidentity='d' THEN ' GENERATED BY DEFAULT AS IDENTITY' WHEN a.attgenerated='s' THEN ' GENERATED ALWAYS AS ('||pg_get_expr(d.adbin,d.adrelid)||') STORED' WHEN d.adbin IS NOT NULL THEN ' DEFAULT '||pg_get_expr(d.adbin,d.adrelid) ELSE '' END FROM pg_attribute a LEFT JOIN pg_attrdef d ON d.adrelid=a.attrelid AND d.adnum=a.attnum WHERE a.attrelid={relation}::regclass AND a.attnum>0 AND NOT a.attisdropped ORDER BY a.attnum")).await?;
                let mut defs = rows.values.iter().map(|r| value(r, 0)).collect::<Vec<_>>();
                defs.extend(c.query(&format!("SELECT 'CONSTRAINT '||quote_ident(conname)||' '||pg_get_constraintdef(oid) FROM pg_constraint WHERE conrelid={relation}::regclass")).await?.values.iter().map(|r|value(r,0)));
                s.ddl = format!(
                    "-- Logical table definition; storage/partition/ownership settings are not included.\nCREATE TABLE {name} (\n  {}\n);",
                    defs.join(",\n  ")
                );
            }
        }
        DatabaseKind::ClickHouse => {
            for r in c.query(&format!("SELECT name,type,comment,is_in_primary_key FROM system.columns WHERE database={} AND table={} ORDER BY position",literal_for(kind,db),literal_for(kind,&object.name))).await?.values{s.columns.push(Column{name:value(&r,0),data_type:value(&r,1),comment:value(&r,2),primary:value(&r,3)=="1", ..Default::default() });}
            if !details {
                return Ok(s);
            }
            s.indexes=c.query(&format!("SELECT name,expr,type FROM system.data_skipping_indices WHERE database={} AND table={}",literal_for(kind,db),literal_for(kind,&object.name))).await?;
            s.ddl = c
                .query(&format!("SHOW CREATE TABLE {name}"))
                .await?
                .values
                .first()
                .map(|r| value(r, 0))
                .unwrap_or_default();
        }
        DatabaseKind::Redis => return Err("Redis 不支持关系表操作".into()),
    }
    Ok(s)
}
pub async fn run(
    c: &mut impl Access,
    kind: DatabaseKind,
    version: &str,
    r: &Request,
) -> Result<Reply, String> {
    // The saved target-version hint may be "Auto" or only a compatibility family.
    // Account capabilities must use the connected server's actual version.
    let account_version = if matches!(
        r.action,
        Action::AccountGrants { .. }
            | Action::AccountApplyGrants { .. }
            | Action::AccountMode
            | Action::AccountCreate { .. }
            | Action::AccountPermissions { .. }
    ) {
        if kind != DatabaseKind::MySql {
            return Err("accounts.unsupported".into());
        }
        Some(
            c.query("SELECT VERSION() AS version")
                .await?
                .values
                .first()
                .and_then(|row| row.first())
                .and_then(Clone::clone)
                .ok_or("accounts.prepare_failed")?,
        )
    } else {
        None
    };
    let version = account_version.as_deref().unwrap_or(version);
    match &r.action {
        Action::AccountGrants { user, host } => account_admin::grants(c, kind, version, user, host)
            .await
            .map(Reply::AccountGrants),
        Action::AccountApplyGrants { plan, confirmed } => {
            account_admin::apply_grants(c, kind, version, plan, *confirmed).await
        }
        Action::AccountMode => account_admin::mode(c, kind, version).await,
        Action::AccountCreate {
            draft,
            sql_mode,
            confirmed,
        } => account_admin::create(c, kind, version, draft, sql_mode, *confirmed).await,
        Action::AccountPermissions {
            user,
            host,
            search,
            offset,
        } => account_admin::permissions(c, kind, version, user, host, search, *offset).await,

        Action::Accounts { offset, limit } => {
            if !(1..=200).contains(limit) || *offset > i64::MAX as usize {
                return Err("browser.invalid".into());
            }
            let source = match kind {
                DatabaseKind::MySql => "SELECT User,Host FROM mysql.user ORDER BY User,Host",
                DatabaseKind::PostgreSql => {
                    "SELECT rolname FROM pg_catalog.pg_roles ORDER BY rolname"
                }
                DatabaseKind::ClickHouse => "SELECT name FROM system.users ORDER BY name",
                _ => return Err("browser.unsupported".into()),
            };
            let rows = c
                .query(&format!("{source} LIMIT {} OFFSET {offset}", limit + 1))
                .await?;
            let more = rows.values.len() > *limit;
            Ok(Reply::CatalogPage {
                objects: rows
                    .values
                    .iter()
                    .take(*limit)
                    .map(|row| Object {
                        name: if kind == DatabaseKind::MySql {
                            format!("{}@{}", value(row, 0), value(row, 1))
                        } else {
                            value(row, 0)
                        },
                        kind: "user".into(),
                        schema: value(row, 0),
                        parent: if kind == DatabaseKind::MySql {
                            value(row, 1)
                        } else {
                            String::new()
                        },
                        comment: String::new(),
                    })
                    .collect(),
                more,
            })
        }

        Action::MongoWrite(_)
        | Action::RedisScan { .. }
        | Action::RedisValue { .. }
        | Action::RedisWrite(_) => Err("此操作仅适用于 Redis".into()),
        Action::NoteGrid {
            plan,
            confirmed,
            action,
        } => {
            super::notebook_query::grid_request(
                c,
                kind,
                &r.database,
                version,
                plan,
                *confirmed,
                action,
            )
            .await
        }
        Action::NoteColumns(object) | Action::BrowseColumns(object) => {
            let object = if kind == DatabaseKind::PostgreSql && object.schema.is_empty() {
                super::notebook_query::resolved_object(
                    c,
                    kind,
                    &r.database,
                    std::slice::from_ref(&object.name),
                )
                .await?
            } else {
                object.clone()
            };
            schema(c, kind, &r.database, &object, false)
                .await
                .map(Reply::Schema)
        }
        Action::NoteExplain { sql } => super::notebook_query::explain(c, kind, sql)
            .await
            .map(Reply::NoteResult),
        Action::NotePrepare { sql } => {
            super::notebook_query::prepare(c, kind, &r.database, version, sql)
                .await
                .map(Reply::NotePrepared)
        }
        Action::ScriptDiagnostics => super::notebook_query::script_diagnostics(c, kind)
            .await
            .map(Reply::ScriptDiagnostics),
        Action::ScriptPrepare { sql } => {
            super::notebook_query::prepare_script(c, kind, &r.database, version, sql)
                .await
                .map(Reply::NotePrepared)
        }
        Action::NoteExecute {
            plan,
            confirmed,
            offset,
            page_size,
        } => super::notebook_query::execute(
            c,
            kind,
            &r.database,
            version,
            plan,
            *confirmed,
            (*offset, *page_size),
        )
        .await
        .map(Reply::NoteResult),
        Action::ScriptExecute { plan, confirmed } => {
            super::notebook_query::execute_script(c, kind, &r.database, version, plan, *confirmed)
                .await
                .map(Reply::NoteResult)
        }
        Action::GridPage { object, options } => {
            grid::page(c, kind, version, &r.database, object, options).await
        }
        Action::Count { object, filter } => {
            grid::count(c, kind, &r.database, object, filter.as_ref()).await
        }
        Action::Commit {
            object,
            ddl,
            changes,
        } => grid::commit(c, kind, &r.database, object, ddl, changes).await,
        Action::BrowseCatalog {
            category,
            search,
            offset,
            limit,
            include_comments,
        } => {
            super::catalog_browser::browse_with_comments(
                c,
                kind,
                version,
                &r.database,
                category,
                search,
                *offset,
                *limit,
                *include_comments,
            )
            .await
        }
        Action::BrowseSection { object, section } => {
            super::catalog_browser::section(c, kind, &r.database, object, section).await
        }
        Action::Catalog => catalog(c, kind, &r.database, version).await,
        Action::Ddl(o) => {
            if kind == DatabaseKind::MySql {
                let name = qualified(kind, &r.database, o);
                let rows = c
                    .query(&format!(
                        "SHOW CREATE {} {name}",
                        if o.kind == "view" { "VIEW" } else { "TABLE" }
                    ))
                    .await?;
                Ok(Reply::Schema(Schema {
                    ddl: rows
                        .values
                        .first()
                        .map(|row| value(row, 1))
                        .unwrap_or_default(),
                    ..Default::default()
                }))
            } else {
                Ok(Reply::Schema(Schema {
                    ddl: schema(c, kind, &r.database, o, true).await?.ddl,
                    ..Default::default()
                }))
            }
        }
        Action::Describe(o) => Ok(Reply::Schema(schema(c, kind, &r.database, o, true).await?)),
        Action::Page {
            object,
            offset,
            page_size,
        } => {
            if !(1..=MAX_GRID_PAGE_SIZE).contains(page_size) {
                return Err("每页行数必须在 1–1000 之间".into());
            }
            if *offset > 10_000_000 {
                return Err("分页偏移过大，请使用条件查询".into());
            }
            let s = schema(c, kind, &r.database, object, false).await?;
            let order = s
                .columns
                .iter()
                .filter(|c| c.primary)
                .map(|c| quote(kind, &c.name))
                .collect::<Vec<_>>();
            let sql = format!(
                "SELECT * FROM {}{} LIMIT {} OFFSET {offset}",
                qualified(kind, &r.database, object),
                if order.is_empty() {
                    String::new()
                } else {
                    format!(" ORDER BY {}", order.join(","))
                },
                page_size + 1
            );
            let mut rows = c.query_data(&sql).await?;
            if rows.columns.is_empty() {
                rows.columns = s.columns.iter().map(|c| c.name.clone()).collect();
            }
            let more = rows.values.len() > *page_size;
            rows.values.truncate(*page_size);
            Ok(Reply::Page {
                rows,
                offset: *offset,
                more,
                sql,
            })
        }
        Action::Execute { sql, guard } => {
            if sql.trim().is_empty() || sql.len() > 60_000 || sql.contains('\0') {
                return Err("SQL 为空或超过 60 KB".into());
            }
            if let Some((o, expected)) = guard
                && schema(c, kind, &r.database, o, true).await?.ddl != *expected
            {
                return Err("表结构已变化，请重新打开预览".into());
            }
            if kind == DatabaseKind::MySql {
                c.query(&format!("USE {}", quote(kind, &r.database)))
                    .await?;
            }
            c.query(sql).await?;
            Ok(Reply::Executed)
        }
    }
}

macro_rules! text_access {
    ($connection:ty) => {
        impl Access for $connection {
            fn query<'a>(&'a mut self, sql: &'a str) -> Query<'a> {
                Box::pin(async move { self.query_counted(sql).await.map(|(rows, _)| rows) })
            }
            fn directory_page<'a>(
                &'a mut self,
                sql: &'a str,
                offset: usize,
                limit: usize,
            ) -> Query<'a> {
                Box::pin(async move {
                    use futures_util::TryStreamExt;
                    use sqlx::Row;
                    let mut stream = sqlx::query(sql).fetch(self);
                    let mut result = Rows::default();
                    let mut seen = 0;
                    let mut bytes = 0;
                    while let Some(row) = stream.try_next().await.map_err(super::db_error)? {
                        if seen < offset {
                            seen += 1;
                            continue;
                        }
                        let name = row.try_get::<String, _>(0).map_err(super::db_error)?;
                        bytes += name.len();
                        if bytes > MAX_BYTES {
                            return Err("browser.response_failed".into());
                        }
                        result.values.push(vec![Some(name)]);
                        if result.values.len() > limit {
                            break;
                        }
                    }
                    Ok(result)
                })
            }

            fn query_counted<'a>(&'a mut self, sql: &'a str) -> CountedQuery<'a> {
                Box::pin(async move {
                    use futures_util::TryStreamExt;
                    use sqlx::{Column, Row};
                    let mut result = Rows::default();
                    let mut bytes = 0;
                    let mut affected = None;
                    let mut stream = sqlx::raw_sql(sql).fetch_many(self);
                    while let Some(entry) = stream.try_next().await.map_err(super::db_error)? {
                        let row = match entry {
                            sqlx::Either::Left(done) => {
                                affected = Some(done.rows_affected());
                                continue;
                            }
                            sqlx::Either::Right(row) => row,
                        };
                        if result.columns.is_empty() {
                            result.columns =
                                row.columns().iter().map(|c| c.name().into()).collect();
                        }
                        if row.len() > 512 {
                            return Err("预览列数超过 512".into());
                        }
                        let mut values = Vec::new();
                        for i in 0..row.len() {
                            let v = match row.try_get_unchecked::<Option<String>, _>(i) {
                                Ok(v) => v,
                                Err(_) => row
                                    .try_get_unchecked::<Option<Vec<u8>>, _>(i)
                                    .map_err(super::db_error)?
                                    .map(|bytes| {
                                        format!(
                                            "0x{}",
                                            bytes
                                                .iter()
                                                .map(|b| format!("{b:02x}"))
                                                .collect::<String>()
                                        )
                                    }),
                            };
                            bytes += v.as_ref().map_or(0, |s| s.len());
                            values.push(v);
                        }
                        if bytes > MAX_BYTES || result.values.len() >= 2000 {
                            return Err("结果超过预览上限，请缩小查询范围（单页 768 KiB）".into());
                        }
                        result.values.push(values);
                    }
                    Ok((result, affected))
                })
            }
        }
    };
}
text_access!(sqlx::MySqlConnection);
text_access!(sqlx::PgConnection);
impl Access for sqlx::SqliteConnection {
    fn query<'a>(&'a mut self, sql: &'a str) -> Query<'a> {
        Box::pin(async move { self.query_counted(sql).await.map(|(rows, _)| rows) })
    }
    fn query_counted<'a>(&'a mut self, sql: &'a str) -> CountedQuery<'a> {
        Box::pin(async move {
            use futures_util::TryStreamExt;
            use sqlx::{Column, Row, TypeInfo, ValueRef};
            let mut result = Rows::default();
            let mut bytes = 0;
            let mut affected = None;
            let mut stream = sqlx::raw_sql(sql).fetch_many(self);
            while let Some(entry) = stream.try_next().await.map_err(super::db_error)? {
                let row = match entry {
                    sqlx::Either::Left(done) => {
                        affected = Some(done.rows_affected());
                        continue;
                    }
                    sqlx::Either::Right(row) => row,
                };
                if result.columns.is_empty() {
                    result.columns = row.columns().iter().map(|c| c.name().into()).collect();
                }
                let mut values = Vec::new();
                for i in 0..row.len() {
                    let raw = row.try_get_raw(i).map_err(super::db_error)?;
                    let value = if raw.is_null() {
                        None
                    } else {
                        Some(match raw.type_info().name() {
                            "INTEGER" => row
                                .try_get::<i64, _>(i)
                                .map_err(super::db_error)?
                                .to_string(),
                            "REAL" => row
                                .try_get::<f64, _>(i)
                                .map_err(super::db_error)?
                                .to_string(),
                            "BLOB" => format!(
                                "0x{}",
                                row.try_get::<Vec<u8>, _>(i)
                                    .map_err(super::db_error)?
                                    .iter()
                                    .map(|b| format!("{b:02x}"))
                                    .collect::<String>()
                            ),
                            _ => row.try_get::<String, _>(i).map_err(super::db_error)?,
                        })
                    };
                    bytes += value.as_ref().map_or(0, |s| s.len());
                    values.push(value);
                }
                if bytes > MAX_BYTES || result.values.len() >= 2000 {
                    return Err("结果超过预览上限（768 KiB）".into());
                }
                result.values.push(values);
            }
            Ok((result, affected))
        })
    }
}
fn mysql_literal_parts(value: &str) -> (String, &'static str) {
    let hex = value
        .as_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let charset = if value.chars().any(|c| c as u32 > 0xffff) {
        "utf8mb4"
    } else {
        "utf8"
    };
    (hex, charset)
}
/// A real constant string item lets MySQL 5.x prune information_schema by schema
/// and table. Function expressions such as CONVERT can force full catalog opens.
/// The introducer still avoids client-charset and NO_BACKSLASH_ESCAPES ambiguity.
pub(super) fn metadata_literal(kind: DatabaseKind, value: &str) -> String {
    if kind == DatabaseKind::MySql {
        let (hex, charset) = mysql_literal_parts(value);
        format!("_{charset} X'{hex}'")
    } else {
        literal_for(kind, value)
    }
}
/// Hex string expressions avoid dependence on MySQL NO_BACKSLASH_ESCAPES.
pub fn literal_for(kind: DatabaseKind, value: &str) -> String {
    if kind == DatabaseKind::MySql {
        let (hex, charset) = mysql_literal_parts(value);
        format!("CONVERT(X'{hex}' USING {charset})")
    } else if kind == DatabaseKind::ClickHouse {
        format!(
            "unhex('{}')",
            value
                .as_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        )
    } else if kind == DatabaseKind::PostgreSql {
        let mut tag = "$turbodbnote$".to_string();
        while value.contains(&tag) {
            tag.insert(1, '_');
        }
        format!("{tag}{value}{tag}")
    } else {
        literal(value)
    }
}
pub fn export_csv(rows: &Rows) -> Result<String, String> {
    let mut writer = csv::Writer::from_writer(Vec::new());
    writer
        .write_record(&rows.columns)
        .map_err(|e| e.to_string())?;
    for row in &rows.values {
        writer
            .write_record(row.iter().map(|v| match v {
                None => "\\N".to_string(),
                Some(s) if s.starts_with('\\') => format!("\\{s}"),
                Some(s) => s.clone(),
            }))
            .map_err(|e| e.to_string())?;
    }
    String::from_utf8(writer.into_inner().map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}
/// Bounded import preview, no IO or database writes. NULL=\N; initial slash is escaped.
pub fn import_csv(
    kind: DatabaseKind,
    database: &str,
    object: &Object,
    columns: &[Column],
    text: &str,
) -> Result<String, String> {
    if text.len() > 48_000 {
        return Err("CSV 导入预览最多 48 KB / 100 行，请拆分文件".into());
    }
    let mut reader = csv::Reader::from_reader(text.as_bytes());
    let headers = reader.headers().map_err(|e| e.to_string())?.clone();
    if headers.is_empty() || headers.iter().any(|h| !columns.iter().any(|c| c.name == h)) {
        return Err("CSV 表头必须匹配目标表列名".into());
    }
    let mut seen = std::collections::HashSet::new();
    if headers.iter().any(|h| !seen.insert(h)) {
        return Err("CSV 表头包含重复列名".into());
    }
    let mut values = Vec::new();
    for record in reader.records() {
        if values.len() >= 100 {
            return Err("CSV 导入预览最多 100 行，请拆分文件".into());
        }
        let record = record.map_err(|e| e.to_string())?;
        values.push(format!(
            "({})",
            record
                .iter()
                .map(|v| if v == "\\N" {
                    "NULL".into()
                } else {
                    literal_for(kind, v.strip_prefix("\\\\").map(|_| &v[1..]).unwrap_or(v))
                })
                .collect::<Vec<_>>()
                .join(",")
        ));
    }
    if values.is_empty() {
        return Err("CSV 没有数据行".into());
    }
    let sql = format!(
        "INSERT INTO {} ({}) VALUES\n{};",
        qualified(kind, database, object),
        headers
            .iter()
            .map(|h| quote(kind, h))
            .collect::<Vec<_>>()
            .join(","),
        values.join(",\n")
    );
    if sql.len() > 60_000 {
        return Err("生成 SQL 超过 60 KB，请拆分文件".into());
    }
    Ok(sql)
}

pub fn categories(kind: DatabaseKind, version: &str) -> Vec<&'static str> {
    match kind {
        DatabaseKind::MySql if version.starts_with("4.") => vec!["table", "index"],
        DatabaseKind::MySql => vec![
            "table",
            "view",
            "index",
            "trigger",
            "procedure",
            "function",
            "event",
        ],
        DatabaseKind::PostgreSql => {
            vec!["table", "view", "index", "trigger", "procedure", "function"]
        }
        DatabaseKind::Sqlite => vec!["table", "view", "index", "trigger"],
        DatabaseKind::ClickHouse => vec!["table", "view", "index"],
        DatabaseKind::Redis => vec![],
        DatabaseKind::External => vec!["table", "view"],
    }
}
pub fn create_template(kind: DatabaseKind, db: &str, table: &Object, category: &str) -> String {
    let target = qualified(kind, db, table);
    let q = |n: &str| quote(kind, n);
    match category {
        "table" if kind == DatabaseKind::ClickHouse => format!(
            "CREATE TABLE {}.{} (id UInt64, name String) ENGINE=MergeTree ORDER BY id;",
            q(db),
            q("new_table")
        ),
        "table" => format!(
            "CREATE TABLE {} (id INTEGER PRIMARY KEY, name VARCHAR(255));",
            if kind == DatabaseKind::PostgreSql {
                format!("{}.{}", q(&table.schema), q("new_table"))
            } else {
                q("new_table")
            }
        ),
        "view" => format!("CREATE VIEW {} AS SELECT * FROM {target};", q("new_view")),
        "index" if kind == DatabaseKind::ClickHouse => {
            format!("ALTER TABLE {target} ADD INDEX new_index id TYPE minmax GRANULARITY 1;")
        }
        "index" => format!("CREATE INDEX {} ON {target} (id);", q("new_index")),
        "trigger" if kind == DatabaseKind::Sqlite => format!(
            "CREATE TRIGGER {} AFTER INSERT ON {target} BEGIN SELECT 1; END;",
            q("new_trigger")
        ),
        "trigger" if kind == DatabaseKind::PostgreSql => format!(
            "CREATE FUNCTION {}.new_trigger_fn() RETURNS trigger LANGUAGE plpgsql AS $body$ BEGIN RETURN NEW; END; $body$;\nCREATE TRIGGER new_trigger AFTER INSERT ON {target} FOR EACH ROW EXECUTE FUNCTION {}.new_trigger_fn();",
            q(&table.schema),
            q(&table.schema)
        ),
        "trigger" => format!(
            "CREATE TRIGGER {} AFTER INSERT ON {target} FOR EACH ROW SET @turbodbnote_trigger = 1;",
            q("new_trigger")
        ),
        "procedure" if kind == DatabaseKind::PostgreSql => {
            "CREATE PROCEDURE new_procedure() LANGUAGE SQL AS 'SELECT 1';".into()
        }
        "function" if kind == DatabaseKind::PostgreSql => {
            "CREATE FUNCTION new_function() RETURNS INTEGER LANGUAGE SQL AS 'SELECT 1';".into()
        }
        "procedure" => "CREATE PROCEDURE new_procedure() SELECT 1;".into(),
        "function" => {
            "CREATE FUNCTION new_function() RETURNS INTEGER DETERMINISTIC RETURN 1;".into()
        }
        "event" => "CREATE EVENT new_event ON SCHEDULE EVERY 1 DAY DO SELECT 1;".into(),
        _ => String::new(),
    }
}
#[allow(clippy::too_many_arguments)]
pub fn change_column(
    kind: DatabaseKind,
    db: &str,
    object: &Object,
    old: &Column,
    ddl: &str,
    name: &str,
    data_type: &str,
    comment: &str,
) -> Result<String, String> {
    let name = name.trim();
    let data_type = data_type.trim();
    if name.is_empty() || name.chars().count() > 64 || name.contains('\0') {
        return Err("列名为空、过长或包含空字符".into());
    }
    // The form accepts a type, not arbitrary SQL. Complex pre-existing types remain unchanged.
    if data_type != old.data_type
        && (data_type.is_empty()
            || data_type.len() > 128
            || !data_type
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_ (),[]".contains(c)))
    {
        return Err("复杂类型请保留原类型，在 SQL 预览中修改".into());
    }
    let target = qualified(kind, db, object);
    let q = |n: &str| quote(kind, n);
    if kind == DatabaseKind::MySql {
        let prefix = q(&old.name);
        let line = ddl
            .lines()
            .map(str::trim)
            .find(|l| l.starts_with(&format!("{prefix} ")))
            .ok_or("无法从服务端 DDL 提取完整列定义")?;
        let rest = line[prefix.len()..].trim().trim_end_matches(',');
        if !rest
            .to_lowercase()
            .starts_with(&old.data_type.to_lowercase())
        {
            return Err("列类型与服务端 DDL 不一致，请刷新结构".into());
        }
        let mut tail = rest[old.data_type.len()..].to_string();
        if comment != old.comment {
            // Comments can contain quotes. Locate only a top-level COMMENT token, never text inside a default expression.
            if let Some((start, end)) = comment_clause(&tail)? {
                tail.replace_range(start..end, "");
            }
            // SHOW CREATE uses backslash escaping. Reject backslashes in a new comment rather than change session SQL mode.
            if comment.contains('\\') || comment.contains('\0') {
                return Err("注释暂不接受反斜杠或空字符，请在 SQL 预览中按服务端模式填写".into());
            }
            tail.push_str(&format!(" COMMENT {}", literal(comment)));
        }
        return Ok(format!(
            "ALTER TABLE {target} CHANGE COLUMN {prefix} {} {data_type}{tail};",
            q(name)
        ));
    }
    let mut statements = Vec::new();
    if name != old.name {
        statements.push(format!(
            "ALTER TABLE {target} RENAME COLUMN {} TO {};",
            q(&old.name),
            q(name)
        ));
    }
    if data_type != old.data_type {
        if kind == DatabaseKind::Sqlite {
            return Err("SQLite 类型修改需要重建表，此编辑器仅支持重命名".into());
        }
        statements.push(if kind == DatabaseKind::PostgreSql {
            format!(
                "ALTER TABLE {target} ALTER COLUMN {} TYPE {data_type};",
                q(name)
            )
        } else {
            format!(
                "ALTER TABLE {target} MODIFY COLUMN {} {data_type};",
                q(name)
            )
        });
    }
    if comment != old.comment {
        match kind {
            DatabaseKind::PostgreSql => statements.push(format!(
                "COMMENT ON COLUMN {target}.{} IS {};",
                q(name),
                literal_for(kind, comment)
            )),
            DatabaseKind::ClickHouse => statements.push(format!(
                "ALTER TABLE {target} COMMENT COLUMN {} {};",
                q(name),
                clickhouse_comment(comment)
            )),
            _ => return Err("此数据库不支持列注释".into()),
        }
    }
    if statements.is_empty() {
        return Err("没有修改列定义".into());
    }
    // ClickHouse HTTP permits one statement per request. Keep multi-step edits explicit.
    if kind == DatabaseKind::ClickHouse && statements.len() > 1 {
        return Err("ClickHouse 请一次修改一个属性".into());
    }
    Ok(statements.join("\n"))
}
fn clickhouse_comment(value: &str) -> String {
    format!(
        "'{}'",
        value
            .replace('\\', "\\\\")
            .replace('\'', "\\'")
            .replace('\n', "\\n")
            .replace('\0', "\\0")
    )
}
pub(super) fn comment_clause(sql: &str) -> Result<Option<(usize, usize)>, String> {
    let bytes = sql.as_bytes();
    let mut i = 0;
    let mut quote = None;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = quote {
            if b == b'\\' {
                i += 2;
                continue;
            }
            if b == q {
                if bytes.get(i + 1) == Some(&q) {
                    i += 2;
                    continue;
                }
                quote = None;
            }
            i += 1;
            continue;
        }
        if b == b'\'' || b == b'"' || b == b'`' {
            quote = Some(b);
            i += 1;
            continue;
        }
        if (i == 0 || bytes[i - 1].is_ascii_whitespace())
            && bytes
                .get(i..i + 7)
                .is_some_and(|s| s.eq_ignore_ascii_case(b"COMMENT"))
            && bytes.get(i + 7).is_some_and(u8::is_ascii_whitespace)
        {
            let start = i;
            i += 7;
            while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
                i += 1;
            }
            if bytes.get(i) != Some(&b'\'') {
                return Err("未知的 COMMENT 定义".into());
            }
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i += 2;
                    continue;
                }
                if bytes[i] == b'\'' {
                    if bytes.get(i + 1) == Some(&b'\'') {
                        i += 2;
                        continue;
                    }
                    return Ok(Some((start, i + 1)));
                }
                i += 1;
            }
            return Err("注释字符串未闭合".into());
        }
        i += 1;
    }
    Ok(None)
}

/// Compile the same filter used by data pages without executing a query.
pub fn filter_select(
    kind: DatabaseKind,
    db: &str,
    object: &Object,
    schema: &Schema,
    filter: Option<&FilterSet>,
) -> Result<String, String> {
    let parts = grid::readable_conditions(kind, schema, filter)?;
    Ok(format!(
        "SELECT * FROM {}{};",
        qualified(kind, db, object),
        if parts.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", parts.join(" AND "))
        }
    ))
}

#[cfg(test)]
#[path = "workbench_tests.rs"]
mod tests;

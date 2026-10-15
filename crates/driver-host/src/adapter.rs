use crate::{
    document::{Cassandra, Http},
    protocol::*,
    relational::Relational,
};
use futures_util::TryStreamExt;
use mongodb::bson::doc;
use serde_json::{Value, json};
enum Backend {
    Relational(Relational),
    Http(Http),
    Mongo(mongodb::Client),
    Cassandra(Box<Cassandra>),
}
pub struct Adapter {
    pub p: Value,
    pub db: String,
    pub version: String,
    driver: String,
    backend: Backend,
}
fn at(row: &[Option<String>], n: usize) -> &str {
    row.get(n).and_then(|v| v.as_deref()).unwrap_or("")
}
impl Adapter {
    pub async fn connect(p: Value, password: &str) -> Result<Self> {
        validate_profile(&p)?;
        let driver = string(&p, "driver_id").to_owned();
        let backend = match driver.as_str() {
            "access" | "sqlserver" | "oracle" | "db2" | "odbc" => {
                Backend::Relational(Relational::connect(&p, password).await?)
            }
            "elasticsearch" | "tdengine" => Backend::Http(Http::new(&p, password)?),
            "mongodb" => Backend::Mongo(crate::document::mongo(&p, password).await?),
            "cassandra" => Backend::Cassandra(Box::new(Cassandra::connect(&p, password).await?)),
            _ => return Err("drivers.invalid_identity"),
        };
        let db = if driver == "odbc" {
            if let Backend::Relational(Relational::Odbc(c)) = &backend {
                c.current_catalog()
                    .ok()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| {
                        let dsn = string(&p["odbc"], "dsn");
                        if dsn.is_empty() {
                            "ODBC".into()
                        } else {
                            dsn.into()
                        }
                    })
            } else {
                return Err(FAILED);
            }
        } else if driver == "access" {
            std::path::Path::new(string(&p, "sqlite_path"))
                .file_name()
                .ok_or(FAILED)?
                .to_string_lossy()
                .into_owned()
        } else {
            string(&p, "database").into()
        };
        let mut a = Self {
            p,
            db,
            version: driver.clone(),
            driver,
            backend,
        };
        a.ping().await?;
        Ok(a)
    }
    async fn query(&mut self, sql: &str, maximum: usize) -> Result<Rows> {
        match &mut self.backend {
            Backend::Relational(c) => {
                c.query(sql, maximum, execution_seconds(&self.p) as usize)
                    .await
            }
            Backend::Http(c) if self.driver == "tdengine" => c.sql(&self.db, sql, maximum).await,
            Backend::Cassandra(c) => c.query_bounded(sql, maximum).await,
            _ => Err(UNSUPPORTED),
        }
    }
    pub async fn ping(&mut self) -> Result<()> {
        match &self.backend {
            Backend::Mongo(c) => {
                c.database(crate::document::mongo_ping_database(&self.p))
                    .run_command(doc! {"ping":1})
                    .await
                    .map_err(crate::document::mongo_error)?;
            }
            Backend::Http(c) if self.driver == "elasticsearch" => {
                let v = c.request(&[], None, "").await?;
                self.version = v["version"]["number"]
                    .as_str()
                    .unwrap_or("Elasticsearch")
                    .into();
            }
            _ => {
                let sql = match self.driver.as_str() {
                    "access" | "odbc" => return Ok(()),
                    "oracle" => "SELECT 1 FROM DUAL",
                    "db2" => "SELECT 1 FROM SYSIBM.SYSDUMMY1",
                    "cassandra" => "SELECT release_version FROM system.local",
                    "tdengine" => "SELECT SERVER_VERSION()",
                    _ => "SELECT 1",
                };
                self.query(sql, 1).await?;
            }
        }
        Ok(())
    }
    pub async fn databases(&mut self) -> Result<Vec<String>> {
        if !self.db.is_empty() && !self.p["list_all_databases"].as_bool().unwrap_or(false) {
            return Ok(vec![self.db.clone()]);
        }
        match &self.backend {
            Backend::Mongo(c) => Ok(c
                .list_database_names()
                .authorized_databases(true)
                .await
                .map_err(crate::document::mongo_error)?
                .into_iter()
                .take(512)
                .collect()),
            Backend::Http(c) if self.driver == "elasticsearch" => Ok(c
                .request(&["_cat", "indices"], None, "")
                .await?
                .as_array()
                .ok_or(FAILED)?
                .iter()
                .take(512)
                .map(|v| string(v, "index").to_owned())
                .collect()),
            _ => {
                let sql = match self.driver.as_str() {
                    "sqlserver" => {
                        "SELECT TOP 512 name FROM sys.databases WHERE HAS_DBACCESS(name)=1"
                    }
                    "tdengine" => "SHOW DATABASES",
                    "cassandra" => "SELECT keyspace_name FROM system_schema.keyspaces LIMIT 512",
                    _ => {
                        return Ok(if self.db.is_empty() {
                            vec![]
                        } else {
                            vec![self.db.clone()]
                        });
                    }
                };
                Ok(self
                    .query(sql, 512)
                    .await?
                    .values
                    .iter()
                    .map(|r| at(r, 0).into())
                    .collect())
            }
        }
    }
    async fn switch(&mut self, db: &str) -> Result<()> {
        if db == self.db {
            return Ok(());
        }
        match self.driver.as_str() {
            "sqlserver" => {
                self.query(&format!("USE {}", quote(db, &self.driver)?), 1)
                    .await?;
            }
            "mongodb" | "elasticsearch" | "cassandra" | "tdengine" => {}
            _ => return Err("drivers.switch_unsupported"),
        }
        self.db = db.into();
        Ok(())
    }
    async fn catalog(&mut self, db: &str) -> Result<Vec<Value>> {
        self.switch(db).await?;
        let mut objects = Vec::new();
        match &self.backend {
            Backend::Mongo(c) => {
                let mut cursor = c
                    .database(db)
                    .list_collections()
                    .await
                    .map_err(|_| FAILED)?;
                while objects.len() < 500 {
                    let Some(v) = cursor.try_next().await.map_err(|_| FAILED)? else {
                        break;
                    };
                    objects.push(object(
                        "",
                        &v.name,
                        if matches!(v.collection_type, mongodb::results::CollectionType::View) {
                            "view"
                        } else {
                            "table"
                        },
                    ));
                }
                return Ok(objects);
            }
            Backend::Http(_) if self.driver == "elasticsearch" => {
                return Ok(self
                    .databases()
                    .await?
                    .iter()
                    .take(500)
                    .map(|n| object("", n, "table"))
                    .collect());
            }
            Backend::Relational(c) if ["access", "odbc"].contains(&self.driver.as_str()) => {
                for r in c.odbc_catalog(self.driver == "odbc")?.values {
                    if !at(&r, 2).starts_with("MSys") {
                        objects.push(object(
                            at(&r, 1),
                            at(&r, 2),
                            if at(&r, 3) == "VIEW" { "view" } else { "table" },
                        ));
                    }
                }
                objects.truncate(500);
                return Ok(objects);
            }
            _ => {}
        }
        let sql=match self.driver.as_str(){
            "sqlserver"=>"SELECT TOP 501 TABLE_SCHEMA,TABLE_NAME,CASE TABLE_TYPE WHEN 'VIEW' THEN 'view' ELSE 'table' END FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA NOT IN ('INFORMATION_SCHEMA','sys')".into(),
            "oracle"=>"SELECT OWNER,OBJECT_NAME,LOWER(OBJECT_TYPE) FROM ALL_OBJECTS WHERE OBJECT_TYPE IN ('TABLE','VIEW') AND OWNER=USER AND ROWNUM<=501".into(),
            "db2"=>"SELECT TABSCHEMA,TABNAME,CASE TYPE WHEN 'V' THEN 'view' ELSE 'table' END FROM SYSCAT.TABLES WHERE TABSCHEMA NOT LIKE 'SYS%' FETCH FIRST 501 ROWS ONLY".into(),
            "cassandra"=>format!("SELECT table_name FROM system_schema.tables WHERE keyspace_name={} LIMIT 501",literal(db)),
            "tdengine"=>"SHOW TABLES".into(),_=>return Err(UNSUPPORTED)
        };
        for row in self.query(&sql, 501).await?.values.into_iter().take(500) {
            objects.push(if self.driver == "cassandra" {
                object(db, at(&row, 0), "table")
            } else if self.driver == "tdengine" {
                object("", at(&row, 0), "table")
            } else {
                object(at(&row, 0), at(&row, 1), at(&row, 2))
            });
        }
        Ok(objects)
    }
    async fn schema(&mut self, obj: &Value, ddl: bool) -> Result<Value> {
        if matches!(self.backend, Backend::Mongo(_)) || self.driver == "elasticsearch" {
            return Ok(schema(vec![column("document", "JSON", "")], String::new()));
        }
        let name = qualified(obj, &self.driver)?;
        let table = literal(string(obj, "name"));
        let owner = literal(string(obj, "schema"));
        let columns = if ["access", "odbc"].contains(&self.driver.as_str()) {
            if let Backend::Relational(c) = &self.backend {
                c.odbc_columns(string(obj, "schema"), string(obj, "name"))?
                    .values
                    .iter()
                    .filter(|r| {
                        at(r, 2) == string(obj, "name")
                            && (string(obj, "schema").is_empty()
                                || at(r, 1) == string(obj, "schema"))
                    })
                    .map(|r| column(at(r, 3), at(r, 5), at(r, 11)))
                    .collect()
            } else {
                return Err(FAILED);
            }
        } else {
            let sql = match self.driver.as_str() {
                "sqlserver" => format!(
                    "SELECT COLUMN_NAME,DATA_TYPE,'' FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_SCHEMA={owner} AND TABLE_NAME={table} ORDER BY ORDINAL_POSITION"
                ),
                "oracle" => format!(
                    "SELECT COLUMN_NAME,DATA_TYPE,'' FROM ALL_TAB_COLUMNS WHERE OWNER={} AND TABLE_NAME={table} ORDER BY COLUMN_ID",
                    if string(obj, "schema").is_empty() {
                        "USER"
                    } else {
                        &owner
                    }
                ),
                "db2" => format!(
                    "SELECT COLNAME,TYPENAME,COALESCE(REMARKS,'') FROM SYSCAT.COLUMNS WHERE TABSCHEMA={owner} AND TABNAME={table} ORDER BY COLNO"
                ),
                "cassandra" => format!(
                    "SELECT column_name,type,kind FROM system_schema.columns WHERE keyspace_name={} AND table_name={table}",
                    literal(&self.db)
                ),
                "tdengine" => format!("DESCRIBE {name}"),
                _ => return Err(UNSUPPORTED),
            };
            self.query(&sql, 512)
                .await?
                .values
                .iter()
                .map(|r| column(at(r, 0), at(r, 1), at(r, 2)))
                .collect::<Vec<_>>()
        };
        let mut text = String::new();
        if ddl {
            if self.driver == "oracle" {
                let sql = format!(
                    "SELECT DBMS_METADATA.GET_DDL({},{},{}) FROM DUAL",
                    literal(&string(obj, "kind").to_uppercase()),
                    table,
                    if string(obj, "schema").is_empty() {
                        "USER"
                    } else {
                        &owner
                    }
                );
                text = self
                    .query(&sql, 1)
                    .await?
                    .values
                    .first()
                    .map(|r| at(r, 0).to_owned())
                    .unwrap_or_default();
            } else {
                text = "-- Metadata preview only; constraints/indexes/defaults are not included.\n"
                    .into();
                for c in &columns {
                    text.push_str(&format!(
                        "-- {} {}\n",
                        quote(string(c, "name"), &self.driver)?,
                        string(c, "data_type")
                    ));
                }
            }
        }
        Ok(schema(columns, text))
    }
    async fn page(&mut self, obj: &Value, options: &Value) -> Result<Value> {
        let started = std::time::Instant::now();
        let (size, offset) = page_options(options)?;
        if let Backend::Mongo(c) = &self.backend {
            let page =
                crate::document::mongo_page(c, &self.db, string(obj, "name"), size, offset).await?;
            return Ok(json!({"MongoPage":page}));
        }
        if self.driver == "odbc"
            && let Backend::Relational(c) = &mut self.backend
        {
            let rows = c.odbc_page(obj, size + 1, offset, execution_seconds(&self.p) as usize)?;
            let more = rows.values.len() > size;
            let mut rows = rows;
            rows.values.truncate(size);
            let metadata = self.schema(obj, false).await?;
            return Ok(
                json!({"GridPage":{"rows":rows.json(),"schema":metadata,"offset":offset,"more":more,"sql":"","next":null,"elapsed_ms":started.elapsed().as_millis() as u64,"keyset":false}}),
            );
        }
        let mut cursor_more = None;
        let mut rows = match &mut self.backend {
            Backend::Http(c) if self.driver == "elasticsearch" => {
                if offset + size as u64 + 1 > 10000 {
                    return Err(TOO_LARGE);
                }
                let v=c.request(&[string(obj,"name"),"_search"],Some(json!({"query":{"match_all":{}},"from":offset,"size":size+1,"track_total_hits":false}).to_string()),"application/json").await?;
                let mut rows = Rows::new(vec!["document".into()])?;
                for h in v["hits"]["hits"].as_array().ok_or(FAILED)? {
                    rows.push(vec![Some(h.to_string())])?;
                }
                rows
            }
            Backend::Cassandra(c) => {
                let sql = format!("SELECT * FROM {}", qualified(obj, &self.driver)?);
                let (rows, more) = c.page(&sql, size, offset).await?;
                cursor_more = Some(more);
                rows
            }
            _ => {
                let name = qualified(obj, &self.driver)?;
                let sql = match self.driver.as_str() {
                    "access" => {
                        if offset != 0 {
                            return Err("drivers.offset_unsupported");
                        }
                        format!("SELECT TOP {} * FROM {name}", size + 1)
                    }
                    "sqlserver" | "oracle" | "db2" => format!(
                        "SELECT * FROM {name}{} OFFSET {offset} ROWS FETCH NEXT {} ROWS ONLY",
                        if self.driver == "sqlserver" {
                            " ORDER BY (SELECT NULL)"
                        } else {
                            ""
                        },
                        size + 1
                    ),
                    _ => format!("SELECT * FROM {name} LIMIT {} OFFSET {offset}", size + 1),
                };
                self.query(&sql, size + 1).await?
            }
        };
        let more = cursor_more.unwrap_or(rows.values.len() > size && self.driver != "access");
        rows.values.truncate(size);
        let metadata = self.schema(obj, false).await?;
        let columns = rows
            .columns
            .iter()
            .map(|name| {
                metadata["columns"]
                    .as_array()
                    .and_then(|v| v.iter().find(|c| string(c, "name") == name))
                    .cloned()
                    .unwrap_or_else(|| column(name, "", ""))
            })
            .collect();
        Ok(
            json!({"GridPage":{"rows":rows.json(),"schema":schema(columns,String::new()),"offset":offset,"more":more,"sql":"","next":null,"elapsed_ms":started.elapsed().as_millis() as u64,"keyset":false}}),
        )
    }
    pub async fn operation(&mut self, kind: &Value) -> Result<Value> {
        allowed(kind)?;
        if kind == "Databases" {
            return Ok(json!({"Databases":self.databases().await?}));
        }
        if let Some(v) = kind.get("Objects") {
            let items = self.catalog(string(v, "database")).await?;
            let notice = if items.len() == 500 {
                Some("drivers.catalog_limit")
            } else {
                None
            };
            return Ok(json!({"Objects":{"items":items,"cursor":null,"notice":notice}}));
        }
        let request = &kind["Workbench"];
        let action = &request["action"];
        if action == "Catalog" {
            let objects = self.catalog(string(request, "database")).await?;
            let notice = if objects.len() == 500 {
                "drivers.catalog_limit"
            } else {
                ""
            };
            return Ok(json!({"Workbench":{"Catalog":{"objects":objects,"notice":notice}}}));
        }
        self.switch(string(request, "database")).await?;
        let result = if let Some(v) = action.get("MongoWrite") {
            if self.p["mongo_read_only"].as_bool() != Some(false) {
                return Err("mongo.document.denied");
            }
            let Backend::Mongo(c) = &self.backend else {
                return Err(UNSUPPORTED);
            };
            let change: turbodbn_core::mongo::Mutation =
                serde_json::from_value(v.clone()).map_err(|_| "mongo.document.invalid")?;
            let inserted = crate::document::mongo_write(c, &self.db, &change).await?;
            json!({"MongoWritten":{"inserted":inserted}})
        } else if let Some(v) = action.get("GridPage") {
            self.page(&v["object"], &v["options"]).await?
        } else if let Some(v) = action.get("Count") {
            let obj = &v["object"];
            let count = match &self.backend {
                Backend::Mongo(c) => c
                    .database(&self.db)
                    .collection::<mongodb::bson::Document>(string(obj, "name"))
                    .count_documents(doc! {})
                    .await
                    .map_err(|_| FAILED)?,
                Backend::Http(c) if self.driver == "elasticsearch" => c
                    .request(&[string(obj, "name"), "_count"], None, "")
                    .await?["count"]
                    .as_u64()
                    .ok_or(FAILED)?,
                _ => {
                    let sql = format!("SELECT COUNT(*) FROM {}", qualified(obj, &self.driver)?);
                    self.query(&sql, 1)
                        .await?
                        .values
                        .first()
                        .ok_or(FAILED)
                        .and_then(|r| at(r, 0).parse::<u64>().map_err(|_| FAILED))?
                }
            };
            json!({"Count":count})
        } else {
            let key = ["Describe", "Ddl", "NoteColumns"]
                .into_iter()
                .find(|k| action.get(*k).is_some())
                .ok_or(UNSUPPORTED)?;
            json!({"Schema":self.schema(&action[key],key!="NoteColumns").await?})
        };
        Ok(json!({"Workbench":result}))
    }
}

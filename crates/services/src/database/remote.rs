use super::*;
use serde_json::Value;
fn network(error: impl std::error::Error + 'static) -> String {
    if let Some(issue) = mysql::tls_cause(&error) {
        return issue.message().into();
    }
    "连接或请求失败，请检查地址、认证、TLS 配置及权限".into()
}
fn read_file(path: &str) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|_| "无法读取证书文件".into())
}
pub(super) struct HttpDatabase {
    client: reqwest::Client,
    url: reqwest::Url,
    user: String,
    password: String,
}
impl HttpDatabase {
    pub async fn open(p: &Profile, password: &str) -> Result<(Self, ServerInfo), String> {
        let begin = Instant::now();
        let mut url = reqwest::Url::parse(if p.tls == TlsMode::Disabled {
            "http://localhost"
        } else {
            "https://localhost"
        })
        .map_err(network)?;
        url.set_host(Some(&p.host))
            .map_err(|_| "ClickHouse 主机名无效")?;
        url.set_port(Some(p.port)).map_err(|_| "端口无效")?;
        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(p.timeout_seconds))
            .danger_accept_invalid_certs(p.tls == TlsMode::Required);
        if !p.ca.is_empty() {
            builder = builder.add_root_certificate(
                reqwest::Certificate::from_pem(&read_file(&p.ca)?)
                    .map_err(|_| "CA 证书格式无效")?,
            );
        }
        if !p.client_cert.is_empty() {
            let mut pem = read_file(&p.client_cert)?;
            pem.extend_from_slice(b"\n");
            pem.extend(read_file(&p.client_key)?);
            builder = builder.identity(
                reqwest::Identity::from_pem(&pem).map_err(|_| "客户端证书/私钥格式无效")?,
            );
        }
        let this = Self {
            client: builder.build().map_err(network)?,
            url,
            user: p.user.clone(),
            password: password.into(),
        };
        let rows = this
            .query(
                "SELECT version() AS version FORMAT JSONEachRow",
                &p.database,
                &[],
            )
            .await?;
        let version = rows
            .first()
            .and_then(|v| v["version"].as_str())
            .ok_or("ClickHouse 未返回版本")?
            .to_string();
        let databases = if p.database.is_empty() {
            this.databases().await?
        } else {
            vec![p.database.clone()]
        };
        Ok((
            this,
            info(
                version,
                "UTF8",
                (!p.database.is_empty()).then(|| p.database.clone()),
                databases,
                if p.tls == TlsMode::Disabled {
                    ""
                } else {
                    "HTTPS / TLS"
                },
                begin.elapsed().as_millis(),
            ),
        ))
    }
    async fn query(
        &self,
        sql: &str,
        database: &str,
        params: &[(&str, &str)],
    ) -> Result<Vec<Value>, String> {
        let mut request = self
            .client
            .post(self.url.clone())
            .basic_auth(&self.user, Some(&self.password))
            .query(params)
            .body(sql.to_string());
        if !database.is_empty() {
            request = request.query(&[("database", database)]);
        }
        let mut response = request.send().await.map_err(network)?;
        if !response.status().is_success() {
            return Err(format!(
                "ClickHouse 请求失败（HTTP {}），请检查权限、配置与对象名称",
                response.status().as_u16()
            ));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(network)? {
            if bytes.len() + chunk.len() > 1024 * 1024 {
                return Err("ClickHouse 元数据响应超过 1 MiB".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        let text = String::from_utf8(bytes).map_err(|_| "ClickHouse 响应不是 UTF-8")?;
        text.lines()
            .filter(|s| !s.trim().is_empty())
            .map(|s| serde_json::from_str(s).map_err(|_| "ClickHouse 元数据格式错误".into()))
            .collect()
    }
    async fn databases(&self) -> Result<Vec<String>, String> {
        Ok(self
            .query("SHOW DATABASES FORMAT JSONEachRow", "", &[])
            .await?
            .into_iter()
            .take(512)
            .filter_map(|v| {
                v["name"]
                    .as_str()
                    .or_else(|| v["Database"].as_str())
                    .map(String::from)
            })
            .collect())
    }
    pub async fn ping(&self) -> Result<(), String> {
        self.query("SELECT 1 FORMAT JSONEachRow", "", &[])
            .await
            .map(|_| ())
    }
    pub async fn operation(&self, p: &Profile, op: &Operation) -> Result<OperationResult, String> {
        match &op.kind {
            OperationKind::Workbench(r) => {
                let mut access = HttpAccess {
                    client: self,
                    database: &r.database,
                };
                super::workbench::run(&mut access, p.kind, "", r)
                    .await
                    .map(|r| OperationResult::Workbench(Box::new(r)))
            }
            OperationKind::Databases => Ok(OperationResult::Databases(if p.database.is_empty() {
                self.databases().await?
            } else {
                vec![p.database.clone()]
            })),
            OperationKind::CreateDatabase { name } => {
                validate_name(p.kind, name)?;
                self.query(
                    "CREATE DATABASE {new_database:Identifier}",
                    "",
                    &[("param_new_database", name)],
                )
                .await?;
                Ok(OperationResult::Created(name.clone()))
            }
            OperationKind::Objects { database, .. } => {
                let rows=self.query("SELECT name,engine FROM system.tables WHERE database={db:String} ORDER BY name LIMIT 501 FORMAT JSONEachRow","",&[("param_db",database)]).await?;
                Ok(objects(
                    rows.into_iter()
                        .map(|v| {
                            ObjectItem::new(
                                String::new(),
                                v["name"].as_str().unwrap_or_default().into(),
                                v["engine"].as_str().unwrap_or("表").into(),
                            )
                        })
                        .collect(),
                ))
            }
            OperationKind::Details { database, object } => {
                let params = [
                    ("param_db", database.as_str()),
                    ("param_table", object.name.as_str()),
                ];
                let mut out = Vec::new();
                for row in self.query("SELECT name,type,default_expression FROM system.columns WHERE database={db:String} AND table={table:String} ORDER BY position LIMIT 500 FORMAT JSONEachRow","",&params).await? {
                    out.push((format!("列 · {}",row["name"].as_str().unwrap_or_default()),format!("{} · {}",row["type"].as_str().unwrap_or_default(),row["default_expression"].as_str().unwrap_or_default())));
                }
                match self.query("SELECT name,expr,type FROM system.data_skipping_indices WHERE database={db:String} AND table={table:String} LIMIT 500 FORMAT JSONEachRow","",&params).await {
                    Ok(rows)=>for row in rows {out.push((format!("索引 · {}",row["name"].as_str().unwrap_or_default()),format!("{} · {}",row["type"].as_str().unwrap_or_default(),row["expr"].as_str().unwrap_or_default())));},
                    Err(_)=>out.push(("索引".into(),"当前权限或服务端版本无法读取索引元数据".into())),
                }
                Ok(OperationResult::Details(out))
            }
        }
    }
}
pub(super) struct RedisDatabase {
    connection: redis::aio::MultiplexedConnection,
    cluster: bool,
    catalog_limited: bool,
    current: i64,
}
impl RedisDatabase {
    pub async fn open(p: &Profile, password: &str) -> Result<(Self, ServerInfo), String> {
        let begin = Instant::now();
        let current = p.database.parse().unwrap_or(0);
        let info_conn = redis::ConnectionInfo {
            addr: if p.tls == TlsMode::Disabled {
                redis::ConnectionAddr::Tcp(p.host.clone(), p.port)
            } else {
                redis::ConnectionAddr::TcpTls {
                    host: p.host.clone(),
                    port: p.port,
                    insecure: p.tls == TlsMode::Required,
                    tls_params: None,
                }
            },
            redis: redis::RedisConnectionInfo {
                db: current,
                username: (!p.user.is_empty()).then(|| p.user.clone()),
                password: (!password.is_empty()).then(|| password.into()),
                ..Default::default()
            },
        };
        let client =
            if p.tls != TlsMode::Disabled && (!p.ca.is_empty() || !p.client_cert.is_empty()) {
                redis::Client::build_with_tls(
                    info_conn,
                    redis::TlsCertificates {
                        root_cert: if p.ca.is_empty() {
                            None
                        } else {
                            Some(read_file(&p.ca)?)
                        },
                        client_tls: if p.client_cert.is_empty() {
                            None
                        } else {
                            Some(redis::ClientTlsConfig {
                                client_cert: read_file(&p.client_cert)?,
                                client_key: read_file(&p.client_key)?,
                            })
                        },
                    },
                )
                .map_err(network)?
            } else {
                redis::Client::open(info_conn).map_err(network)?
            };
        let mut connection = client
            .get_multiplexed_async_connection()
            .await
            .map_err(network)?;
        let _: String = redis::cmd("PING")
            .query_async(&mut connection)
            .await
            .map_err(network)?;
        let details: Option<String> = redis::cmd("INFO")
            .arg("server")
            .query_async(&mut connection)
            .await
            .ok();
        let cluster_info: Option<String> = redis::cmd("INFO")
            .arg("cluster")
            .query_async(&mut connection)
            .await
            .ok();
        let cluster = p.redis_cluster
            || cluster_info.is_some_and(|s| s.lines().any(|s| s.trim() == "cluster_enabled:1"));
        let version = details
            .and_then(|s| {
                s.lines().find_map(|l| {
                    l.strip_prefix("redis_version:")
                        .map(|s| s.trim().to_string())
                })
            })
            .unwrap_or("版本信息不可读".into());
        let mut this = Self {
            connection,
            cluster,
            catalog_limited: false,
            current,
        };
        let databases = if !p.database.is_empty() {
            vec![p.database.clone()]
        } else {
            this.databases().await
        };
        let mut result = info(
            version,
            "二进制",
            Some(current.to_string()),
            databases,
            if p.tls == TlsMode::Disabled {
                ""
            } else {
                "TLS"
            },
            begin.elapsed().as_millis(),
        );
        if cluster {
            result.database_list_error =
                Some("Redis 集群仅显示 DB 0；Key 浏览范围为当前节点".into());
        }
        if !cluster && this.catalog_limited {
            result.database_list_error =
                Some("逻辑库枚举受权限或数量限制，可在连接配置中手动指定编号".into());
        }
        Ok((this, result))
    }
    async fn databases(&mut self) -> Vec<String> {
        if self.cluster {
            return vec!["0".into()];
        }
        let result: Result<Vec<String>, _> = redis::cmd("CONFIG")
            .arg("GET")
            .arg("databases")
            .query_async(&mut self.connection)
            .await;
        self.catalog_limited = result.is_err();
        let count = result
            .ok()
            .and_then(|r| r.get(1).and_then(|s| s.parse::<u32>().ok()))
            .unwrap_or(1)
            .min(512);
        (0..count).map(|n| n.to_string()).collect()
    }
    pub async fn ping(&mut self) -> Result<(), String> {
        redis::cmd("PING")
            .query_async::<String>(&mut self.connection)
            .await
            .map(|_| ())
            .map_err(network)
    }
    async fn select(&mut self, database: &str) -> Result<(), String> {
        let n = database
            .parse::<i64>()
            .map_err(|_| "Redis 数据库编号无效")?;
        if n < 0 || self.cluster && n != 0 {
            return Err("Redis 集群仅支持 DB 0".into());
        }
        if n != self.current {
            let _: String = redis::cmd("SELECT")
                .arg(n)
                .query_async(&mut self.connection)
                .await
                .map_err(network)?;
            self.current = n;
        }
        Ok(())
    }
    pub async fn operation(
        &mut self,
        p: &Profile,
        op: &Operation,
    ) -> Result<OperationResult, String> {
        match &op.kind {
            OperationKind::Workbench(_) => Err("Redis 使用 Key 浏览器，不支持关系表 SQL".into()),
            OperationKind::Databases => Ok(OperationResult::Databases(if !p.database.is_empty() {
                vec![p.database.clone()]
            } else {
                self.databases().await
            })),
            OperationKind::CreateDatabase { .. } => {
                Err("Redis 使用已有逻辑库，不支持 CREATE DATABASE".into())
            }
            OperationKind::Objects { database, cursor } => {
                self.select(database).await?;
                let (next, keys): (u64, Vec<Vec<u8>>) = redis::cmd("SCAN")
                    .arg(cursor.as_deref().unwrap_or("0"))
                    .arg("COUNT")
                    .arg(100)
                    .query_async(&mut self.connection)
                    .await
                    .map_err(network)?;
                let items = keys
                    .into_iter()
                    .take(500)
                    .filter(|key| key.len() <= 4096)
                    .map(|key| ObjectItem {
                        schema: String::new(),
                        name: String::from_utf8(key.clone())
                            .unwrap_or_else(|_| format!("二进制 Key · {} bytes", key.len())),
                        kind: "Key".into(),
                        key: Some(key),
                    })
                    .collect();
                Ok(OperationResult::Objects {
                    items,
                    cursor: (next != 0).then(|| next.to_string()),
                    notice: self.cluster.then(|| {
                        "Redis 集群：仅当前节点；SCAN 为增量快照，变化中的 Key 可能重复".into()
                    }),
                })
            }
            OperationKind::Details { database, object } => {
                self.select(database).await?;
                let key = object.key.as_deref().unwrap_or(object.name.as_bytes());
                let kind: String = redis::cmd("TYPE")
                    .arg(key)
                    .query_async(&mut self.connection)
                    .await
                    .map_err(network)?;
                let ttl: i64 = redis::cmd("TTL")
                    .arg(key)
                    .query_async(&mut self.connection)
                    .await
                    .map_err(network)?;
                let mut out = vec![
                    ("类型".into(), kind.clone()),
                    (
                        "TTL".into(),
                        match ttl {
                            -1 => "永久".into(),
                            -2 => "已不存在".into(),
                            n => format!("{n} 秒"),
                        },
                    ),
                ];
                let command = match kind.as_str() {
                    "string" => Some("STRLEN"),
                    "hash" => Some("HLEN"),
                    "list" => Some("LLEN"),
                    "set" => Some("SCARD"),
                    "zset" => Some("ZCARD"),
                    "stream" => Some("XLEN"),
                    _ => None,
                };
                if let Some(command) = command {
                    let size: i64 = redis::cmd(command)
                        .arg(key)
                        .query_async(&mut self.connection)
                        .await
                        .map_err(network)?;
                    out.push(("长度 / 元素数".into(), size.to_string()));
                }
                Ok(OperationResult::Details(out))
            }
        }
    }
}

struct HttpAccess<'a> {
    client: &'a HttpDatabase,
    database: &'a str,
}
impl super::workbench::Access for HttpAccess<'_> {
    fn query<'a>(&'a mut self, sql: &'a str) -> super::workbench::Query<'a> {
        Box::pin(async move {
            let read = sql.trim_start().to_ascii_uppercase();
            let sql = if read.starts_with("SELECT") || read.starts_with("SHOW") {
                format!("{sql} FORMAT JSONCompactEachRow")
            } else {
                sql.into()
            };
            let rows = self.client.query(&sql, self.database, &[]).await?;
            Ok(super::workbench::Rows {
                columns: Vec::new(),
                values: rows
                    .into_iter()
                    .map(|r| {
                        r.as_array()
                            .map(|r| {
                                r.iter()
                                    .map(|v| {
                                        if v.is_null() {
                                            None
                                        } else {
                                            Some(
                                                v.as_str()
                                                    .map(String::from)
                                                    .unwrap_or_else(|| v.to_string()),
                                            )
                                        }
                                    })
                                    .collect()
                            })
                            .unwrap_or_default()
                    })
                    .collect(),
            })
        })
    }
}

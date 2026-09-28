//! Copy to a new destination only. MySQL copies base tables; SQLite publishes a new file atomically.
use super::*;
use workbench::Access;
#[derive(Clone)]
struct Token {
    start: usize,
    end: usize,
    identifier: Option<String>,
}
fn tokens(sql: &str, escapes: bool) -> Result<Vec<Token>, String> {
    let b = sql.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let ch = sql[i..].chars().next().unwrap();
        if ch.is_whitespace() {
            i += ch.len_utf8();
            continue;
        }
        if b[i..].starts_with(b"/*") {
            i += 2;
            i += sql[i..].find("*/").ok_or("无法解析表定义注释")? + 2;
            continue;
        }
        let start = i;
        let identifier;
        if matches!(b[i], b'`' | b'\'' | b'"') {
            let quote = b[i];
            i += 1;
            let mut closed = false;
            while i < b.len() {
                if b[i] == quote {
                    if b.get(i + 1) == Some(&quote) {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    closed = true;
                    break;
                }
                if b[i] == b'\\' && escapes && quote != b'`' {
                    i += 2;
                } else {
                    i += 1;
                }
            }
            if !closed {
                return Err("无法解析表定义引号".into());
            }
            identifier = if matches!(quote, b'`' | b'"') {
                Some(
                    sql[start + 1..i - 1]
                        .replace("``", "`")
                        .replace("\"\"", "\""),
                )
            } else {
                None
            };
        } else if ch.is_alphanumeric() || ch == '_' || ch == '$' {
            i += ch.len_utf8();
            while i < b.len() {
                let c = sql[i..].chars().next().unwrap();
                if !(c.is_alphanumeric() || c == '_' || c == '$') {
                    break;
                }
                i += c.len_utf8();
            }
            identifier = Some(sql[start..i].into());
        } else {
            i += ch.len_utf8();
            identifier = None;
        }
        out.push(Token {
            start,
            end: i,
            identifier,
        });
    }
    Ok(out)
}
fn requalify(
    sql: &str,
    source: &str,
    target: &str,
    escapes: bool,
    insensitive: bool,
) -> Result<String, String> {
    let ts = tokens(sql, escapes)?;
    let mut out = sql.to_string();
    for pair in ts.windows(2).rev() {
        if pair[0].identifier.as_deref().is_some_and(|s| {
            if insensitive {
                s.eq_ignore_ascii_case(source)
            } else {
                s == source
            }
        }) && &sql[pair[1].start..pair[1].end] == "."
        {
            out.replace_range(pair[0].start..pair[0].end, &quote_mysql(target));
        }
    }
    Ok(out)
}
fn foreign_clause(
    sql: &str,
    source: &str,
    target: &str,
    escapes: bool,
    insensitive: bool,
) -> Result<String, String> {
    let mut sql = requalify(sql, source, target, escapes, insensitive)?;
    let ts = tokens(&sql, escapes)?;
    for (index, token) in ts.iter().enumerate() {
        if sql[token.start..token.end].eq_ignore_ascii_case("REFERENCES") {
            let table = ts.get(index + 1).ok_or("外键引用缺少表名")?;
            if ts
                .get(index + 2)
                .is_none_or(|t| &sql[t.start..t.end] != ".")
            {
                sql.insert_str(table.start, &format!("{}.", quote_mysql(target)));
            }
            break;
        }
    }
    Ok(sql)
}
fn table_definition(
    sql: &str,
    source: &str,
    target: &str,
    name: &str,
    escapes: bool,
    insensitive: bool,
) -> Result<(String, Vec<String>), String> {
    if sql.len() > 60_000
        || [
            "DATA DIRECTORY",
            "INDEX DIRECTORY",
            "TABLESPACE",
            "CONNECTION=",
        ]
        .iter()
        .any(|p| sql.to_ascii_uppercase().contains(p))
    {
        return Err("表定义包含不支持的外部存储选项，或过长".into());
    }
    let ts = tokens(sql, escapes)?;
    if ts.len() < 4
        || !sql[ts[0].start..ts[0].end].eq_ignore_ascii_case("CREATE")
        || !sql[ts[1].start..ts[1].end].eq_ignore_ascii_case("TABLE")
    {
        return Err("仅支持普通表定义".into());
    }
    let mut index = 3;
    if sql[ts[index].start..ts[index].end] == *"." {
        index += 2;
    }
    if ts.get(index).is_none_or(|t| &sql[t.start..t.end] != "(") {
        return Err("表定义格式不支持".into());
    }
    let open = ts[index].end;
    let mut start = open;
    let mut depth = 1;
    let mut parts = Vec::new();
    let mut close = None;
    for t in &ts[index + 1..] {
        match &sql[t.start..t.end] {
            "(" => depth += 1,
            ")" => {
                depth -= 1;
                if depth == 0 {
                    parts.push(sql[start..t.start].trim().to_string());
                    close = Some(t.start);
                    break;
                }
            }
            "," if depth == 1 => {
                parts.push(sql[start..t.start].trim().to_string());
                start = t.end;
            }
            _ => {}
        }
    }
    let close = close.ok_or("表定义括号不完整")?;
    let mut foreign = Vec::new();
    let mut fields = Vec::new();
    for part in parts {
        let parts = tokens(&part, escapes)?;
        let first = parts.first().map(|t| &part[t.start..t.end]).unwrap_or("");
        let is_foreign = first.eq_ignore_ascii_case("FOREIGN")
            || first.eq_ignore_ascii_case("CONSTRAINT")
                && parts
                    .get(2)
                    .is_some_and(|t| part[t.start..t.end].eq_ignore_ascii_case("FOREIGN"));
        if is_foreign {
            foreign.push(foreign_clause(&part, source, target, escapes, insensitive)?);
        } else {
            fields.push(part);
        }
    }
    if fields.is_empty() {
        return Err("表定义没有可复制的列".into());
    }
    let ddl = format!(
        "CREATE TABLE {}.{} ({}{}",
        quote_mysql(target),
        quote_mysql(name),
        fields.join(
            ",
"
        ),
        &sql[close..]
    );
    Ok((
        requalify(&ddl, source, target, escapes, insensitive)?,
        foreign,
    ))
}
pub async fn mysql(
    c: &mut impl Access,
    source: &str,
    target: &str,
    charset: &str,
    include_data: bool,
) -> Result<OperationResult, String> {
    validate_name(DatabaseKind::MySql, source)?;
    validate_name(DatabaseKind::MySql, target)?;
    if source.eq_ignore_ascii_case(target) {
        return Err("目标库必须与源库不同".into());
    }
    let settings = c.query("SELECT VERSION()").await?;
    let version = settings
        .values
        .first()
        .and_then(|r| r.first())
        .and_then(|v| v.as_deref())
        .unwrap_or("");
    if version.starts_with("4.0") && !charset.is_empty() {
        return Err("MySQL 4.0 仅支持服务器默认字符集".into());
    }
    let mode = c.query("SHOW VARIABLES LIKE 'sql_mode'").await?;
    let escapes = !mode
        .values
        .first()
        .and_then(|r| r.get(1))
        .and_then(|v| v.as_deref())
        .unwrap_or("")
        .contains("NO_BACKSLASH_ESCAPES");
    let casing = c
        .query("SHOW VARIABLES LIKE 'lower_case_table_names'")
        .await?;
    let insensitive = casing
        .values
        .first()
        .and_then(|r| r.get(1))
        .and_then(|v| v.as_deref())
        .is_some_and(|v| v == "1" || v == "2");
    let status = c
        .query(&format!("SHOW TABLE STATUS FROM {}", quote_mysql(source)))
        .await?;
    let mut plan = Vec::new();
    for row in status.values {
        let name = row.first().and_then(|v| v.as_deref()).ok_or("表名无效")?;
        let engine = row.get(1).and_then(|v| v.as_deref()).unwrap_or("");
        if engine.is_empty() && row.last().and_then(|v| v.as_deref()) == Some("VIEW") {
            continue;
        }
        if !["innodb", "myisam", "memory", "heap", "aria"]
            .contains(&engine.to_ascii_lowercase().as_str())
        {
            return Err("源库包含不支持安全复制的表引擎".into());
        }
        if plan.len() >= 200 {
            return Err("快速复制最多支持 200 张表，请使用原生备份工具".into());
        }
        let ddl = c
            .query(&format!(
                "SHOW CREATE TABLE {}.{}",
                quote_mysql(source),
                quote_mysql(name)
            ))
            .await?;
        let ddl = ddl
            .values
            .first()
            .and_then(|r| r.get(1))
            .and_then(|v| v.as_deref())
            .ok_or("未能读取表定义")?;
        let (ddl, foreign) = table_definition(ddl, source, target, name, escapes, insensitive)?;
        let columns = if include_data {
            c.query(&format!(
                "SHOW COLUMNS FROM {}.{}",
                quote_mysql(source),
                quote_mysql(name)
            ))
            .await?
            .values
            .into_iter()
            .filter(|r| {
                !r.get(5)
                    .and_then(|v| v.as_deref())
                    .unwrap_or("")
                    .to_ascii_uppercase()
                    .contains("VIRTUAL GENERATED")
                    && !r
                        .get(5)
                        .and_then(|v| v.as_deref())
                        .unwrap_or("")
                        .to_ascii_uppercase()
                        .contains("STORED GENERATED")
            })
            .filter_map(|r| r.into_iter().next().flatten())
            .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        if include_data && columns.is_empty() {
            return Err("没有可复制的数据列".into());
        }
        plan.push((name.to_string(), ddl, foreign, columns));
    }
    let create = if charset.is_empty() && !version.starts_with("4.0") {
        let rows = c
            .query(&format!("SHOW CREATE DATABASE {}", quote_mysql(source)))
            .await?;
        let ddl = rows
            .values
            .first()
            .and_then(|r| r.get(1))
            .and_then(|v| v.as_deref())
            .ok_or("未能读取源库字符集")?;
        let ts = tokens(ddl, escapes)?;
        let identifier = ts
            .iter()
            .skip(2)
            .find(|t| {
                t.identifier
                    .as_deref()
                    .is_some_and(|s| s.eq_ignore_ascii_case(source))
            })
            .ok_or("源库定义无法识别")?;
        format!(
            "CREATE DATABASE {}{}",
            quote_mysql(target),
            &ddl[identifier.end..]
        )
    } else {
        create_database_sql(DatabaseKind::MySql, target, charset)?
    };
    c.query(&create).await?;
    let result = async {
        for (_, ddl, _, _) in &plan {
            c.query(ddl).await?;
        }
        if include_data {
            for (name, _, _, columns) in &plan {
                let cols = columns
                    .iter()
                    .map(|s| quote_mysql(s))
                    .collect::<Vec<_>>()
                    .join(",");
                c.query(&format!(
                    "INSERT INTO {}.{} ({cols}) SELECT {cols} FROM {}.{}",
                    quote_mysql(target),
                    quote_mysql(name),
                    quote_mysql(source),
                    quote_mysql(name)
                ))
                .await?;
            }
        }
        for (name, _, foreign, _) in &plan {
            for clause in foreign {
                c.query(&format!(
                    "ALTER TABLE {}.{} ADD {clause}",
                    quote_mysql(target),
                    quote_mysql(name)
                ))
                .await?;
            }
        }
        Ok::<_, String>(())
    }
    .await;
    result.map_err(|e| format!("复制未完成，目标库已保留，请检查后处理：{e}"))?;
    Ok(OperationResult::Copied {
        name: target.into(),
        tables: plan.len(),
        include_data,
    })
}
pub async fn sqlite(
    c: &mut sqlx::SqliteConnection,
    target: &str,
    include_data: bool,
) -> Result<OperationResult, String> {
    let target = Path::new(target);
    if !target.is_absolute() || target.exists() {
        return Err("请选择尚不存在的目标文件，不能覆盖源文件或已有文件".into());
    }
    let parent = target.parent().ok_or("目标文件夹无效")?;
    let temp = tempfile::NamedTempFile::new_in(parent).map_err(|_| "无法创建目标临时文件")?;
    let count = c
        .query("SELECT count(*) FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%'")
        .await?;
    let tables = count
        .values
        .first()
        .and_then(|r| r.first())
        .and_then(|v| v.as_ref())
        .and_then(|v| v.parse().ok())
        .ok_or("源库表统计无效")?;
    if include_data {
        c.query(&format!(
            "VACUUM INTO {}",
            workbench::literal_for(DatabaseKind::Sqlite, &temp.path().display().to_string())
        ))
        .await?;
    } else {
        let rows=c.query("SELECT type,name,sql FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' AND sql IS NOT NULL ORDER BY CASE type WHEN 'table' THEN 0 WHEN 'index' THEN 1 WHEN 'view' THEN 2 ELSE 3 END,name").await?;
        if rows.values.len() > 500
            || rows.values.iter().any(|r| {
                r.get(2)
                    .and_then(|v| v.as_deref())
                    .is_some_and(|sql| sql.to_ascii_uppercase().contains("CREATE VIRTUAL TABLE"))
            })
        {
            return Err("仅结构复制不支持虚拟表或超过 500 个对象，请选择完整复制".into());
        }
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(temp.path())
            .create_if_missing(false)
            .disable_statement_logging();
        let mut destination = sqlx::SqliteConnection::connect_with(&options)
            .await
            .map_err(db_error)?;
        let encoding = c.query("PRAGMA encoding").await?;
        let encoding = encoding
            .values
            .first()
            .and_then(|r| r.first())
            .and_then(|v| v.as_deref())
            .filter(|s| ["UTF-8", "UTF-16le", "UTF-16be"].contains(s))
            .ok_or("SQLite 编码无法识别")?;
        sqlx::raw_sql(&format!("PRAGMA encoding='{encoding}'"))
            .execute(&mut destination)
            .await
            .map_err(db_error)?;
        let result = async {
            sqlx::raw_sql("BEGIN")
                .execute(&mut destination)
                .await
                .map_err(db_error)?;
            for row in rows.values {
                let ddl = row
                    .get(2)
                    .and_then(|v| v.as_deref())
                    .ok_or("对象定义无效")?;
                sqlx::raw_sql(ddl)
                    .execute(&mut destination)
                    .await
                    .map_err(db_error)?;
            }
            sqlx::raw_sql("COMMIT")
                .execute(&mut destination)
                .await
                .map_err(db_error)?;
            Ok::<_, String>(())
        }
        .await;
        if result.is_err() {
            let _ = sqlx::raw_sql("ROLLBACK").execute(&mut destination).await;
        }
        let closed = destination.close().await.map_err(db_error);
        result?;
        closed?;
    }
    temp.persist_noclobber(target)
        .map_err(|_| "无法保存目标文件，可能已存在同名文件")?;
    Ok(OperationResult::Copied {
        name: target
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        tables,
        include_data,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn foreign_keys_are_added_after_tables_and_strings_are_preserved() {
        let ddl = "CREATE TABLE `child` (`id` int, `text` varchar(30) DEFAULT 'src.value,(x)', CONSTRAINT `fk` FOREIGN KEY (`id`) REFERENCES `src`.`parent` (`id`)) ENGINE=InnoDB";
        let (table, fk) = table_definition(ddl, "src", "dest", "child", true, false).unwrap();
        assert!(
            foreign_clause(
                "FOREIGN KEY (`id`) REFERENCES `Src`.`parent` (`id`)",
                "src",
                "dest",
                true,
                false
            )
            .unwrap()
            .contains("`Src`.`parent`")
        );
        assert!(table.starts_with("CREATE TABLE `dest`.`child`"));
        assert!(table.contains("'src.value,(x)'"));
        assert!(!table.contains("FOREIGN KEY"));
        assert!(fk[0].contains("REFERENCES `dest`.`parent`"));
        assert!(
            table_definition(
                "CREATE TABLE `x` (`id` int) DATA DIRECTORY='/tmp'",
                "src",
                "dest",
                "x",
                true,
                false
            )
            .is_err()
        );
    }
    #[tokio::test]
    async fn sqlite_copy_structure_data_and_no_overwrite() {
        let folder = tempfile::tempdir().unwrap();
        let source = folder.path().join("source.db");
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&source)
            .create_if_missing(true);
        let mut c = sqlx::SqliteConnection::connect_with(&options)
            .await
            .unwrap();
        sqlx::raw_sql("CREATE TABLE demo(id INTEGER PRIMARY KEY,value TEXT); INSERT INTO demo VALUES(1,'example'); CREATE VIEW view_demo AS SELECT * FROM demo;").execute(&mut c).await.unwrap();
        for (with_data, file, expected) in [(false, "schema.db", 0), (true, "full.db", 1)] {
            let target = folder.path().join(file);
            sqlite(&mut c, target.to_str().unwrap(), with_data)
                .await
                .unwrap();
            let mut copied = sqlx::SqliteConnection::connect_with(
                &sqlx::sqlite::SqliteConnectOptions::new().filename(&target),
            )
            .await
            .unwrap();
            let count: i64 = sqlx::query_scalar("SELECT count(*) FROM view_demo")
                .fetch_one(&mut copied)
                .await
                .unwrap();
            assert_eq!(count, expected);
            copied.close().await.unwrap();
            assert!(
                sqlite(&mut c, target.to_str().unwrap(), true)
                    .await
                    .is_err()
            );
        }
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM demo")
            .fetch_one(&mut c)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }
}

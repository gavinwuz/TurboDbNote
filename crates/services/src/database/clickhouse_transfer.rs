//! Native HTTP transfers. Payloads stay on disk; never pass them through the GUI grid.
use super::*;
use serde_json::Value;
use std::io::Seek;

fn lit(s: &str) -> String {
    format!(
        "'{}'",
        s.replace('\\', "\\\\")
            .replace('\'', "\\'")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\0', "\\0")
    )
}
fn id(s: &str) -> String {
    crate::database::workbench::quote(DatabaseKind::ClickHouse, s)
}
fn upload(
    file: File,
    c: &Control,
    tx: &async_channel::Sender<Event>,
) -> Result<reqwest::blocking::Body, String> {
    struct Upload {
        file: File,
        cancelled: Arc<AtomicBool>,
        tx: async_channel::Sender<Event>,
        bytes: u64,
        total: u64,
        tick: std::time::Instant,
    }
    impl Read for Upload {
        fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
            if self.cancelled.load(Ordering::Relaxed) {
                return Err(std::io::Error::other("Transfer cancelled"));
            }
            let n = self.file.read(b)?;
            self.bytes += n as u64;
            if self.tick.elapsed() >= Duration::from_millis(100) || self.bytes == self.total {
                notify(
                    &self.tx,
                    Progress {
                        unit: Unit::Bytes,
                        bytes: self.bytes,
                        completed: self.bytes,
                        total: Some(self.total),
                        importing: true,
                        ..Default::default()
                    },
                );
                self.tick = std::time::Instant::now();
            }
            Ok(n)
        }
    }
    let total = file.metadata().map_err(io_error)?.len();
    Ok(reqwest::blocking::Body::sized(
        Upload {
            file,
            cancelled: c.cancelled.clone(),
            tx: tx.clone(),
            bytes: 0,
            total,
            tick: std::time::Instant::now(),
        },
        total,
    ))
}
struct Http {
    client: reqwest::blocking::Client,
    url: reqwest::Url,
    user: String,
    password: String,
    session: String,
}
impl Http {
    fn new(e: &Endpoint) -> Result<Self, String> {
        let p = &e.profile;
        let mut url = reqwest::Url::parse(if p.tls == TlsMode::Disabled {
            "http://localhost"
        } else {
            "https://localhost"
        })
        .map_err(io_error)?;
        url.set_host(Some(&p.host))
            .map_err(|_| "clickhouse.http.execution")?;
        url.set_port(Some(p.port))
            .map_err(|_| "clickhouse.http.execution")?;
        let mut b = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(p.timeout_seconds.clamp(1, 60)))
            .timeout(p.execution_timeout())
            .danger_accept_invalid_certs(p.tls == TlsMode::Required);
        if !p.ca.is_empty() {
            b = b.add_root_certificate(
                reqwest::Certificate::from_pem(&std::fs::read(&p.ca).map_err(io_error)?)
                    .map_err(io_error)?,
            );
        }
        if !p.client_cert.is_empty() {
            let mut pem = std::fs::read(&p.client_cert).map_err(io_error)?;
            pem.push(b'\n');
            pem.extend(std::fs::read(&p.client_key).map_err(io_error)?);
            b = b.identity(reqwest::Identity::from_pem(&pem).map_err(io_error)?);
        }
        Ok(Self {
            client: b.build().map_err(io_error)?,
            url,
            user: p.user.clone(),
            password: e.password.clone(),
            session: uuid::Uuid::new_v4().to_string(),
        })
    }
    fn request(
        &self,
        db: &str,
        sql: &str,
        body: Option<reqwest::blocking::Body>,
    ) -> Result<reqwest::blocking::Response, String> {
        let mut r = self
            .client
            .post(self.url.clone())
            .basic_auth(&self.user, Some(&self.password))
            .query(&[
                ("session_id", self.session.as_str()),
                ("wait_end_of_query", "1"),
            ]);
        if !db.is_empty() {
            r = r.query(&[("database", db)]);
        }
        r = if let Some(file) = body {
            r.query(&[("query", sql)]).body(file)
        } else {
            r.body(sql.to_owned())
        };
        let response = r.send().map_err(|_| "clickhouse.http.execution")?;
        if !response.status().is_success()
            || response
                .headers()
                .contains_key("X-ClickHouse-Exception-Code")
        {
            return Err(crate::database::clickhouse_sql::http_error(
                response.status().as_u16(),
                response
                    .headers()
                    .get("X-ClickHouse-Exception-Code")
                    .and_then(|h| h.to_str().ok()),
            ));
        }
        Ok(response)
    }
    fn json(&self, db: &str, sql: &str) -> Result<Value, String> {
        let mut response = self.request(db, &format!("{sql} FORMAT JSON"), None)?;
        let mut bytes = Vec::new();
        response
            .by_ref()
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err("clickhouse.transfer.catalog_limit".into());
        }
        serde_json::from_slice(&bytes).map_err(|_| "clickhouse.http.response".into())
    }
    fn execute(&self, db: &str, sql: &str) -> Result<(), String> {
        let mut r = self.request(db, sql, None)?;
        let mut bytes = Vec::new();
        r.by_ref()
            .take(65537)
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        if bytes.len() > 65536 {
            return Err("clickhouse.http.response".into());
        }
        Ok(())
    }
}
#[derive(Clone)]
struct Item {
    name: String,
    engine: String,
    ddl: String,
}
fn objects(h: &Http, db: &str) -> Result<Vec<Item>, String> {
    let json=h.json(db,&format!("SELECT name, engine, create_table_query FROM system.tables WHERE database={} ORDER BY name LIMIT 2001",lit(db)))?;
    let data = json["data"].as_array().ok_or("clickhouse.http.response")?;
    if data.len() > 2000 {
        return Err("clickhouse.transfer.catalog_limit".into());
    }
    data.iter()
        .map(|r| {
            Ok(Item {
                name: r["name"].as_str().ok_or("clickhouse.http.response")?.into(),
                engine: r["engine"]
                    .as_str()
                    .ok_or("clickhouse.http.response")?
                    .into(),
                ddl: r["create_table_query"]
                    .as_str()
                    .ok_or("clickhouse.http.response")?
                    .into(),
            })
        })
        .collect()
}
fn is_view(i: &Item) -> bool {
    i.engine.contains("View")
}
fn stores_data(i: &Item) -> bool {
    !is_view(i)
        && !matches!(
            i.engine.as_str(),
            "Dictionary"
                | "Distributed"
                | "Buffer"
                | "Null"
                | "Merge"
                | "Kafka"
                | "RabbitMQ"
                | "S3"
                | "URL"
                | "File"
        )
}
fn local_ddl(sql: &str, db: &str) -> Result<String, String> {
    use sqlparser::{
        dialect::ClickHouseDialect,
        tokenizer::{Token, Tokenizer},
    };
    let ts = Tokenizer::new(&ClickHouseDialect {}, sql)
        .tokenize_with_location()
        .map_err(|_| "script.error.syntax")?
        .into_iter()
        .filter(|t| !matches!(t.token, Token::Whitespace(_)))
        .collect::<Vec<_>>();
    let offset = |line: u64, column: u64| -> Result<usize, String> {
        let (mut l, mut col) = (1, 1);
        for (i, c) in sql.char_indices() {
            if (l, col) == (line, column) {
                return Ok(i);
            }
            if c == '\n' {
                l += 1;
                col = 1;
            } else {
                col += 1;
            }
        }
        if (l, col) == (line, column) {
            Ok(sql.len())
        } else {
            Err("script.error.syntax".into())
        }
    };
    let mut ranges = vec![];
    for (i, w) in ts.windows(2).enumerate() {
        let relation = i > 0
            && matches!(&ts[i-1].token,Token::Word(v) if v.quote_style.is_none() && matches!(v.value.to_ascii_uppercase().as_str(),"TABLE"|"VIEW"|"DICTIONARY"|"FROM"|"JOIN"|"TO"|"EXISTS"));
        let qualified_column = ts.get(i + 3).is_some_and(|t| t.token == Token::Period);
        if (relation || qualified_column)
            && matches!(&w[0].token,Token::Word(v) if v.value==db)
            && w[1].token == Token::Period
        {
            ranges.push((
                offset(w[0].span.start.line, w[0].span.start.column)?,
                offset(w[1].span.end.line, w[1].span.end.column)?,
            ));
        }
    }
    let mut out = sql.to_owned();
    for (start, end) in ranges.into_iter().rev() {
        out.replace_range(start..end, "");
    }
    let keyword = |at: usize, w: &str| matches!(ts.get(at).map(|t|&t.token),Some(Token::Word(v)) if v.quote_style.is_none() && v.value.eq_ignore_ascii_case(w));
    if keyword(0, "CREATE") && keyword(1, "TABLE") && !keyword(2, "IF") {
        let end = offset(ts[1].span.end.line, ts[1].span.end.column)?;
        out.insert_str(end, " IF NOT EXISTS");
    }
    crate::database::clickhouse_sql::classify(&out, "")?;
    Ok(out)
}
fn dependencies(item: &Item) -> Result<Vec<String>, String> {
    use sqlparser::tokenizer::Token;
    let ts = crate::database::clickhouse_sql::tokens(&item.ddl)?;
    let mut names = vec![];
    for i in 0..ts.len().saturating_sub(1) {
        if matches!(&ts[i],Token::Word(w) if w.quote_style.is_none() && matches!(w.value.to_ascii_uppercase().as_str(),"FROM"|"JOIN"|"TO"))
        {
            let at = if ts.get(i + 2) == Some(&Token::Period) {
                i + 3
            } else {
                i + 1
            };
            if let Some(Token::Word(w)) = ts.get(at)
                && ts.get(at + 1) != Some(&Token::LParen)
            {
                names.push(w.value.clone());
            }
        }
    }
    Ok(names)
}
fn restore_order(items: &[Item]) -> Result<Vec<Item>, String> {
    let mut pending = items.to_vec();
    let mut ordered: Vec<Item> = vec![];
    while !pending.is_empty() {
        let mut found = None;
        for (i, item) in pending.iter().enumerate() {
            let deps = dependencies(item)?;
            if !deps
                .iter()
                .any(|n| n != &item.name && pending.iter().any(|j| &j.name == n))
            {
                found = Some(i);
                break;
            }
        }
        ordered.push(pending.remove(found.ok_or("clickhouse.transfer.dependencies")?));
    }
    Ok(ordered)
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    objects: Vec<BundleObject>,
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BundleObject {
    name: String,
    ddl: Option<String>,
    data: Option<String>,
    sha256: Option<String>,
    columns: Option<String>,
}
fn hash(file: &mut File, c: &Control) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    file.rewind().map_err(io_error)?;
    let mut h = Sha256::new();
    let mut b = [0; 65536];
    loop {
        c.check()?;
        let n = file.read(&mut b).map_err(io_error)?;
        if n == 0 {
            break;
        }
        h.update(&b[..n]);
    }
    file.rewind().map_err(io_error)?;
    Ok(format!("{:x}", h.finalize()))
}
fn export_bundle(
    h: &Http,
    db: &str,
    items: &[Item],
    contents: Contents,
    out: &mut File,
    c: &Control,
    tx: &async_channel::Sender<Event>,
) -> Result<(), String> {
    let mut zip = zip::ZipWriter::new(out);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .large_file(true);
    let mut manifest = Manifest {
        version: 1,
        objects: vec![],
    };
    for (n, i) in restore_order(items)?.iter().enumerate() {
        c.check()?;
        let mut object = BundleObject {
            name: i.name.clone(),
            ddl: contents.schema.then(|| local_ddl(&i.ddl, db)).transpose()?,
            data: None,
            sha256: None,
            columns: None,
        };
        if contents.data && stores_data(i) {
            let mut data = tempfile::NamedTempFile::new().map_err(io_error)?;
            let fields = insert_columns(h, db, &i.name)?;
            object.columns = Some(fields.clone());
            stream(
                h.request(
                    db,
                    &format!("SELECT {fields} FROM {} FORMAT Native", id(&i.name)),
                    None,
                )?,
                data.as_file_mut(),
                c,
                tx,
                false,
                None,
            )?;
            object.sha256 = Some(hash(data.as_file_mut(), c)?);
            let path = format!("data/{n}.native");
            zip.start_file(&path, opts).map_err(io_error)?;
            stream(data.as_file_mut(), &mut zip, c, tx, false, None)?;
            object.data = Some(path);
        }
        manifest.objects.push(object);
    }
    zip.start_file("manifest.json", opts).map_err(io_error)?;
    serde_json::to_writer(&mut zip, &manifest).map_err(io_error)?;
    zip.finish().map_err(io_error)?;
    Ok(())
}
fn insert_columns(h: &Http, db: &str, table: &str) -> Result<String, String> {
    let cols=h.json(db,&format!("SELECT name FROM system.columns WHERE database={} AND table={} AND default_kind NOT IN ('MATERIALIZED','ALIAS') ORDER BY position",lit(db),lit(table)))?;
    let names = cols["data"]
        .as_array()
        .ok_or("clickhouse.http.response")?
        .iter()
        .map(|r| r["name"].as_str().map(id).ok_or("clickhouse.http.response"))
        .collect::<Result<Vec<_>, _>>()?;
    if names.is_empty() {
        return Err("clickhouse.sql.target".into());
    }
    Ok(names.join(","))
}
fn import_bundle(
    h: &Http,
    db: &str,
    path: &Path,
    c: &Control,
    tx: &async_channel::Sender<Event>,
) -> Result<(), String> {
    let mut archive =
        zip::ZipArchive::new(File::open(path).map_err(io_error)?).map_err(io_error)?;
    let manifest: Manifest = {
        let mut entry = archive.by_name("manifest.json").map_err(io_error)?;
        if entry.size() > 4 * 1024 * 1024 {
            return Err("clickhouse.transfer.catalog_limit".into());
        }
        serde_json::from_reader(&mut entry).map_err(|_| "clickhouse.transfer.manifest")?
    };
    if manifest.version != 1
        || manifest.objects.len() > 2000
        || archive.len() != manifest.objects.iter().filter(|o| o.data.is_some()).count() + 1
    {
        return Err("clickhouse.transfer.manifest".into());
    }
    let staging = tempfile::tempdir().map_err(io_error)?;
    let mut names = std::collections::HashSet::new();
    let mut entry_names = std::collections::HashSet::new();
    let mut unpacked = 0u64;
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(io_error)?;
        if !entry_names.insert(entry.name().to_owned()) {
            return Err("clickhouse.transfer.manifest".into());
        }
        unpacked = unpacked
            .checked_add(entry.size())
            .ok_or("clickhouse.transfer.manifest")?;
        if entry.size() > 512 * 1024 * 1024 * 1024 || unpacked > 4 * 1024 * 1024 * 1024 * 1024 {
            return Err("clickhouse.transfer.manifest".into());
        }
    }
    // Validate every definition and every payload before executing any statement.
    for (n, o) in manifest.objects.iter().enumerate() {
        c.check()?;
        if o.name.is_empty() || !names.insert(&o.name) {
            return Err("clickhouse.transfer.manifest".into());
        }
        if let Some(ddl) = &o.ddl {
            let command = crate::database::clickhouse_sql::classify(ddl, db)?;
            if command.permission != "create"
                || command.targets.first().is_none_or(|t| t.1 != o.name)
            {
                return Err("clickhouse.transfer.manifest".into());
            }
            if crate::database::clickhouse_sql::tokens(ddl)?.iter().any(|t|matches!(t,sqlparser::tokenizer::Token::Word(w) if w.quote_style.is_none() && w.value.to_ascii_lowercase().starts_with("replicated"))) {
                let existing=h.json(db,&format!("SELECT name FROM system.tables WHERE database={} AND name={}",lit(db),lit(&o.name)))?;
                if existing["data"].as_array().is_none_or(Vec::is_empty){return Err("clickhouse.transfer.replication".into());}
            }
        }
        match (&o.data, &o.sha256) {
            (Some(name), Some(expected)) => {
                // No archive extraction: generated local filenames, never entry paths.
                if name != &format!("data/{n}.native")
                    || expected.len() != 64
                    || !expected.bytes().all(|b| b.is_ascii_hexdigit())
                    || o.columns.as_ref().is_none_or(String::is_empty)
                {
                    return Err("clickhouse.transfer.manifest".into());
                }
                let entry = archive.by_name(name).map_err(io_error)?;
                let mut file =
                    File::create(staging.path().join(n.to_string())).map_err(io_error)?;
                stream(entry, &mut file, c, tx, true, None)?;
                if hash(
                    &mut File::open(staging.path().join(n.to_string())).map_err(io_error)?,
                    c,
                )? != *expected
                {
                    return Err("clickhouse.transfer.manifest".into());
                }
            }
            (None, None) => {}
            _ => return Err("clickhouse.transfer.manifest".into()),
        }
    }
    for (n, o) in manifest.objects.iter().enumerate() {
        c.check()?;
        if let Some(ddl) = &o.ddl {
            h.execute(db, ddl)?;
        }
        if o.data.is_some() {
            let fields = insert_columns(h, db, &o.name)?;
            if o.columns.as_deref() != Some(fields.as_str()) {
                return Err("notes.preview_changed".into());
            }
            let response = h.request(
                db,
                &format!("INSERT INTO {} ({fields}) FORMAT Native", id(&o.name)),
                Some(upload(
                    File::open(staging.path().join(n.to_string())).map_err(io_error)?,
                    c,
                    tx,
                )?),
            )?;
            stream(response, std::io::sink(), c, tx, true, None)?;
        }
        notify(
            tx,
            Progress {
                unit: Unit::Tables,
                completed: n as u64 + 1,
                total: Some(manifest.objects.len() as u64),
                importing: true,
                ..Default::default()
            },
        );
    }
    Ok(())
}
pub(super) fn preview(e: &Endpoint, table: Option<&str>) -> Result<Preview, String> {
    let h = Http::new(e)?;
    let db = &e.profile.database;
    let items = objects(&h, db)?;
    let selected = table
        .and_then(|t| items.iter().find(|i| i.name == t))
        .or_else(|| items.iter().find(|i| stores_data(i)));
    let mut p = Preview::default();
    if let Some(i) = selected {
        let json = h.json(db, &format!("SELECT * FROM {} LIMIT 5", id(&i.name)))?;
        p.table = i.name.clone();
        p.columns = json["meta"]
            .as_array()
            .ok_or("clickhouse.http.response")?
            .iter()
            .take(16)
            .filter_map(|m| m["name"].as_str().map(str::to_owned))
            .collect();
        p.rows = json["data"]
            .as_array()
            .ok_or("clickhouse.http.response")?
            .iter()
            .map(|r| {
                p.columns
                    .iter()
                    .map(|n| {
                        let v = &r[n];
                        v.as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| v.to_string())
                            .chars()
                            .take(128)
                            .collect()
                    })
                    .collect()
            })
            .collect();
    }
    p.objects = items
        .iter()
        .map(|i| ObjectChoice {
            name: i.name.clone(),
            kind: if is_view(i) { "view" } else { "table" }.into(),
        })
        .collect();
    Ok(p)
}
pub(super) fn preflight(
    source: &Endpoint,
    target: Option<&Endpoint>,
    mode: Mode,
    new_db: &str,
    c: &Control,
) -> Result<String, String> {
    c.check()?;
    if mode != Mode::ImportFile {
        Http::new(source)?.execute(&source.profile.database, "SELECT 1 FORMAT Null")?;
    }
    if let Some(t) = target {
        if t.profile.kind != DatabaseKind::ClickHouse {
            return Err("clickhouse.transfer.same_driver".into());
        }
        let h = Http::new(t)?;
        if mode == Mode::Copy {
            if new_db.trim().is_empty() {
                return Err("clickhouse.sql.target".into());
            }
            let json = h.json(
                "",
                &format!(
                    "SELECT name FROM system.databases WHERE name={}",
                    lit(new_db)
                ),
            )?;
            if json["data"].as_array().is_none_or(|v| !v.is_empty()) {
                return Err("clickhouse.transfer.exists".into());
            }
        } else {
            h.execute(&t.profile.database, "SELECT 1 FORMAT Null")?;
        }
        if mode == Mode::ImportConnection
            && source.profile.host.eq_ignore_ascii_case(&t.profile.host)
            && source.profile.port == t.profile.port
            && source.profile.database == t.profile.database
        {
            return Err("clickhouse.transfer.same_target".into());
        }
    }
    Ok("UTF8".into())
}
fn stream(
    mut input: impl Read,
    mut output: impl Write,
    c: &Control,
    tx: &async_channel::Sender<Event>,
    importing: bool,
    total: Option<u64>,
) -> Result<u64, String> {
    let mut b = [0; 65536];
    let mut bytes = 0;
    let mut tick = std::time::Instant::now();
    loop {
        c.check()?;
        let n = input.read(&mut b).map_err(io_error)?;
        if n == 0 {
            break;
        }
        output.write_all(&b[..n]).map_err(io_error)?;
        bytes += n as u64;
        if tick.elapsed() >= Duration::from_millis(100) {
            notify(
                tx,
                Progress {
                    unit: Unit::Bytes,
                    bytes,
                    completed: bytes,
                    total,
                    importing,
                    ..Default::default()
                },
            );
            tick = std::time::Instant::now();
        }
    }
    output.flush().map_err(io_error)?;
    notify(
        tx,
        Progress {
            unit: Unit::Bytes,
            bytes,
            completed: bytes,
            total,
            importing,
            ..Default::default()
        },
    );
    Ok(bytes)
}
fn format(path: &Path) -> Result<Option<&'static str>, String> {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "sql" => Ok(None),
        "csv" => Ok(Some("CSV")),
        "tsv" => Ok(Some("TabSeparated")),
        "jsonl" | "ndjson" => Ok(Some("JSONEachRow")),
        "parquet" => Ok(Some("Parquet")),
        "native" => Ok(Some("Native")),
        _ => Err("clickhouse.transfer.format".into()),
    }
}
fn selected(h: &Http, db: &str, o: &Options) -> Result<Vec<Item>, String> {
    let items = objects(h, db)?;
    if let Some(names) = &o.tables
        && names.iter().any(|n| !items.iter().any(|i| &i.name == n))
    {
        return Err("clickhouse.sql.target".into());
    }
    let selected = items
        .iter()
        .filter(|i| {
            o.tables
                .as_ref()
                .is_none_or(|names| names.contains(&i.name))
        })
        .filter(|i| !is_view(i) || o.views)
        .filter(|i| !o.excluded_objects.iter().any(|(_, n)| n == &i.name))
        .cloned()
        .collect::<Vec<_>>();
    for i in &selected {
        for dep in dependencies(i)? {
            if items.iter().any(|i| i.name == dep) && !selected.iter().any(|i| i.name == dep) {
                return Err("clickhouse.transfer.dependencies".into());
            }
        }
    }
    Ok(selected)
}
fn validate_data(items: &[Item], contents: Contents) -> Result<(), String> {
    if !contents.data {
        return Ok(());
    }
    for i in items {
        if i.name.starts_with(".inner") || (i.engine=="MaterializedView" && !crate::database::clickhouse_sql::tokens(&i.ddl)?.iter().any(|t|matches!(t,sqlparser::tokenizer::Token::Word(w) if w.quote_style.is_none() && w.value.eq_ignore_ascii_case("TO")))) {
            return Err("clickhouse.transfer.inner_view".into());
        }
        if !stores_data(i) && !is_view(i) && i.engine != "Dictionary" {
            return Err("clickhouse.transfer.engine".into());
        }
    }
    Ok(())
}
fn dump(
    h: &Http,
    db: &str,
    items: &[Item],
    contents: Contents,
    o: &Options,
    out: &mut File,
    progress: (&Control, &async_channel::Sender<Event>),
) -> Result<(), String> {
    let (c, tx) = progress;
    writeln!(out,"-- TurboDBNote ClickHouse export. Not a cross-table snapshot.\n-- Restore into the selected database; existing object definitions are not reconciled.").map_err(io_error)?;
    // Refuse dependencies outside the selection rather than producing a dump
    // described as restorable when its materialized view targets are missing.
    let ordered = restore_order(items)?;
    for (n, i) in ordered.iter().enumerate() {
        c.check()?;
        if contents.schema {
            if i.ddl.trim().is_empty() {
                return Err("clickhouse.http.response".into());
            }
            writeln!(
                out,
                "{};",
                local_ddl(i.ddl.trim().trim_end_matches(';'), db)?
            )
            .map_err(io_error)?;
        }
        if contents.data && stores_data(i) {
            let names = insert_columns(h, db, &i.name)?;
            // SQLInsert writes its target verbatim. Include our escaped column
            // list there; its own column-name writer does not escape backticks.
            let target = format!("{} ({names})", id(&i.name));
            let sql = format!(
                "SELECT {names} FROM {} SETTINGS output_format_sql_insert_table_name={}, output_format_sql_insert_include_column_names=0, output_format_sql_insert_max_batch_size={} FORMAT SQLInsert",
                id(&i.name),
                lit(&target),
                o.batch_size
            );
            let mut response = h.request(db, &sql, None)?;
            stream(&mut response, &mut *out, c, tx, false, None)?;
            writeln!(out).map_err(io_error)?;
        }
        notify(
            tx,
            Progress {
                unit: Unit::Tables,
                completed: n as u64 + 1,
                total: Some(items.len() as u64),
                ..Default::default()
            },
        );
    }
    Ok(())
}
fn import_sql(
    h: &Http,
    db: &str,
    file: &Path,
    c: &Control,
    tx: &async_channel::Sender<Event>,
) -> Result<(), String> {
    let mut copy = tempfile::Builder::new()
        .suffix(".sql")
        .tempfile()
        .map_err(io_error)?;
    stream(
        File::open(file).map_err(io_error)?,
        copy.as_file_mut(),
        c,
        tx,
        true,
        None,
    )?;
    let copy = copy.into_temp_path();
    let prepared = crate::sql_script::prepare(
        &copy,
        turbodbn_core::notebook::SqlDialect::ClickHouse,
        &c.cancelled,
        |bytes, total| {
            notify(
                tx,
                Progress {
                    unit: Unit::Bytes,
                    bytes,
                    completed: bytes,
                    total: Some(total),
                    importing: true,
                    ..Default::default()
                },
            )
        },
    )?;
    if prepared.script.unsupported > 0 {
        return Err("clickhouse.transfer.unsupported".into());
    }
    let count = prepared.script.statements;
    let mut reader = prepared.reader()?;
    // Freeze and validate the entire file before the first side effect.
    for n in 0..count {
        c.check()?;
        let sql = reader.statement(n)?;
        crate::database::clickhouse_sql::classify(&sql, db)?;
    }
    for n in 0..count {
        c.check()?;
        let sql = reader.statement(n)?;
        h.execute(db, &sql)?;
        notify(
            tx,
            Progress {
                completed: n + 1,
                total: Some(count),
                importing: true,
                ..Default::default()
            },
        );
    }
    Ok(())
}
pub(super) fn run(job: Job, c: &Control, tx: &async_channel::Sender<Event>) -> Result<(), String> {
    let result = run_inner(job, c, tx);
    c.check()?;
    result
}
fn run_inner(job: Job, c: &Control, tx: &async_channel::Sender<Event>) -> Result<(), String> {
    job.options
        .validate(job.mode, job.contents, &job.source.profile)?;
    preflight(&job.source, job.target.as_ref(), job.mode, &job.database, c)?;
    if job.mode == Mode::ImportFile
        && job.options.tables.is_some()
        && job
            .file
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("sql") || e.eq_ignore_ascii_case("zip"))
    {
        return Err("clickhouse.transfer.file_scope".into());
    }

    if !job.contents.schema && !job.contents.data {
        return Err("clickhouse.transfer.contents".into());
    }
    if job.mode == Mode::Export {
        let h = Http::new(&job.source)?;
        let db = &job.source.profile.database;
        let items = selected(&h, db, &job.options)?;
        validate_data(&items, job.contents)?;
        if job.file.exists() {
            return Err("clickhouse.transfer.exists".into());
        }
        let mut staged =
            tempfile::NamedTempFile::new_in(job.file.parent().ok_or("clickhouse.sql.target")?)
                .map_err(io_error)?;
        if job
            .file
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
        {
            export_bundle(&h, db, &items, job.contents, staged.as_file_mut(), c, tx)?;
        } else if let Some(f) = format(&job.file)? {
            if job.contents.schema
                || !job.contents.data
                || items.len() != 1
                || !stores_data(&items[0])
            {
                return Err("clickhouse.transfer.one_table".into());
            }
            let fields = insert_columns(&h, db, &items[0].name)?;
            let response = h.request(
                db,
                &format!("SELECT {fields} FROM {} FORMAT {f}", id(&items[0].name)),
                None,
            )?;
            stream(response, staged.as_file_mut(), c, tx, false, None)?;
        } else {
            let mut raw = tempfile::NamedTempFile::new().map_err(io_error)?;
            dump(
                &h,
                db,
                &items,
                job.contents,
                &job.options,
                raw.as_file_mut(),
                (c, tx),
            )?;
            raw.as_file_mut().rewind().map_err(io_error)?;
            export_text::write_sql(
                raw.as_file_mut(),
                staged.as_file_mut(),
                job.options.crlf,
                DatabaseKind::ClickHouse,
                || c.check().map_err(std::io::Error::other),
            )
            .map_err(io_error)?;
        }
        c.check()?;
        staged.persist_noclobber(&job.file).map_err(io_error)?;
        return Ok(());
    }
    let target = job.target.as_ref().ok_or("clickhouse.sql.target")?;
    let h = Http::new(target)?;
    let db = if job.mode == Mode::Copy {
        &job.database
    } else {
        &target.profile.database
    };
    if job.mode == Mode::ImportFile {
        let frozen = freeze_file(&job.file, c)?;
        if job
            .file
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
        {
            import_bundle(&h, db, frozen.path(), c, tx)?;
        } else if let Some(f) = format(&job.file)? {
            // Explicit table selection is mandatory; file names never authorize a target.
            let tables = job
                .options
                .tables
                .as_ref()
                .filter(|v| v.len() == 1)
                .ok_or("clickhouse.transfer.one_table")?;
            let items = objects(&h, db)?;
            if !items.iter().any(|i| i.name == tables[0] && stores_data(i)) {
                return Err("clickhouse.sql.target".into());
            }
            let fields = insert_columns(&h, db, &tables[0])?;
            let mut response = h.request(
                db,
                &format!("INSERT INTO {} ({fields}) FORMAT {f}", id(&tables[0])),
                Some(upload(frozen.reopen().map_err(io_error)?, c, tx)?),
            )?;
            stream(&mut response, std::io::sink(), c, tx, true, None)?;
        } else {
            import_sql(&h, db, frozen.path(), c, tx)?;
        }
        return Ok(());
    }
    let source = Http::new(&job.source)?;
    let items = selected(&source, &job.source.profile.database, &job.options)?;
    validate_data(&items, job.contents)?;
    if job.contents.schema && items.iter().any(|i| i.engine.starts_with("Replicated")) {
        return Err("clickhouse.transfer.replication".into());
    }
    let mut file = tempfile::NamedTempFile::new().map_err(io_error)?;
    export_bundle(
        &source,
        &job.source.profile.database,
        &items,
        job.contents,
        file.as_file_mut(),
        c,
        tx,
    )?;
    file.as_file_mut().rewind().map_err(io_error)?;
    if job.mode == Mode::Copy {
        h.execute("", &format!("CREATE DATABASE {}", id(db)))?;
    }
    import_bundle(&h, db, file.path(), c, tx)
}

#[cfg(test)]
#[path = "clickhouse_transfer_tests.rs"]
mod tests;

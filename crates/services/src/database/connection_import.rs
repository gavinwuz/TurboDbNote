//! Explicitly selected third-party connection files. Secrets and executable settings are never imported.
use super::*;
use std::{collections::HashSet, io::Read, path::Path};
#[derive(Clone, Copy)]
pub enum Source {
    Navicat(DatabaseKind),
    DBeaver,
}
pub struct Report {
    pub profiles: Vec<Profile>,
    pub skipped: usize,
}
fn kind(value: &str) -> Option<DatabaseKind> {
    let value = value.to_lowercase();
    if value.contains("mysql") || value.contains("mariadb") {
        Some(DatabaseKind::MySql)
    } else if value.contains("postgres") {
        Some(DatabaseKind::PostgreSql)
    } else if value.contains("sqlite") {
        Some(DatabaseKind::Sqlite)
    } else if value.contains("clickhouse") {
        Some(DatabaseKind::ClickHouse)
    } else if value.contains("redis") {
        Some(DatabaseKind::Redis)
    } else {
        None
    }
}
fn plain(value: &str) -> Result<String, String> {
    if value.len() > 4096 || value.chars().any(char::is_control) || value.contains("${") {
        Err("连接字段包含不支持的值".into())
    } else {
        Ok(value.to_string())
    }
}
fn unescape(value: &str) -> Result<String, String> {
    let b = value.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = value.get(i + 1..i + 3).ok_or("URL 编码无效")?;
            out.push(u8::from_str_radix(hex, 16).map_err(|_| "URL 编码无效")?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| "URL 编码无效".into())
}
fn url_fields(profile: &mut Profile, url: &str) -> Result<(), String> {
    let url = url.strip_prefix("jdbc:").unwrap_or(url);
    if profile.kind == DatabaseKind::Sqlite {
        profile.sqlite_path = plain(url.strip_prefix("sqlite:").ok_or("SQLite URL 无效")?)?;
        return Ok(());
    }
    let parsed = reqwest::Url::parse(url).map_err(|_| "连接 URL 格式不支持")?;
    if ![
        "mysql",
        "mariadb",
        "postgresql",
        "postgres",
        "clickhouse",
        "http",
        "https",
        "redis",
        "rediss",
    ]
    .contains(&parsed.scheme())
    {
        return Err("连接 URL 协议不支持".into());
    }
    for (key, _) in parsed.query_pairs() {
        if !["characterEncoding", "useUnicode", "serverTimezone"].contains(&key.as_ref()) {
            return Err("URL 含高级参数，需要手动迁移".into());
        }
    }
    profile.host = parsed.host_str().ok_or("URL 缺少主机")?.into();
    profile.port = parsed
        .port_or_known_default()
        .unwrap_or(profile.kind.port());
    profile.database = unescape(parsed.path().trim_start_matches('/'))?;
    if !parsed.username().is_empty() {
        profile.user = unescape(parsed.username())?;
    }
    if let Some((_, value)) = parsed.query_pairs().find(|(k, _)| k == "characterEncoding") {
        profile.charset = if value.eq_ignore_ascii_case("UTF-8") {
            "utf8".into()
        } else {
            value.into_owned()
        };
    }
    Ok(())
}
fn finish(mut p: Profile) -> Result<Profile, String> {
    p.name = plain(&p.name)?;
    p.host = plain(&p.host)?;
    p.database = plain(&p.database)?;
    p.group = plain(&p.group)?;
    p.user = plain(&p.user)?;
    p.environment = plain(&p.environment)?;
    p.sqlite_path = plain(&p.sqlite_path)?;
    if p.name.trim().is_empty() {
        return Err("缺少连接名称".into());
    }
    if p.kind == DatabaseKind::Redis && p.database.is_empty() {
        p.database = "0".into();
    }
    p.id = uuid::Uuid::new_v4();
    p.remember_password = false;
    if p.kind == DatabaseKind::Sqlite && !Path::new(&p.sqlite_path).is_absolute() {
        return Err("SQLite 路径需要手动迁移".into());
    }
    let mut validation = p.clone();
    if matches!(p.kind, DatabaseKind::MySql | DatabaseKind::PostgreSql) && p.user.is_empty() {
        validation.user = "pending".into();
    }
    validation.validate()?;
    Ok(p)
}
fn enabled_handler(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Array(values) => values.iter().any(enabled_handler),
        serde_json::Value::Object(values) => {
            values
                .get("enabled")
                .is_some_and(|v| v.as_bool() == Some(true) || v.as_str() == Some("true"))
                || values.values().any(enabled_handler)
        }
        _ => false,
    }
}
fn json_profiles(bytes: &[u8], report: &mut Report) -> Result<(), String> {
    let root: serde_json::Value =
        serde_json::from_slice(bytes.strip_prefix(&[239, 187, 191]).unwrap_or(bytes))
            .map_err(|_| "DBeaver JSON 格式无效")?;
    let entries = root
        .get("connections")
        .and_then(|v| v.as_object())
        .ok_or("未找到 DBeaver connections")?;
    if entries.len() > 200 {
        return Err("单次最多导入 200 个连接".into());
    }
    for value in entries.values() {
        let result = (|| {
            let text = |node: &serde_json::Value, key: &str| {
                node.get(key)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            };
            let cfg = value.get("configuration").ok_or("缺少连接配置")?;
            let driver = format!("{} {}", text(value, "provider"), text(value, "driver"));
            let kind = kind(&driver).ok_or("不支持的数据库类型")?;
            if cfg.get("handlers").is_some_and(enabled_handler)
                || cfg
                    .get("config-profile")
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| !s.is_empty())
            {
                return Err("网络配置需手动迁移".into());
            }
            if value.get("read-only").and_then(|v| v.as_bool()) == Some(true)
                && kind != DatabaseKind::Sqlite
            {
                return Err("只读限制需手动迁移".into());
            }
            if cfg
                .get("auth-model")
                .and_then(|v| v.as_str())
                .is_some_and(|s| s != "native" && !s.is_empty())
            {
                return Err("认证方式需手动迁移".into());
            }
            let mut p = defaults(kind);
            p.name = text(value, "name");
            p.group = text(value, "folder");
            p.environment = text(cfg, "type");
            if p.environment.is_empty() {
                p.environment = "dev".into();
            }
            p.host = text(cfg, "host");
            p.database = text(cfg, "database");
            p.user = text(cfg, "user");
            if let Some(auth) = cfg.get("auth-properties") {
                let mut user = text(auth, "userName");
                if user.is_empty() {
                    user = text(auth, "user");
                }
                if !user.is_empty() {
                    p.user = user;
                }
            }
            if p.user.is_empty() {
                p.user = text(cfg, "userName");
            }
            if let Some(port) = cfg.get("port") {
                let port = port
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| port.to_string());
                if !port.is_empty() {
                    p.port = port.parse().map_err(|_| "端口无效")?;
                }
            }
            let url = text(cfg, "url");
            if !url.is_empty() {
                url_fields(&mut p, &url)?;
            }
            if kind == DatabaseKind::Sqlite {
                if p.sqlite_path.is_empty() {
                    p.sqlite_path = p.database.clone();
                }
                p.sqlite_read_only = value
                    .get("read-only")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
            }
            if cfg
                .get("properties")
                .and_then(|v| v.as_object())
                .is_some_and(|m| {
                    m.keys().any(|k| {
                        let k = k.to_lowercase();
                        k.contains("ssl")
                            || k.contains("tls")
                            || k.contains("socket")
                            || k.contains("proxy")
                            || k.contains("ssh")
                    })
                })
            {
                return Err("安全或网络参数需手动迁移".into());
            }
            finish(p)
        })();
        match result {
            Ok(p) => report.profiles.push(p),
            Err(_) => report.skipped += 1,
        }
    }
    Ok(())
}
fn ncx_profiles(bytes: &[u8], fallback: DatabaseKind, report: &mut Report) -> Result<(), String> {
    let text = if bytes.starts_with(&[255, 254]) {
        let (text, _, bad) = encoding_rs::UTF_16LE.decode(bytes);
        if bad {
            return Err("NCX 编码无效".into());
        }
        text.into_owned()
    } else if bytes.starts_with(&[254, 255]) {
        let (text, _, bad) = encoding_rs::UTF_16BE.decode(bytes);
        if bad {
            return Err("NCX 编码无效".into());
        }
        text.into_owned()
    } else {
        std::str::from_utf8(bytes)
            .map_err(|_| "NCX 编码无效")?
            .trim_start_matches('\u{feff}')
            .to_string()
    };
    let doc = roxmltree::Document::parse(&text).map_err(|_| "NCX XML 格式无效，不支持 DTD")?;
    let nodes = doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name().eq_ignore_ascii_case("Connection"))
        .collect::<Vec<_>>();
    if nodes.len() > 200 {
        return Err("单次最多导入 200 个连接".into());
    }
    for node in nodes {
        let result = (|| {
            let attr = |name: &str| {
                node.attributes()
                    .find(|a| a.name().eq_ignore_ascii_case(name))
                    .map(|a| a.value())
                    .unwrap_or("")
            };
            if [
                "SSH",
                "UseSSH",
                "SSHEnabled",
                "UseSSL",
                "SSLEnabled",
                "UseProxy",
                "UseSocket",
                "UseNamedPipe",
                "UseHTTP",
                "UseHTTPTunnel",
            ]
            .iter()
            .any(|k| matches!(attr(k).to_lowercase().as_str(), "1" | "true" | "yes"))
            {
                return Err("网络配置需手动迁移".into());
            }
            let ty = if !attr("ConnType").is_empty() {
                attr("ConnType")
            } else {
                attr("ConnectionType")
            };
            let kind = if ty.is_empty() || ty.chars().all(|c| c.is_ascii_digit()) {
                fallback
            } else {
                kind(ty).ok_or("不支持的数据库类型")?
            };
            let mut p = defaults(kind);
            p.name = attr("ConnectionName").into();
            p.host = if attr("Host").is_empty() {
                attr("HostName")
            } else {
                attr("Host")
            }
            .into();
            p.database = attr("Database").into();
            p.user = attr("UserName").into();
            p.group = attr("Group").into();
            p.environment = "unknown".into();
            if !attr("Port").is_empty() {
                p.port = attr("Port").parse().map_err(|_| "端口无效")?;
            }
            if attr("Encoding")
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic())
                && mysql::valid_charset(attr("Encoding"))
            {
                p.charset = attr("Encoding").into();
            }
            if kind == DatabaseKind::Sqlite {
                p.sqlite_path = if attr("FileName").is_empty() {
                    attr("Database")
                } else {
                    attr("FileName")
                }
                .into();
                p.sqlite_read_only = true;
            }
            finish(p)
        })();
        match result {
            Ok(p) => report.profiles.push(p),
            Err(_) => report.skipped += 1,
        }
    }
    Ok(())
}
pub fn parse(bytes: &[u8], source: Source) -> Result<Report, String> {
    if bytes.len() > 8 * 1024 * 1024 {
        return Err("连接文件超过 8 MiB".into());
    }
    let mut report = Report {
        profiles: Vec::new(),
        skipped: 0,
    };
    match source {
        Source::Navicat(kind) => ncx_profiles(bytes, kind, &mut report)?,
        Source::DBeaver => json_profiles(bytes, &mut report)?,
    }
    Ok(report)
}
pub fn read(path: &Path, source: Source) -> Result<Report, String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|_| "无法读取连接文件")?
        .take(8 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "无法读取连接文件")?;
    if matches!(source, Source::DBeaver) && bytes.starts_with(b"PK") {
        let mut archive =
            zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|_| "DBeaver ZIP 无效")?;
        let mut report = Report {
            profiles: Vec::new(),
            skipped: 0,
        };
        let mut total = 0;
        for i in 0..archive.len().min(2000) {
            let mut entry = archive.by_index(i).map_err(|_| "ZIP 条目无效")?;
            if entry.name().rsplit('/').next() != Some("data-sources.json") {
                continue;
            }
            let mut bytes = Vec::new();
            entry
                .by_ref()
                .take(8 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| "无法读取连接配置")?;
            total += bytes.len();
            if total > 8 * 1024 * 1024 {
                return Err("连接配置超过 8 MiB".into());
            }
            json_profiles(&bytes, &mut report)?;
        }
        if report.profiles.len() > 200 {
            return Err("单次最多导入 200 个连接".into());
        }
        return Ok(report);
    }
    parse(&bytes, source)
}
pub fn unique_names(profiles: &mut [Profile], existing: &[Profile]) {
    let mut names = existing
        .iter()
        .map(|p| p.name.to_lowercase())
        .collect::<HashSet<_>>();
    for p in profiles {
        let base = p.name.clone();
        let mut n = 2;
        while !names.insert(p.name.to_lowercase()) {
            p.name = format!("{base}-{n}");
            n += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ncx_imports_basic_fields_and_rejects_dtd() {
        let secret = uuid::Uuid::new_v4().to_string();
        let xml = format!(
            "<Connections><Connection ConnectionName=\"Example\" Host=\"localhost\" Port=\"3306\" UserName=\"tester\" Database=\"demo\" Password=\"{secret}\"/></Connections>"
        );
        let report = parse(xml.as_bytes(), Source::Navicat(DatabaseKind::MySql)).unwrap();
        assert_eq!(report.profiles.len(), 1);
        let profile = &report.profiles[0];
        assert_eq!(profile.database, "demo");
        assert!(!profile.remember_password);
        assert!(!serde_json::to_string(profile).unwrap().contains(&secret));
        assert!(parse(b"<!DOCTYPE Connections [<!ENTITY value SYSTEM 'file:///not-readable'>]><Connections/>",Source::Navicat(DatabaseKind::MySql)).is_err());
    }
    #[test]
    fn dbeaver_zip_reads_only_connection_definitions() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("project.dbp");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        zip.start_file(
            "project/.dbeaver/data-sources.json",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(br#"{"connections":{"a":{"provider":"mysql","name":"ZIP connection","configuration":{"host":"localhost","database":"demo","auth-properties":{"userName":"tester"}}}}}"#).unwrap();
        zip.start_file(
            "project/.dbeaver/credentials-config.json",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(b"not a connection document").unwrap();
        zip.finish().unwrap();
        let report = read(&path, Source::DBeaver).unwrap();
        assert_eq!(report.profiles.len(), 1);
        assert_eq!(report.profiles[0].name, "ZIP connection");
    }
    #[test]
    fn dbeaver_preserves_url_precedence_and_skips_unsupported_security() {
        let secret = uuid::Uuid::new_v4().to_string();
        let doc = serde_json::json!({"connections": {
            "a":{"provider":"mysql","name":"Example","folder":"Work","configuration":{"host":"ignored","database":"ignored","url":"jdbc:mysql://localhost:3307/demo","config-profile":null,"auth-properties":{"userName":"tester","password":secret}}},
            "b":{"provider":"postgresql","name":"Tunnel","configuration":{"host":"localhost","handlers":{"ssh":{"enabled":true}}}},
            "c":{"provider":"mysql","name":"Missing user","configuration":{"host":"localhost","database":"demo"}}
        }});
        let report = parse(&serde_json::to_vec(&doc).unwrap(), Source::DBeaver).unwrap();
        assert_eq!(report.profiles.len(), 2);
        assert_eq!(report.skipped, 1);
        let p = report
            .profiles
            .iter()
            .find(|p| p.name == "Example")
            .unwrap();
        assert_eq!(p.port, 3307);
        assert_eq!(p.database, "demo");
        assert_eq!(p.group, "Work");
        assert!(!serde_json::to_string(p).unwrap().contains(&secret));
        assert!(
            report
                .profiles
                .iter()
                .find(|p| p.name == "Missing user")
                .unwrap()
                .user
                .is_empty()
        );
        let mut existing = defaults(DatabaseKind::MySql);
        existing.name = "Example".into();
        let mut profiles = report.profiles;
        unique_names(&mut profiles, &[existing]);
        assert!(profiles.iter().any(|p| p.name == "Example-2"));
    }
}

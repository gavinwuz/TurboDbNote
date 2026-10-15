//! Bounded, local-only URL import. Never return input or credentials in errors.
use crate::{
    driver_catalog,
    mysql::{Profile, TlsMode},
};

pub struct Draft {
    pub profile: Profile,
    pub password: String,
    pub suggested_database: bool,
    pub notices: Vec<&'static str>,
}
pub struct Row {
    pub line: usize,
    pub draft: Result<Draft, &'static str>,
}
fn decode(value: &str) -> Result<String, &'static str> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3).ok_or("url.error.invalid")?;
            let hex = std::str::from_utf8(hex).map_err(|_| "url.error.invalid")?;
            out.push(u8::from_str_radix(hex, 16).map_err(|_| "url.error.invalid")?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let text = String::from_utf8(out).map_err(|_| "url.error.invalid")?;
    if text.chars().any(char::is_control) {
        return Err("url.error.invalid");
    }
    Ok(text)
}
pub fn parse(text: &str) -> Result<Vec<Row>, &'static str> {
    if text.len() > 65_536 || text.lines().count() > 100 {
        return Err("url.error.limit");
    }
    Ok(text
        .lines()
        .enumerate()
        .filter(|(_, s)| !s.trim().is_empty())
        .map(|(line, text)| Row {
            line: line + 1,
            draft: one(text.trim()),
        })
        .collect())
}
fn one(text: &str) -> Result<Draft, &'static str> {
    let scheme = text.split_once("://").ok_or("url.error.invalid")?.0;
    let id = match scheme {
        "mysql" | "mariadb" | "postgresql" | "mongodb" | "redis" | "clickhouse" => scheme,
        "postgres" => "postgresql",
        "rediss" => "redis",
        _ => return Err("url.error.scheme"),
    };
    let rest = text.split_once("://").ok_or("url.error.invalid")?.1;
    let (url, go_credentials) = if matches!(id, "mysql" | "mariadb")
        && let Some((auth, transport)) = rest.rsplit_once("@tcp(")
    {
        let (address, path) = transport.split_once(")/").ok_or("url.error.invalid")?;
        if address.is_empty() {
            return Err("url.error.invalid");
        }
        let url = reqwest::Url::parse(&format!("{scheme}://{address}/{path}"))
            .map_err(|_| "url.error.invalid")?;
        if !url.username().is_empty() || url.password().is_some() {
            return Err("url.error.invalid");
        }
        (url, Some(auth.split_once(':').unwrap_or((auth, ""))))
    } else {
        (
            reqwest::Url::parse(text).map_err(|_| "url.error.invalid")?,
            None,
        )
    };
    if url.fragment().is_some() {
        return Err("url.error.invalid");
    }
    let mut profile = driver_catalog::find(id)
        .ok_or("url.error.scheme")?
        .profile();
    profile.host = url
        .host_str()
        .ok_or("url.error.invalid")?
        .trim_matches(['[', ']'])
        .to_owned();
    if profile.host.contains(',') {
        return Err("url.error.topology");
    }
    profile.port = url.port().unwrap_or(if id == "clickhouse" {
        8123
    } else {
        profile.port
    });
    if profile.port == 0 {
        return Err("url.error.invalid");
    }
    profile.user = match go_credentials {
        Some((user, _)) => user.to_owned(),
        None => decode(url.username())?,
    };
    let password = match go_credentials {
        Some((_, password)) => password.to_owned(),
        None => decode(url.password().unwrap_or_default())?,
    };
    if profile.user.chars().any(char::is_control) || password.chars().any(char::is_control) {
        return Err("url.error.invalid");
    }
    profile.database = decode(url.path().strip_prefix('/').unwrap_or(url.path()))?;
    if profile.database.contains('/') {
        return Err("url.error.invalid");
    }
    profile.tls = if scheme == "rediss" {
        TlsMode::VerifyIdentity
    } else {
        TlsMode::Disabled
    };
    profile.mongo_auth_source.clear();
    profile.remember_password = cfg!(windows);
    profile.allow_old_auth = id == "mysql";
    profile.group.clear();
    profile.name.clear();
    let mut seen = std::collections::HashSet::new();
    let mut notices = Vec::new();
    for pair in url
        .query()
        .unwrap_or_default()
        .split('&')
        .filter(|s| !s.is_empty())
    {
        let (key, value) = pair.split_once('=').ok_or("url.error.parameter")?;
        let key = decode(&key.replace('+', "%20"))?;
        let value = decode(&value.replace('+', "%20"))?;
        let identity = if matches!(key.as_str(), "ssl" | "tls") {
            "tls"
        } else {
            key.as_str()
        };
        if !seen.insert(identity.to_owned()) {
            return Err("url.error.parameter");
        }
        match key.as_str() {
            "authSource" if id == "mongodb" && !value.is_empty() => {
                profile.mongo_auth_source = value
            }
            "tls" | "ssl" => {
                profile.tls = match value.as_str() {
                    "true" => TlsMode::VerifyIdentity,
                    "false" => TlsMode::Disabled,
                    _ => return Err("url.error.parameter"),
                }
            }
            "charset" if matches!(id, "mysql" | "mariadb") && !value.is_empty() => {
                profile.charset = value
            }
            "interpolateParams" | "parseTime"
                if matches!(id, "mysql" | "mariadb")
                    && (value.eq_ignore_ascii_case("true")
                        || value.eq_ignore_ascii_case("false")) =>
            {
                if !notices.contains(&"url.go_client_options") {
                    notices.push("url.go_client_options");
                }
            }
            "loc"
                if matches!(id, "mysql" | "mariadb") && !value.is_empty() && value.len() <= 128 =>
            {
                if !notices.contains(&"url.go_client_options") {
                    notices.push("url.go_client_options");
                }
            }
            _ => return Err("url.error.parameter"),
        }
    }
    if scheme == "rediss" && profile.tls == TlsMode::Disabled {
        return Err("url.error.parameter");
    }
    if id == "clickhouse" && url.port().is_none() && profile.tls != TlsMode::Disabled {
        profile.port = 8443;
    }
    let suggested_database =
        id == "mongodb" && profile.database.is_empty() && !profile.mongo_auth_source.is_empty();
    if suggested_database {
        profile.database = profile.mongo_auth_source.clone();
    }
    // Freeze URI authentication semantics before a suggested target or later edits change it.
    if id == "mongodb" && profile.mongo_auth_source.is_empty() {
        profile.mongo_auth_source = if profile.database.is_empty() {
            "admin".into()
        } else {
            profile.database.clone()
        };
    }
    if id == "redis" {
        if profile.database.is_empty() {
            profile.database = "0".into();
        }
        profile
            .database
            .parse::<u32>()
            .map_err(|_| "url.error.invalid")?;
    }
    let mut validation = profile.clone();
    validation.name = "url-import".into();
    if validation.user.is_empty() {
        validation.user = "url-import".into();
    }
    validation.validate().map_err(|_| "url.error.invalid")?;
    Ok(Draft {
        profile,
        password,
        suggested_database,
        notices,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn imports_go_tcp_wrapper_and_surfaces_client_only_options() {
        let rows=parse("mysql://reader:***@tcp(127.0.0.1:3307)/app?interpolateParams=true&charset=utf8mb4&parseTime=True&loc=Local").unwrap();
        let draft = rows[0].draft.as_ref().ok().unwrap();
        assert_eq!(draft.profile.host, "127.0.0.1");
        assert_eq!(draft.profile.port, 3307);
        assert_eq!(draft.profile.database, "app");
        assert_eq!(draft.profile.charset, "utf8mb4");
        assert!(draft.password == "***");
        assert_eq!(draft.notices, vec!["url.go_client_options"]);
        let ipv6 = parse("mysql://reader@tenant:***@tcp([::1]:3306)/app").unwrap();
        let draft = ipv6[0].draft.as_ref().ok().unwrap();
        assert_eq!(draft.profile.host, "::1");
        assert_eq!(draft.profile.user, "reader@tenant");
        for url in [
            "mysql://reader:***@tcp(host:3306/app",
            "mysql://host/app?parseTime=invalid",
            "mysql://host/app?multiStatements=true",
            "mysql://host/app?parseTime=true&parseTime=false",
        ] {
            assert!(parse(url).unwrap()[0].draft.is_err());
        }
    }
    #[test]
    fn imports_multiple_schemes_and_decodes_credentials_without_plus_conversion() {
        let rows = parse("mysql://u:p%40ss+word@localhost/db\nredis://localhost/0\nclickhouse://localhost/data\nmongodb://u:p@localhost/?authSource=auth").unwrap();
        let a = rows[0].draft.as_ref().ok().unwrap();
        assert_eq!(a.password, "p@ss+word");
        assert_eq!(a.profile.port, 3306);
        assert_eq!(rows[2].draft.as_ref().ok().unwrap().profile.port, 8123);
        let m = rows[3].draft.as_ref().ok().unwrap();
        assert_eq!(m.profile.mongo_auth_source, "auth");
        assert_eq!(m.profile.database, "auth");
        assert!(m.suggested_database);
    }
    #[test]
    fn respects_independent_authentication_and_secure_scheme() {
        let rows = parse("mongodb://u:p@localhost/business?authSource=auth\nmongodb://localhost/business\nmongodb://localhost/\nrediss://localhost/0").unwrap();
        assert_eq!(
            rows[0].draft.as_ref().ok().unwrap().profile.database,
            "business"
        );
        assert_eq!(
            rows[1]
                .draft
                .as_ref()
                .ok()
                .unwrap()
                .profile
                .mongo_auth_source,
            "business"
        );
        assert_eq!(
            rows[2]
                .draft
                .as_ref()
                .ok()
                .unwrap()
                .profile
                .mongo_auth_source,
            "admin"
        );
        assert_eq!(
            rows[3].draft.as_ref().ok().unwrap().profile.tls,
            TlsMode::VerifyIdentity
        );
    }
    #[test]
    fn rejects_unsupported_parameters_and_malformed_inputs_without_leaking_them() {
        for url in [
            "mongodb+srv://secret@host/",
            "mongodb://u:secret@host/?replicaSet=x",
            "mysql://u:secret%GG@host/db",
            "mysql://host/db?tls=true&tls=false",
            "mysql://host/db#secret",
        ] {
            assert!(parse(url).unwrap()[0].draft.is_err());
        }
        assert!(parse(&"x".repeat(65_537)).is_err());
    }
    #[test]
    fn rejects_lossy_query_decoding_and_topology_and_keeps_ipv6() {
        for url in [
            "mongodb://host/?authSource=%GG",
            "mongodb://host/?authSource=%FF",
            "mongodb://host1,host2/db",
            "rediss://host/0?tls=false",
            "mysql://host/db?tls=true&ssl=false",
        ] {
            assert!(parse(url).unwrap()[0].draft.is_err());
        }
        let rows =
            parse("mysql://u:p@[::1]:3307/%E6%95%B0%E6%8D%AE\nclickhouse://localhost/db?tls=true")
                .unwrap();
        assert_eq!(rows[0].draft.as_ref().ok().unwrap().profile.host, "::1");
        assert_eq!(
            rows[0].draft.as_ref().ok().unwrap().profile.database,
            "数据"
        );
        assert_eq!(rows[1].draft.as_ref().ok().unwrap().profile.port, 8443);
    }
}

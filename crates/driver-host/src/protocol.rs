use serde_json::{Value, json};
pub type Result<T> = std::result::Result<T, &'static str>;
pub const LIMIT: usize = 768 * 1024;
pub const FAILED: &str = "drivers.operation_failed";
pub const UNSUPPORTED: &str = "drivers.operation_unsupported";
pub const TOO_LARGE: &str = "drivers.value_limit";
pub fn string<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}
pub fn validate_profile(p: &Value) -> Result<()> {
    if ![
        "sqlserver",
        "access",
        "oracle",
        "db2",
        "mongodb",
        "elasticsearch",
        "tdengine",
        "cassandra",
    ]
    .contains(&string(p, "driver_id"))
    {
        return Err("drivers.invalid_identity");
    }
    if !p["port"].as_u64().is_some_and(|n| (1..=65535).contains(&n))
        || !p["timeout_seconds"]
            .as_u64()
            .is_some_and(|n| (1..=60).contains(&n))
        || !["Disabled", "Required", "VerifyIdentity"].contains(&string(p, "tls"))
    {
        return Err("drivers.invalid_target");
    }
    for key in ["host", "database", "user"] {
        let text = string(p, key);
        if text.len() > 1024 || text.chars().any(char::is_control) {
            return Err("drivers.invalid_target");
        }
    }
    if string(p, "driver_id") != "access" && string(p, "host").is_empty() {
        return Err("drivers.invalid_target");
    }
    if string(p, "client_cert").is_empty() != string(p, "client_key").is_empty() {
        return Err("drivers.tls_options_unsupported");
    }
    Ok(())
}
pub fn quote(s: &str, driver: &str) -> Result<String> {
    if s.is_empty() || s.len() > 1024 || s.chars().any(char::is_control) {
        return Err("drivers.invalid_target");
    }
    Ok(match driver {
        "access" | "sqlserver" => format!("[{}]", s.replace(']', "]]")),
        "tdengine" => format!("`{}`", s.replace('`', "``")),
        _ => format!("\"{}\"", s.replace('"', "\"\"")),
    })
}
pub fn literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}
pub fn qualified(o: &Value, driver: &str) -> Result<String> {
    let name = quote(string(o, "name"), driver)?;
    let schema = string(o, "schema");
    Ok(if schema.is_empty() {
        name
    } else {
        format!("{}.{}", quote(schema, driver)?, name)
    })
}
pub fn empty() -> Value {
    json!({"columns":[],"values":[]})
}
pub fn schema(columns: Vec<Value>, ddl: String) -> Value {
    json!({"columns":columns,"ddl":ddl,"indexes":empty(),"foreign_keys":empty(),"triggers":empty()})
}
pub fn column(name: &str, ty: &str, comment: &str) -> Value {
    json!({"name":name,"data_type":ty,"comment":comment,"primary":false})
}
pub fn object(schema: &str, name: &str, kind: &str) -> Value {
    json!({"schema":schema,"name":name,"kind":kind,"comment":"","parent":""})
}
pub fn cell(v: &Value) -> Result<Option<String>> {
    let value = match v {
        Value::Null => return Ok(None),
        Value::String(s) => s.clone(),
        _ => v.to_string(),
    };
    if value.len() > 65536 {
        return Err(TOO_LARGE);
    }
    Ok(Some(value))
}
#[derive(Default)]
pub struct Rows {
    pub columns: Vec<String>,
    pub values: Vec<Vec<Option<String>>>,
    bytes: usize,
}
impl Rows {
    pub fn new(columns: Vec<String>) -> Result<Self> {
        if columns.len() > 512 {
            return Err(TOO_LARGE);
        }
        Ok(Self {
            columns,
            ..Self::default()
        })
    }
    pub fn push(&mut self, row: Vec<Option<String>>) -> Result<()> {
        if row.len() != self.columns.len() {
            return Err(FAILED);
        }
        for s in row.iter().flatten() {
            if s.len() > 65536 {
                return Err(TOO_LARGE);
            }
            self.bytes += s.len();
        }
        if self.bytes > LIMIT || self.values.len() >= 1001 {
            return Err(TOO_LARGE);
        }
        self.values.push(row);
        Ok(())
    }
    pub fn json(self) -> Value {
        json!({"columns":self.columns,"values":self.values})
    }
}
pub fn page_options(o: &Value) -> Result<(usize, u64)> {
    let size = o
        .get("page_size")
        .map_or(Some(200), Value::as_u64)
        .ok_or(TOO_LARGE)?;
    let offset = o
        .get("offset")
        .map_or(Some(0), Value::as_u64)
        .ok_or(TOO_LARGE)?;
    if !(1..=1000).contains(&size) || offset > 100000 {
        return Err(TOO_LARGE);
    }
    if ["filter", "sort", "after"].iter().any(|k| !o[*k].is_null()) {
        return Err(UNSUPPORTED);
    }
    Ok((size as usize, offset))
}
// Validate before changing database context or touching a server.
pub fn allowed(kind: &Value) -> Result<()> {
    if kind == "Databases" || kind.get("Objects").is_some() {
        return Ok(());
    }
    let a = &kind["Workbench"]["action"];
    if a == "Catalog" {
        return Ok(());
    }
    if let Some(map) = a.as_object()
        && map.len() == 1
        && map
            .keys()
            .all(|k| ["Count", "GridPage", "Describe", "Ddl", "NoteColumns"].contains(&k.as_str()))
    {
        if let Some(v) = map.get("GridPage") {
            page_options(&v["options"])?;
        }
        if let Some(v) = map.get("Count")
            && !v["filter"].is_null()
        {
            return Err(UNSUPPORTED);
        }
        return Ok(());
    }
    Err(UNSUPPORTED)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_writes_and_invalid_pages_before_dispatch() {
        assert!(allowed(&json!({"Workbench":{"database":"other","action":{"Execute":{"sql":"DELETE FROM t"}}}})).is_err());
        assert!(page_options(&json!({"page_size":1001})).is_err());
        assert!(page_options(&json!({"page_size":-1})).is_err());
        assert!(page_options(&json!({"filter":{"sql":"1=1"}})).is_err());
    }
    #[test]
    fn quotes_and_limits_are_lossless() {
        assert_eq!(quote("a]b", "sqlserver").unwrap(), "[a]]b]");
        let mut r = Rows::new(vec!["x".into()]).unwrap();
        assert!(r.push(vec![Some("a".repeat(65537))]).is_err());
        assert!(quote("a\n", "oracle").is_err());
    }
    #[test]
    fn aggregate_budget_rejects_instead_of_truncating() {
        let mut rows = Rows::new(vec!["x".into()]).unwrap();
        for _ in 0..12 {
            rows.push(vec![Some("x".repeat(65536))]).unwrap();
        }
        assert!(rows.push(vec![Some("x".into())]).is_err());
        assert!(Rows::new(vec!["x".into(); 513]).is_err());
        let mut rows = Rows::new(vec!["x".into()]).unwrap();
        rows.push(vec![None]).unwrap();
        rows.push(vec![Some(String::new())]).unwrap();
        assert_eq!(rows.json()["values"], json!([[null], [""]]));
    }
    #[test]
    fn profile_rejects_untyped_ports_and_unknown_tls() {
        let mut p = json!({"driver_id":"sqlserver","host":"localhost","port":1433,"tls":"VerifyIdentity","timeout_seconds":5});
        assert!(validate_profile(&p).is_ok());
        p["port"] = json!("1433;other");
        assert!(validate_profile(&p).is_err());
        p["port"] = json!(1433);
        p["tls"] = json!("unknown");
        assert!(validate_profile(&p).is_err());
    }
}

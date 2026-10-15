//! Non-secret ODBC configuration. Credentials use the application's secret channel.
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    pub dsn: String,
    pub driver: String,
    /// One non-secret KEY=VALUE attribute per line, not a connection string.
    pub parameters: String,
}
impl Options {
    pub fn attributes(&self) -> Result<Vec<(String, String)>, &'static str> {
        let invalid = "odbc.invalid_options";
        if self.dsn.is_empty() == self.driver.is_empty()
            || [&self.dsn, &self.driver]
                .iter()
                .any(|v| v.len() > 256 || v.chars().any(char::is_control))
            || self.parameters.len() > 8192
        {
            return Err(invalid);
        }
        let mut result = Vec::new();
        for line in self.parameters.lines().filter(|s| !s.trim().is_empty()) {
            let (key, value) = line.split_once('=').ok_or(invalid)?;
            let key = key.trim().to_ascii_uppercase();
            let value = value.trim();
            // Explicit whitelist keeps secrets and file/plugin-loading directives out of storage.
            if ![
                "SERVER",
                "HOSTNAME",
                "PORT",
                "DATABASE",
                "DBQ",
                "PROTOCOL",
                "CHARSET",
                "SSL",
                "SSLMODE",
                "SECURITY",
                "ENCRYPT",
                "TRUSTSERVERCERTIFICATE",
                "AUTHENTICATION",
                "APPLICATIONNAME",
            ]
            .contains(&key.as_str())
                || value.is_empty()
                || value.len() > 1024
                || value.chars().any(char::is_control)
                || result.iter().any(|(k, _)| k == &key)
                || result.len() >= 16
            {
                return Err(invalid);
            }
            result.push((key, value.to_owned()));
        }
        Ok(result)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn options_exclude_secrets_and_ambiguous_targets() {
        let mut o = Options {
            dsn: "sample".into(),
            ..Default::default()
        };
        assert!(o.attributes().is_ok());
        for value in [
            "PWD=hidden",
            "TOKEN=hidden",
            "FILEDSN=a",
            "SERVER=a\nserver=b",
            "broken",
        ] {
            o.parameters = value.into();
            assert!(o.attributes().is_err());
        }
        o.parameters = "SERVER=localhost\nDATABASE=sample".into();
        assert_eq!(o.attributes().unwrap().len(), 2);
        o.driver = "Other driver".into();
        assert!(o.attributes().is_err());
    }
}

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Region {
    #[default]
    China,
    Global,
}
impl Region {
    pub fn windows_host(self) -> &'static str {
        match self {
            Self::China => "pc.crashsight.qq.com",
            Self::Global => "pc.crashsight.wetest.net",
        }
    }
    pub fn mac_url(self) -> &'static str {
        match self {
            Self::China => "https://mac.crashsight.qq.com/pb/sync",
            Self::Global => "https://mac.crashsight.wetest.net/pb/sync",
        }
    }
}

// Deliberately not Debug: an analytics token is a separate, optional service credential.
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub enabled: bool,
    pub app_id: String,
    pub region: Region,
    pub fallback: Option<Destination>,
    pub record_usage: bool,
    pub analytics_endpoint: Option<String>,
    #[serde(skip_serializing)]
    pub analytics_token: Option<String>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Destination {
    pub app_id: String,
    pub region: Region,
}
impl Config {
    pub fn from_json(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() > 16_384 {
            return Err("Diagnostics configuration is too large");
        }
        let config: Self =
            serde_json::from_slice(bytes).map_err(|_| "Invalid diagnostics configuration")?;
        if config.enabled
            && (config.app_id.is_empty()
                || config.app_id.len() > 64
                || !config.app_id.bytes().all(|b| b.is_ascii_alphanumeric()))
        {
            return Err("Invalid CrashSight App ID");
        }
        if let Some(fallback) = &config.fallback
            && (fallback.app_id.is_empty()
                || fallback.app_id.len() > 64
                || !fallback.app_id.bytes().all(|b| b.is_ascii_alphanumeric()))
        {
            return Err("Invalid fallback App ID");
        }
        if let Some(endpoint) = &config.analytics_endpoint {
            validate_endpoint(endpoint)?;
        }
        if config
            .analytics_token
            .as_ref()
            .is_some_and(|token| token.contains(['\r', '\n']) || token.len() > 4096)
        {
            return Err("Invalid analytics credential");
        }
        Ok(config)
    }
}
pub(crate) fn validate_endpoint(value: &str) -> Result<reqwest::Url, &'static str> {
    let url = reqwest::Url::parse(value).map_err(|_| "Invalid analytics endpoint")?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Analytics endpoint must be HTTPS without credentials, query or fragment");
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_credentials_in_url_and_accidental_app_key() {
        assert!(Config::from_json(br#"{"app_key":"do-not-ship"}"#).is_err());
        for url in [
            "http://example.com",
            "https://user:secret@example.com",
            "https://example.com?key=secret",
            "file:///tmp/events",
        ] {
            assert!(validate_endpoint(url).is_err());
        }
        assert!(validate_endpoint("https://example.com/events").is_ok());
    }
    #[test]
    fn domestic_hosts_are_platform_specific() {
        assert_eq!(Region::China.windows_host(), "pc.crashsight.qq.com");
        assert_eq!(
            Region::China.mac_url(),
            "https://mac.crashsight.qq.com/pb/sync"
        );
    }
}

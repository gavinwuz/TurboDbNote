use crate::preferences::ProviderPreferences;
use serde_json::Value;
use std::{io::Read, time::Duration};

pub const REPOSITORY: &str = "https://github.com/gavinwuz/TurboDbNote";
pub const RELEASES: &str = "https://github.com/gavinwuz/TurboDbNote/releases";

fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("TurboDbNote/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| "Unable to create HTTP client".into())
}
fn json(response: reqwest::blocking::Response) -> Result<Value, String> {
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    let mut bytes = Vec::new();
    response
        .take(2_097_153)
        .read_to_end(&mut bytes)
        .map_err(|_| "Unable to read response")?;
    if bytes.len() > 2_097_152 {
        return Err("Response exceeds 2 MiB".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "Invalid JSON response".into())
}
pub fn endpoint(value: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(value.trim()).map_err(|_| "Invalid API Endpoint")?;
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if !(url.scheme() == "https" || (url.scheme() == "http" && local))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "Use HTTPS (HTTP is allowed for localhost); omit credentials, query and fragment"
                .into(),
        );
    }
    Ok(url)
}
pub fn validate_provider(provider: &ProviderPreferences) -> Result<(), String> {
    if provider.display_name.trim().is_empty() {
        return Err("Display name is required".into());
    }
    endpoint(&provider.endpoint)?;
    if ![
        "OpenAI Chat Completions",
        "OpenAI Responses",
        "Anthropic Messages",
    ]
    .contains(&provider.api_mode.as_str())
    {
        return Err("Unsupported API mode".into());
    }
    let advanced: Value = serde_json::from_str(&provider.advanced)
        .map_err(|_| "Advanced parameters must be a JSON object")?;
    if !advanced.is_object() {
        return Err("Advanced parameters must be a JSON object".into());
    }
    if advanced.get("model").is_some()
        || advanced.get("messages").is_some()
        || advanced.get("input").is_some()
    {
        return Err("Advanced parameters cannot override model, messages or input".into());
    }
    Ok(())
}
pub fn models(provider: &ProviderPreferences) -> Result<Vec<String>, String> {
    validate_provider(provider)?;
    let mut url = endpoint(&provider.endpoint)?;
    let base = url.path().trim_end_matches('/').to_string();
    url.set_path(&format!("{base}/models"));
    let client = client()?;
    let mut request = client.get(url);
    if provider.api_mode == "Anthropic Messages" {
        request = request.header("anthropic-version", "2023-06-01");
        if !provider.api_key.is_empty() {
            request = request.header("x-api-key", &provider.api_key);
        }
    } else if !provider.api_key.is_empty() {
        request = request.bearer_auth(&provider.api_key);
    }
    // Never expose request URLs, credentials or response bodies in error messages.
    let value = json(
        request
            .send()
            .map_err(|_| "Model request failed or timed out")?,
    )?;
    parse_models(&value)
}
fn parse_models(value: &Value) -> Result<Vec<String>, String> {
    let data = value
        .get("data")
        .and_then(Value::as_array)
        .ok_or("Missing model data array")?;
    let mut models: Vec<String> = data
        .iter()
        .filter_map(|item| item.get("id")?.as_str().map(str::to_owned))
        .collect();
    models.sort();
    models.dedup();
    models.truncate(1000);
    Ok(models)
}
pub fn latest_release() -> Result<String, String> {
    let response = client()?
        .get("https://api.github.com/repos/gavinwuz/TurboDbNote/releases/latest")
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|_| "Update check failed or timed out")?;
    if response.status().as_u16() == 404 {
        return Ok("No published stable release".into());
    }
    let value = json(response)?;
    let tag = value
        .get("tag_name")
        .and_then(Value::as_str)
        .ok_or("Missing release version")?;
    let latest = semver::Version::parse(tag.trim_start_matches('v'))
        .map_err(|_| "Invalid release version")?;
    let current = semver::Version::parse(env!("CARGO_PKG_VERSION"))
        .map_err(|_| "Invalid application version")?;
    Ok(if latest > current {
        format!("New version available: {tag}")
    } else {
        format!("Up to date: v{current}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn model_requests_use_correct_path_and_protocol_headers() {
        use std::{io::Write, net::TcpListener};
        for mode in [
            "OpenAI Chat Completions",
            "OpenAI Responses",
            "Anthropic Messages",
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                let mut byte = [0];
                while !request.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).unwrap();
                    request.push(byte[0]);
                }
                let body = r#"{"data":[{"id":"model-a"}]}"#;
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .unwrap();
                String::from_utf8(request).unwrap().to_lowercase()
            });
            let provider = ProviderPreferences {
                endpoint: format!("http://{address}/v1"),
                api_mode: mode.into(),
                api_key: "test-only-key".into(),
                ..Default::default()
            };
            assert_eq!(models(&provider).unwrap(), vec!["model-a"]);
            let request = server.join().unwrap();
            assert!(request.starts_with("get /v1/models http/1.1"));
            if mode == "Anthropic Messages" {
                assert!(request.contains("x-api-key: test-only-key"));
                assert!(request.contains("anthropic-version: 2023-06-01"));
                assert!(!request.contains("authorization:"));
            } else {
                assert!(request.contains("authorization: bearer test-only-key"));
            }
        }
    }
    #[test]
    fn validates_endpoint_and_advanced_parameters() {
        assert!(endpoint("https://example.org/v1").is_ok());
        assert!(endpoint("http://localhost:11434/v1").is_ok());
        for url in [
            "http://example.org",
            "https://user:secret@example.org",
            "https://example.org?key=secret",
            "file:///tmp/models",
        ] {
            assert!(endpoint(url).is_err());
        }
        let p = ProviderPreferences {
            advanced: "[]".into(),
            ..Default::default()
        };
        assert!(validate_provider(&p).is_err());
    }
    #[test]
    fn model_list_is_sorted_deduplicated_and_validated() {
        assert_eq!(
            parse_models(&serde_json::json!({"data":[{"id":"b"},{"id":"a"},{"id":"b"}]})).unwrap(),
            vec!["a", "b"]
        );
        assert!(parse_models(&serde_json::json!({"error":"unauthorized"})).is_err());
    }
}

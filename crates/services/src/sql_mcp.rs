//! Local, bounded MCP transport. Database credentials never enter this module.
use async_channel::{Receiver, Sender};
use http_body_util::{BodyExt, Full, Limited};
use hyper::{
    Request as HttpRequest, Response, StatusCode,
    body::{Bytes, Incoming},
};
use serde_json::{Value, json};
use std::{convert::Infallible, sync::Arc, time::Duration};

#[derive(Clone)]
pub struct Config {
    pub url: String,
    token: String,
}
impl Config {
    fn codex_override(&self) -> String {
        format!(
            "mcp_servers={{turbodbnote={{url={},bearer_token_env_var=\"TURBODBN_MCP_TOKEN\",tool_timeout_sec=180,required=true,enabled_tools=[\"current_database\",\"query_sql\"],tools={{current_database={{approval_mode=\"approve\"}},query_sql={{approval_mode=\"approve\"}}}}}}}}",
            json!(self.url)
        )
    }
    pub fn configure_codex(&self, command: &mut tokio::process::Command) {
        command
            .arg("-c")
            .arg(self.codex_override())
            .env("TURBODBN_MCP_TOKEN", &self.token);
        for setting in [
            "features.shell_tool=false",
            "features.unified_exec=false",
            "features.shell_snapshot=false",
            "features.apps=false",
            "features.hooks=false",
            "features.multi_agent=false",
            "features.remote_plugin=false",
        ] {
            command.arg("-c").arg(setting);
        }
    }
    pub fn configure_claude(&self, command: &mut tokio::process::Command, args: &mut [String]) {
        if let Some(i) = args.iter().position(|v| v == "--mcp-config") {
            args[i + 1] = json!({"mcpServers":{"turbodbnote":{"type":"http","url":self.url,"headers":{"Authorization":"Bearer ${TURBODBN_MCP_TOKEN}"}}}}).to_string();
        }
        command.env("TURBODBN_MCP_TOKEN", &self.token);
    }
}
pub struct Call {
    pub sql: Option<String>,
    pub response: Sender<Result<Value, String>>,
}

pub fn result_data(page: crate::database::notebook_query::ResultPage) -> Value {
    let columns = page
        .rows
        .columns
        .iter()
        .take(64)
        .map(|s| s.chars().take(128).collect::<String>())
        .collect::<Vec<_>>();
    let per_value = if page.rows.columns.len() == 1 && page.rows.values.len() == 1 {
        32768
    } else {
        512
    };
    let mut remaining = 32768usize;
    let mut truncated = page.rows.columns.len() > 64 || page.rows.values.len() > 20;
    let rows = page
        .rows
        .values
        .iter()
        .take(20)
        .map(|row| {
            row.iter()
                .take(64)
                .enumerate()
                .map(|(i, value)| {
                    let name = page
                        .rows
                        .columns
                        .get(i)
                        .map(|s| s.to_ascii_lowercase())
                        .unwrap_or_default();
                    if [
                        "password",
                        "passwd",
                        "secret",
                        "token",
                        "api_key",
                        "private_key",
                    ]
                    .iter()
                    .any(|key| name.contains(key))
                    {
                        Some("***".to_string())
                    } else {
                        value.as_ref().map(|v| {
                            let mut text = String::new();
                            for c in v.chars().take(per_value) {
                                if c.len_utf8() > remaining {
                                    break;
                                }
                                remaining -= c.len_utf8();
                                text.push(c);
                            }
                            truncated |= text.len() < v.len();
                            text
                        })
                    }
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    json!({"columns":columns,"rows":rows,"more":page.more,"truncated":truncated,"elapsed_ms":page.elapsed_ms,"affected_rows":page.affected_rows,"execution":page.execution,"read_only":page.read_only,"limits":{"rows":20,"columns":64,"characters_per_value":per_value,"value_bytes":32768}})
}
pub struct Server {
    pub config: Config,
    stop: Sender<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.stop.try_send(());
    }
}
impl Server {
    pub fn start() -> Result<(Self, Receiver<Call>), String> {
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .map_err(|_| "mcp.unavailable")?;
        listener
            .set_nonblocking(true)
            .map_err(|_| "mcp.unavailable")?;
        let authority = listener
            .local_addr()
            .map_err(|_| "mcp.unavailable")?
            .to_string();
        let config = Config {
            url: format!("http://{authority}/mcp"),
            token: format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            ),
        };
        let auth = format!("Bearer {}", config.token);
        let (send, calls) = async_channel::bounded(2);
        let (stop, stopped) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            rt.block_on(async move {
                let Ok(listener) = tokio::net::TcpListener::from_std(listener) else { return; };
                let slots = Arc::new(tokio::sync::Semaphore::new(4));
                loop {
                    tokio::select! {
                        _ = stopped.recv() => break,
                        accepted = listener.accept() => {
                            let Ok((socket, _)) = accepted else { break; };
                            let Ok(slot) = slots.clone().try_acquire_owned() else { continue; };
                            let auth = auth.clone(); let authority = authority.clone(); let send = send.clone();
                            tokio::spawn(async move {
                                let _slot = slot;
                                let service = hyper::service::service_fn(move |request| handle(request, auth.clone(), authority.clone(), send.clone()));
                                let connection = hyper::server::conn::http1::Builder::new().max_buf_size(16384).serve_connection(hyper_util::rt::TokioIo::new(socket), service);
                                let _ = tokio::time::timeout(Duration::from_secs(180), connection).await;
                            });
                        }
                    }
                }
            });
        });
        Ok((Self { config, stop }, calls))
    }
}
fn response(status: StatusCode, value: Option<Value>) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header("Content-Type", "application/json")
        .header("Cache-Control", "no-store")
        .body(Full::new(Bytes::from(
            value.map(|v| v.to_string()).unwrap_or_default(),
        )))
        .unwrap()
}
async fn handle(
    request: HttpRequest<Incoming>,
    auth: String,
    authority: String,
    calls: Sender<Call>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let headers = request.headers();
    if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some(&auth)
        || headers.get("host").and_then(|v| v.to_str().ok()) != Some(&authority)
        || headers.contains_key("origin")
    {
        return Ok(response(StatusCode::FORBIDDEN, None));
    }
    if request.uri().path() != "/mcp" {
        return Ok(response(StatusCode::NOT_FOUND, None));
    }
    if request.method() != hyper::Method::POST {
        return Ok(response(StatusCode::METHOD_NOT_ALLOWED, None));
    }
    if headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_none_or(|s| s.split(';').next() != Some("application/json"))
    {
        return Ok(response(StatusCode::UNSUPPORTED_MEDIA_TYPE, None));
    }
    let body = tokio::time::timeout(
        Duration::from_secs(5),
        Limited::new(request.into_body(), 65536).collect(),
    )
    .await;
    let Ok(Ok(body)) = body else {
        return Ok(response(StatusCode::PAYLOAD_TOO_LARGE, None));
    };
    let Ok(request) = serde_json::from_slice::<Value>(&body.to_bytes()) else {
        return Ok(response(StatusCode::BAD_REQUEST, None));
    };
    Ok(rpc(request, calls).await)
}
async fn rpc(request: Value, calls: Sender<Call>) -> Response<Full<Bytes>> {
    let id = request.get("id").cloned();
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if request.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return response(StatusCode::BAD_REQUEST, None);
    }
    if id.is_none() {
        return response(StatusCode::ACCEPTED, None);
    }
    let result = match method {
        "initialize" => {
            json!({"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"TurboDBNote","version":env!("CARGO_PKG_VERSION")}})
        }
        "ping" => json!({}),
        "tools/list" => json!({"tools":[
            {"name":"current_database","description":"Get the database scope explicitly selected in TurboDBNote. No credentials are returned.","inputSchema":{"type":"object","properties":{},"additionalProperties":false},"annotations":{"readOnlyHint":true,"destructiveHint":false,"openWorldHint":false}},
            {"name":"query_sql","description":"Request one SQL statement in the selected database. TurboDBNote checks command policy and asks the user before execution and sharing results. Default is read-only. Never request credentials. Results are limited to 20 rows.","inputSchema":{"type":"object","properties":{"sql":{"type":"string","maxLength":60000}},"required":["sql"],"additionalProperties":false},"annotations":{"readOnlyHint":false,"destructiveHint":true}}
        ]}),
        "tools/call" => {
            let name = request["params"]["name"].as_str().unwrap_or_default();
            let args = &request["params"]["arguments"];
            let sql = args.get("sql").and_then(Value::as_str);
            let valid = match name {
                "current_database" => {
                    args.is_null() || args.as_object().is_some_and(|v| v.is_empty())
                }
                "query_sql" => {
                    sql.is_some_and(|s| !s.trim().is_empty() && s.len() <= 60000)
                        && args.as_object().is_some_and(|v| v.len() == 1)
                }
                _ => false,
            };
            if !valid {
                json!({"isError":true,"content":[{"type":"text","text":"Invalid tool arguments"}]})
            } else {
                let (response, receiver) = async_channel::bounded(1);
                let result = if calls
                    .try_send(Call {
                        sql: sql.map(str::to_owned),
                        response,
                    })
                    .is_err()
                {
                    Err("Tool busy".into())
                } else {
                    tokio::time::timeout(Duration::from_secs(150), receiver.recv())
                        .await
                        .ok()
                        .and_then(Result::ok)
                        .unwrap_or_else(|| {
                            Err(
                                "Request cancelled or timed out; do not automatically retry writes"
                                    .into(),
                            )
                        })
                };
                match result {
                    Ok(data) => json!({"content":[{"type":"text","text":data.to_string()}]}),
                    Err(error) => json!({"isError":true,"content":[{"type":"text","text":error}]}),
                }
            }
        }
        _ => {
            return response(
                StatusCode::OK,
                Some(
                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Method not found"}}),
                ),
            );
        }
    };
    response(
        StatusCode::OK,
        Some(json!({"jsonrpc":"2.0","id":id,"result":result})),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn codex_override_is_balanced_scoped_and_never_contains_the_token() {
        let config = Config {
            url: "http://127.0.0.1:12345/mcp".into(),
            token: "not-a-real-token".into(),
        };
        let value = config.codex_override();
        assert_eq!(value.matches('{').count(), value.matches('}').count());
        assert!(value.contains("current_database={approval_mode=\"approve\"}"));
        assert!(value.contains("query_sql={approval_mode=\"approve\"}"));
        assert!(value.contains("required=true"));
        assert!(!value.contains(&config.token));
    }
    #[test]
    fn results_are_utf8_bounded_and_sensitive_columns_are_masked() {
        use crate::database::{notebook_query::ResultPage, workbench::Rows};
        let page = ResultPage {
            execution: None,
            affected_rows: None,
            grid: None,
            rows: Rows {
                columns: std::iter::once("password".into())
                    .chain((1..64).map(|i| format!("value_{i}")))
                    .collect(),
                values: vec![vec![Some("界".repeat(600)); 64]; 20],
            },
            more: false,
            offset: 0,
            elapsed_ms: 1,
            read_only: true,
        };
        let result = result_data(page);
        assert!(result["rows"][0][0] == "***");
        assert_eq!(result["truncated"], true);
        assert!(result.to_string().len() < 131072);
        assert_eq!(result["rows"].as_array().unwrap().len(), 20);
        let plan = ResultPage {
            execution: None,
            affected_rows: None,
            grid: None,
            rows: Rows {
                columns: vec!["EXPLAIN".into()],
                values: vec![vec![Some("x".repeat(8000))]],
            },
            more: false,
            offset: 0,
            elapsed_ms: 1,
            read_only: true,
        };
        assert_eq!(
            result_data(plan)["rows"][0][0].as_str().unwrap().len(),
            8000
        );
    }
    #[test]
    fn authenticated_transport_rejects_browser_and_invalid_tools() {
        let (server, calls) = Server::start().unwrap();
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let client = reqwest::Client::new();
            let initialize = json!({"jsonrpc":"2.0","id":1,"method":"initialize"}).to_string();
            let request = || client.post(&server.config.url).header("Content-Type", "application/json").body(initialize.clone());
            assert_eq!(request().send().await.unwrap().status(), StatusCode::FORBIDDEN);
            assert_eq!(request().bearer_auth(&server.config.token).header("Origin", "https://example.invalid").send().await.unwrap().status(), StatusCode::FORBIDDEN);
            let response = request().bearer_auth(&server.config.token).send().await.unwrap();
            assert!(response.status().is_success());
            let body: Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
            assert_eq!(body["result"]["serverInfo"]["name"], "TurboDBNote");
            let invalid = json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"query_sql","arguments":{"sql":"SELECT 1","connection":"other"}}});
            let response = client.post(&server.config.url).header("Content-Type", "application/json").bearer_auth(&server.config.token).body(invalid.to_string()).send().await.unwrap();
            let body: Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
            assert_eq!(body["result"]["isError"], true);
            assert!(calls.try_recv().is_err());
            let call = json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"query_sql","arguments":{"sql":"SELECT 1"}}});
            let request = client.post(&server.config.url).header("Content-Type", "application/json").bearer_auth(&server.config.token).body(call.to_string()).send();
            let reply = async { let call = calls.recv().await.unwrap(); assert_eq!(call.sql.as_deref(), Some("SELECT 1")); call.response.send(Ok(json!({"rows":[[1]]}))).await.unwrap(); };
            let (response, _) = tokio::join!(request, reply);
            assert!(response.unwrap().text().await.unwrap().contains("rows"));
        });
    }
}

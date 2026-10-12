mod adapter;
mod document;
mod protocol;
mod relational;
use protocol::*;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use zeroize::Zeroize;
async fn emit(value: Value) -> Result<()> {
    let mut bytes = serde_json::to_vec(&value).map_err(|_| FAILED)?;
    if bytes.len() > 1048576 {
        bytes = serde_json::to_vec(&json!({"Err":TOO_LARGE})).map_err(|_| FAILED)?;
    }
    bytes.push(b'\n');
    let mut out = tokio::io::stdout();
    out.write_all(&bytes).await.map_err(|_| FAILED)?;
    out.flush().await.map_err(|_| FAILED)
}
async fn run() -> Result<()> {
    let mut input = tokio::io::BufReader::new(tokio::io::stdin());
    let mut line = String::new();
    (&mut input)
        .take(65537)
        .read_line(&mut line)
        .await
        .map_err(|_| FAILED)?;
    if line.len() > 65536 {
        return Err(TOO_LARGE);
    }
    let mut request: Value = serde_json::from_str(&line).map_err(|_| FAILED)?;
    line.zeroize();
    let mut password =
        zeroize::Zeroizing::new(request["password"].as_str().unwrap_or("").to_owned());
    if let Some(Value::String(s)) = request.get_mut("password") {
        s.zeroize();
    }
    let started = std::time::Instant::now();
    let p = request["profile"].take();
    let timeout =
        std::time::Duration::from_secs(p["timeout_seconds"].as_u64().unwrap_or(10).clamp(1, 60));
    let mut adapter = tokio::time::timeout(timeout, adapter::Adapter::connect(p, &password))
        .await
        .map_err(|_| FAILED)??;
    password.zeroize();
    let databases = adapter.databases().await?;
    emit(json!({"info":{"version":adapter.version,"session_id":0,"charset":"UTF-8","database":adapter.db,"tls_cipher":if string(&adapter.p,"tls")=="Disabled" {""}else{"TLS"},"elapsed_ms":started.elapsed().as_millis() as u64,"databases":databases,"database_list_error":null}})).await?;
    loop {
        line.clear();
        if (&mut input)
            .take(1048577)
            .read_line(&mut line)
            .await
            .map_err(|_| FAILED)?
            == 0
        {
            break;
        }
        if line.len() > 1048576 {
            return Err(TOO_LARGE);
        }
        if line.trim() == "ping" {
            tokio::time::timeout(timeout, adapter.ping())
                .await
                .map_err(|_| FAILED)??;
            emit(json!({"ok":true})).await?;
            continue;
        }
        let result = match serde_json::from_str::<Value>(&line) {
            Ok(v) => match allowed(&v["kind"]) {
                Ok(()) => {
                    match tokio::time::timeout(timeout, adapter.operation(&v["kind"])).await {
                        Ok(result) => result,
                        Err(_) => {
                            emit(json!({"Err":FAILED})).await?;
                            break;
                        }
                    }
                }
                Err(e) => Err(e),
            },
            Err(_) => Err(FAILED),
        };
        emit(match result {
            Ok(v) => json!({"Ok":v}),
            Err(e) => json!({"Err":e}),
        })
        .await?;
    }
    Ok(())
}
fn main() {
    std::panic::set_hook(Box::new(|_| {}));
    let _ = rustls::crypto::ring::default_provider().install_default();
    if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        rt.block_on(async {
            if let Err(e) = run().await {
                let _ = emit(json!({"error":e})).await;
            }
        });
    }
}

//! Explicit live Codex/MCP probe, with an in-memory SQLite database and no user DB access.
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use turbodbn_services::{
    DatabaseKind, agent,
    codex::{ClientCommand, Event, Prompt},
    database::notebook_query,
    sql_mcp,
};

fn main() {
    let path = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("sql_mcp_probe <codex.exe> --live"),
    );
    assert!(
        std::env::args().any(|a| a == "--live"),
        "--live explicitly enables one model request"
    );
    let directory = tempfile::tempdir().unwrap();
    let (server, calls) = sql_mcp::Server::start().expect("Local MCP startup failed");
    let scope_count = Arc::new(AtomicUsize::new(0));
    let query_count = Arc::new(AtomicUsize::new(0));
    let scopes = scope_count.clone();
    let queries = query_count.clone();
    let worker = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async move {
            use sqlx::Connection;
            let mut db = sqlx::SqliteConnection::connect("sqlite::memory:").await.unwrap();
            sqlx::query("CREATE TABLE probe(id INTEGER PRIMARY KEY, name TEXT)").execute(&mut db).await.unwrap();
            while let Ok(call) = calls.recv().await {
                let result = if let Some(sql) = call.sql {
                    if sql.trim() != "EXPLAIN QUERY PLAN SELECT * FROM probe WHERE id = 1" {
                        Err("Probe allows only the specified isolated EXPLAIN".into())
                    } else {
                        match notebook_query::explain(&mut db, DatabaseKind::Sqlite, &sql).await {
                            Ok(page) => { queries.fetch_add(1, Ordering::SeqCst); Ok(sql_mcp::result_data(page)) },
                            Err(_) => Err("Isolated EXPLAIN failed".into()),
                        }
                    }
                } else {
                    scopes.fetch_add(1, Ordering::SeqCst);
                    Ok(serde_json::json!({"connection_ref":"isolated-probe","database":"main","kind":"SQLite"}))
                };
                let _ = call.response.send(result).await;
            }
        });
    });
    let (connection, events) = agent::connect_with_mcp(
        agent::Engine::Codex,
        path,
        directory.path().to_owned(),
        Some(server.config.clone()),
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(180), async {
            let mut sent = false;
            let mut ready = false;
            let mut selected = None;
            while let Ok(event) = events.recv().await {
                match event {
                    Event::Account { ready: value, .. } => { ready = value; println!("CLI account ready: {ready}"); }
                    Event::Models(models) if !sent => {
                        selected = models.iter().find(|m| m.is_default).or(models.first()).cloned();
                    }
                    Event::Completed(_) => break,
                    Event::Error(_) => panic!("Codex probe failed; external details suppressed"),
                    Event::Disconnected => panic!("Codex disconnected"),
                    _ => {}
                }
                if ready && !sent && let Some(model) = &selected {
                        connection.send(ClientCommand::Send(Prompt {
                            text: "Integration test in an isolated in-memory SQLite database. Call turbodbnote current_database, then query_sql with exactly: EXPLAIN QUERY PLAN SELECT * FROM probe WHERE id = 1 . The SQL argument must not include that final punctuation period. Both calls are explicitly authorized for this test. Do not read files, use shell, or query anything else. Summarize the actual plan returned.".into(),
                            model: model.model.clone(), effort: model.supported_reasoning_efforts.iter().find(|e| e.reasoning_effort == "low").map(|e| e.reasoning_effort.clone()),
                        })).unwrap(); sent = true;
                }
            }
        }).await.expect("Live MCP probe timed out");
    });
    drop(connection);
    drop(server);
    worker.join().unwrap();
    assert!(
        scope_count.load(Ordering::SeqCst) > 0,
        "No current_database call reached MCP"
    );
    assert!(
        query_count.load(Ordering::SeqCst) > 0,
        "No EXPLAIN call reached isolated SQLite"
    );
    println!(
        "PASS: live Codex -> authenticated MCP -> isolated SQLite EXPLAIN; no credentials or model text printed"
    );
}

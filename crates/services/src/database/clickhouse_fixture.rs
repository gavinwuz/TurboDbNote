//! Owned localhost HTTP fixture; never connects to a database server.
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
pub const DDL: &str =
    "CREATE TABLE fixture.items (id UInt64, label String) ENGINE=MergeTree ORDER BY id";
pub const NATIVE: &[u8] = b"fixture-native\0;\n";
#[derive(Clone)]
pub struct Seen {
    pub sql: String,
    pub params: std::collections::HashMap<String, String>,
    pub payload: Vec<u8>,
}
pub struct Fixture {
    pub profile: crate::mysql::Profile,
    pub seen: Arc<Mutex<Vec<Seen>>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
fn json_rows(names: &[&str], rows: Vec<Vec<Value>>) -> Vec<u8> {
    let meta = names
        .iter()
        .map(|name| json!({"name":name,"type":"String"}))
        .collect::<Vec<_>>();
    let data = rows
        .into_iter()
        .map(|row| {
            names
                .iter()
                .zip(row)
                .map(|(k, v)| (k.to_string(), v))
                .collect::<serde_json::Map<_, _>>()
        })
        .collect::<Vec<_>>();
    serde_json::to_vec(&json!({"meta":meta,"data":data})).unwrap()
}
impl Fixture {
    pub fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let seen = Arc::new(Mutex::new(vec![]));
        let records = seen.clone();
        let stopped = stop.clone();
        let thread = thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                let (mut socket, _) = match listener.accept() {
                    Ok(v) => v,
                    Err(_) => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                };
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = vec![];
                let mut byte = [0];
                while !request.ends_with(b"\r\n\r\n") {
                    if socket.read_exact(&mut byte).is_err() {
                        break;
                    }
                    request.push(byte[0]);
                    if request.len() > 65536 {
                        break;
                    }
                }
                let headers = String::from_utf8_lossy(&request);
                let path = headers
                    .lines()
                    .next()
                    .and_then(|l| l.split_whitespace().nth(1))
                    .unwrap_or("/");
                let params = reqwest::Url::parse(&format!("http://fixture{path}"))
                    .unwrap()
                    .query_pairs()
                    .map(|(k, v)| (k.into_owned(), v.into_owned()))
                    .collect::<std::collections::HashMap<_, _>>();
                let len = headers
                    .lines()
                    .filter_map(|l| l.split_once(':'))
                    .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                    .and_then(|(_, v)| v.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                let mut body = vec![0; len];
                if socket.read_exact(&mut body).is_err() {
                    continue;
                }
                let (sql, payload) = if let Some(sql) = params.get("query") {
                    (sql.clone(), body)
                } else {
                    (String::from_utf8(body).unwrap(), vec![])
                };
                let (status, extra, response) = if sql.contains("fixture_denied") {
                    (403, "X-ClickHouse-Exception-Code: 497\r\n", vec![])
                } else if sql.contains("fixture_late_error") {
                    (
                        200,
                        "X-ClickHouse-Exception-Code: 62\r\n",
                        b"redacted".to_vec(),
                    )
                } else {
                    let bytes = if sql.starts_with("SELECT version()") {
                        b"{\"version\":\"24.8-fixture\"}\n".to_vec()
                    } else if sql.starts_with("SELECT name, engine, create_table_query") {
                        json_rows(
                            &["name", "engine", "create_table_query"],
                            vec![vec![json!("items"), json!("MergeTree"), json!(DDL)]],
                        )
                    } else if sql.starts_with("SELECT create_table_query") {
                        json_rows(&["create_table_query"], vec![vec![json!(DDL)]])
                    } else if sql.starts_with("SELECT name FROM system.columns") {
                        json_rows(&["name"], vec![vec![json!("id")], vec![json!("label")]])
                    } else if sql.starts_with("SELECT name FROM system.databases")
                        || sql.starts_with("SELECT name, engine FROM system.databases")
                    {
                        json_rows(&["name"], vec![])
                    } else if sql.contains("FORMAT Native") && sql.starts_with("SELECT") {
                        NATIVE.to_vec()
                    } else if sql.contains("FORMAT SQLInsert") {
                        b"INSERT INTO `items` (`id`,`label`) VALUES (1,'a;\\\'b');\n".to_vec()
                    } else if sql.starts_with("SELECT") && !sql.contains("FORMAT Null") {
                        json_rows(
                            &["id", "label"],
                            (0..21).map(|i| vec![json!(i), json!("fixture")]).collect(),
                        )
                    } else {
                        vec![]
                    };
                    (200, "", bytes)
                };
                records.lock().unwrap().push(Seen {
                    sql,
                    params,
                    payload,
                });
                let _ = write!(
                    socket,
                    "HTTP/1.1 {status} fixture\r\nConnection: close\r\n{extra}Content-Length: {}\r\n\r\n",
                    response.len()
                );
                let _ = socket.write_all(&response);
            }
        });
        let mut profile = crate::driver_catalog::find("clickhouse").unwrap().profile();
        profile.host = "127.0.0.1".into();
        profile.port = port;
        profile.database = "fixture".into();
        profile.list_all_databases = false;
        profile.tls = crate::mysql::TlsMode::Disabled;
        Self {
            profile,
            seen,
            stop,
            thread: Some(thread),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            t.join().unwrap();
        }
    }
}

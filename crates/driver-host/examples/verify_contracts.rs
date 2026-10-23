//! Native, isolated HTTP-contract and process-lifecycle verification.
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime},
};
type Check<T> = Result<T, String>;
const TIMEOUT: Duration = Duration::from_secs(15);
const FRAME: u64 = 1024 * 1024;
fn require(condition: bool, message: &str) -> Check<()> {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}
struct Server {
    port: u16,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<Check<()>>>,
}
impl Server {
    fn start() -> Check<Self> {
        let listener =
            TcpListener::bind(("127.0.0.1", 0)).map_err(|_| "Cannot bind isolated fixture")?;
        let port = listener
            .local_addr()
            .map_err(|_| "Fixture address unavailable")?
            .port();
        listener
            .set_nonblocking(true)
            .map_err(|_| "Cannot configure fixture")?;
        let stop = Arc::new(AtomicBool::new(false));
        let halted = stop.clone();
        let thread = thread::spawn(move || {
            while !halted.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => respond(stream)?,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(_) => return Err("Fixture accept failed".into()),
                }
            }
            Ok(())
        });
        Ok(Self {
            port,
            stop,
            thread: Some(thread),
        })
    }
    fn finish(&mut self) -> Check<()> {
        self.stop.store(true, Ordering::Release);
        self.thread
            .take()
            .ok_or("Fixture already stopped")?
            .join()
            .map_err(|_| "Fixture thread failed")?
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take()
            && let Ok(Err(error)) = thread.join()
        {
            eprintln!("Fixture server failed: {error}");
        }
    }
}
fn respond(mut stream: TcpStream) -> Check<()> {
    // Accepted sockets inherit listener nonblocking mode on Windows.
    stream
        .set_nonblocking(false)
        .map_err(|_| "Fixture blocking setup failed")?;
    stream
        .set_read_timeout(Some(TIMEOUT))
        .map_err(|_| "Fixture timeout setup failed")?;
    stream
        .set_write_timeout(Some(TIMEOUT))
        .map_err(|_| "Fixture timeout setup failed")?;
    let mut reader = BufReader::new(&mut stream);
    let mut request = String::new();
    if reader
        .read_line(&mut request)
        .map_err(|_| "Fixture request failed")?
        == 0
    {
        return Ok(());
    }
    let path = request
        .split_whitespace()
        .nth(1)
        .ok_or("Invalid fixture request")?
        .to_owned();
    let mut length = 0;
    let mut header_size = request.len();
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|_| "Fixture header failed")?;
        header_size += line.len();
        require(header_size < 16384, "Fixture header limit exceeded")?;
        if line == "\r\n" || line.is_empty() {
            break;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = value
                .trim()
                .parse::<usize>()
                .map_err(|_| "Invalid fixture content length")?;
        }
    }
    require(length <= 65536, "Fixture request limit exceeded")?;
    let mut data = vec![0; length];
    reader
        .read_exact(&mut data)
        .map_err(|_| "Fixture body failed")?;
    drop(reader);
    let response = if request.starts_with("GET ") {
        if path.ends_with("/_count") {
            json!({"count":3})
        } else {
            json!({"version":{"number":"fixture"}})
        }
    } else if path.ends_with("/_search") {
        let search: Value =
            serde_json::from_slice(&data).map_err(|_| "Invalid search fixture request")?;
        require(
            search["size"] == 3 && search["track_total_hits"] == false,
            "Search request was not bounded",
        )?;
        let content = if path.starts_with("/oversized/") {
            "x".repeat(65537)
        } else {
            "fixture".into()
        };
        json!({"hits":{"hits":(0..3).map(|i| json!({"_id":i.to_string(),"_source":{"name":content}})).collect::<Vec<_>>()}})
    } else {
        let sql = std::str::from_utf8(&data).map_err(|_| "Invalid fixture text")?;
        require(
            !sql.starts_with("DROP "),
            "A rejected write reached the fixture",
        )?;
        let (columns, rows) = if sql.contains("SERVER_VERSION") {
            (json!([["version", "VARCHAR", 20]]), json!([["fixture"]]))
        } else if sql.starts_with("DESCRIBE ") {
            (
                json!([
                    ["field", "VARCHAR", 20],
                    ["type", "VARCHAR", 20],
                    ["length", "INT", 4],
                    ["note", "VARCHAR", 20]
                ]),
                json!([["ts", "TIMESTAMP", 8, ""]]),
            )
        } else if sql == "SHOW TABLES" {
            (json!([["table_name", "VARCHAR", 20]]), json!([["meters"]]))
        } else {
            (json!([["ts", "TIMESTAMP", 8]]), json!([["2026-01-01"]]))
        };
        json!({"code":0,"column_meta":columns,"data":rows})
    };
    let body = serde_json::to_vec(&response).map_err(|_| "Fixture response failed")?;
    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).map_err(|_| "Fixture response failed")?;
    stream
        .write_all(&body)
        .map_err(|_| "Fixture response failed")?;
    Ok(())
}
struct Worker {
    child: Child,
    input: Option<ChildStdin>,
    replies: mpsc::Receiver<Check<Value>>,
    stderr: mpsc::Receiver<Vec<u8>>,
}
impl Worker {
    fn start(runtime: &Path) -> Check<Self> {
        let executable = runtime.join(format!(
            "turbodbnote-driver-host{}",
            std::env::consts::EXE_SUFFIX
        ));
        require(
            executable.is_file(),
            "Native worker is missing from the explicit runtime directory",
        )?;
        let mut command = Command::new(executable);
        command
            .current_dir(runtime)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command.spawn().map_err(|_| "Cannot launch native worker")?;
        let input = child.stdin.take();
        let stdout = child.stdout.take().ok_or("Worker stdout unavailable")?;
        let error = child.stderr.take().ok_or("Worker stderr unavailable")?;
        let (sender, replies) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = vec![];
                match (&mut reader).take(FRAME + 1).read_until(b'\n', &mut line) {
                    Ok(0) => break,
                    Ok(count) if count <= FRAME as usize => {
                        let result = serde_json::from_slice(&line)
                            .map_err(|_| "Invalid worker JSON frame".into());
                        if sender.send(result).is_err() {
                            break;
                        }
                    }
                    _ => {
                        let _ = sender.send(Err("Worker output limit exceeded".into()));
                        break;
                    }
                }
            }
        });
        let (sender, stderr) = mpsc::channel();
        thread::spawn(move || {
            let mut bytes = vec![];
            let _ = error.take(FRAME + 1).read_to_end(&mut bytes);
            let _ = sender.send(bytes);
        });
        Ok(Self {
            child,
            input,
            replies,
            stderr,
        })
    }
    fn send_bytes(&mut self, bytes: &[u8]) -> Check<()> {
        let input = self.input.as_mut().ok_or("Worker input closed")?;
        input
            .write_all(bytes)
            .and_then(|_| input.write_all(b"\n"))
            .and_then(|_| input.flush())
            .map_err(|_| "Worker input failed".into())
    }
    fn request(&mut self, value: &Value) -> Check<Value> {
        self.send_bytes(&serde_json::to_vec(value).map_err(|_| "Request serialization failed")?)?;
        self.replies
            .recv_timeout(TIMEOUT)
            .map_err(|_| "Worker reply timed out or closed")?
    }
    fn operation(&mut self, action: Value) -> Check<Value> {
        self.request(&json!({"id":1,"kind":{"Workbench":{"database":"fixture","action":action}}}))
    }
    fn close(&mut self) -> Check<()> {
        self.input.take();
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if self
                .child
                .try_wait()
                .map_err(|_| "Cannot inspect owned worker")?
                .is_some()
            {
                return Ok(());
            }
            require(
                Instant::now() < deadline,
                "Worker did not exit on stdin close",
            )?;
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn quiet(&self) -> Check<()> {
        let bytes = self
            .stderr
            .recv_timeout(TIMEOUT)
            .map_err(|_| "Worker stderr did not close")?;
        require(
            bytes.is_empty(),
            "Worker emitted unexpected diagnostic data",
        )
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn run() -> Check<()> {
    let runtime = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("Usage: verify_contracts <runtime-directory>")?,
    )
    .canonicalize()
    .map_err(|_| "Runtime directory unavailable")?;
    let mut server = Server::start()?;
    let mut delayed = TcpStream::connect(("127.0.0.1", server.port))
        .map_err(|_| "Cannot connect delayed fixture request")?;
    delayed
        .set_read_timeout(Some(TIMEOUT))
        .map_err(|_| "Cannot bound delayed fixture request")?;
    thread::sleep(Duration::from_millis(25));
    delayed
        .write_all(b"GET / HTTP/1.1\r\nHost: fixture\r\nConnection: close\r\n\r\n")
        .map_err(|_| "Cannot send delayed fixture request")?;
    let mut response = vec![];
    delayed
        .take(4096)
        .read_to_end(&mut response)
        .map_err(|_| "Delayed fixture response failed")?;
    require(
        response.starts_with(b"HTTP/1.1 200 OK\r\n"),
        "Delayed fixture request was not served",
    )?;
    for driver in ["elasticsearch", "tdengine"] {
        let mut worker = Worker::start(&runtime)?;
        let profile = json!({"driver_id":driver,"kind":"External","host":"127.0.0.1","port":server.port,"user":"","database":"fixture","timeout_seconds":10,"tls":"Disabled","list_all_databases":false});
        let reply = worker.request(&json!({"profile":profile,"password":""}))?;
        require(
            reply.get("info").is_some(),
            "Worker fixture handshake failed",
        )?;
        let catalog = worker.operation(json!("Catalog"))?;
        let object = catalog["Ok"]["Workbench"]["Catalog"]["objects"]
            .as_array()
            .and_then(|objects| objects.first())
            .ok_or("Fixture catalog missing")?
            .clone();
        let page = worker.operation(
            json!({"GridPage":{"object":object,"options":{"page_size":2,"offset":0}}}),
        )?;
        require(
            page["Ok"]["Workbench"]["GridPage"]["rows"]["values"]
                .as_array()
                .is_some_and(|rows| !rows.is_empty() && rows.len() <= 2),
            "Grid page was empty or unbounded",
        )?;
        require(
            worker
                .operation(json!({"Execute":{"sql":"DROP TABLE fixture","guard":null}}))?
                .get("Err")
                .is_some(),
            "Worker accepted a forbidden write",
        )?;
        if driver == "elasticsearch" {
            let mut large = object.clone();
            large["name"] = json!("oversized");
            require(
                worker.operation(
                    json!({"GridPage":{"object":large,"options":{"page_size":2,"offset":0}}}),
                )? == json!({"Err":"drivers.value_limit"}),
                "Oversized value was not rejected",
            )?;
            let count = worker.operation(json!({"Count":{"object":object,"filter":null}}))?;
            require(
                count["Ok"]["Workbench"]["Count"] == 3,
                "Fixture row count mismatch",
            )?;
        }
        worker.close()?;
        worker.quiet()?;
        println!("{driver}: bounded reads, rejected writes and clean exit: PASS");
    }
    server.finish()?;
    let sentinel = format!(
        "fixture-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_err(|_| "Clock unavailable")?
            .as_nanos()
    );
    for payload in [
        vec![b'x'; 65538],
        serde_json::to_vec(&json!({"profile":{"driver_id":"unknown"},"password":sentinel}))
            .map_err(|_| "Fixture serialization failed")?,
    ] {
        let mut worker = Worker::start(&runtime)?;
        worker.send_bytes(&payload)?;
        let reply = worker
            .replies
            .recv_timeout(TIMEOUT)
            .map_err(|_| "Invalid handshake did not return")??;
        require(
            matches!(
                reply["error"].as_str(),
                Some("drivers.invalid_identity" | "drivers.value_limit")
            ),
            "Invalid handshake category was not preserved",
        )?;
        let serialized = serde_json::to_vec(&reply).map_err(|_| "Reply serialization failed")?;
        require(
            !serialized
                .windows(sentinel.len())
                .any(|part| part == sentinel.as_bytes()),
            "Sensitive fixture input was echoed",
        )?;
        worker.close()?;
        worker.quiet()?;
    }
    println!("Malformed/oversized inputs rejected; diagnostic values not echoed: PASS");
    Ok(())
}
fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("Native contract verification failed: {message}");
            std::process::ExitCode::FAILURE
        }
    }
}

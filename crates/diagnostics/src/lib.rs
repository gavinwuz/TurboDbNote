//! Optional native crash capture and a bounded queue of allowlisted usage events.
pub mod config;
mod native;
mod queue;

use config::Config;
use native::NativeSdk;
use queue::{EventRecord, Queue};
use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy)]
pub enum Event {
    AppStarted,
    AppStopped,
    SettingsOpened,
    ProjectsOpened,
    ConnectionsOpened,
    ChatSent,
    AgentCodex,
    AgentClaude,
    UpdateChecked,
}
impl Event {
    fn name(self) -> &'static str {
        match self {
            Self::AppStarted => "app_started",
            Self::AppStopped => "app_stopped",
            Self::SettingsOpened => "settings_opened",
            Self::ProjectsOpened => "projects_opened",
            Self::ConnectionsOpened => "connections_opened",
            Self::ChatSent => "chat_sent",
            Self::AgentCodex => "agent_codex",
            Self::AgentClaude => "agent_claude",
            Self::UpdateChecked => "update_checked",
        }
    }
}
enum Command {
    Event(Event),
    Fault(Fault),
    Stop(mpsc::Sender<()>),
}
#[derive(Clone, Copy)]
pub enum Fault {
    AgentTransport,
    SettingsPersistence,
}
impl Fault {
    fn name(self) -> &'static str {
        match self {
            Self::AgentTransport => "AgentTransportError",
            Self::SettingsPersistence => "SettingsPersistenceError",
        }
    }
}
static SENDER: OnceLock<SyncSender<Command>> = OnceLock::new();
static STATUS: OnceLock<&'static str> = OnceLock::new();
static STOPPED: AtomicBool = AtomicBool::new(false);
pub fn status() -> &'static str {
    STATUS.get().copied().unwrap_or("Diagnostics disabled")
}
pub fn track(event: Event) {
    if let Some(sender) = SENDER.get() {
        let _ = sender.try_send(Command::Event(event));
    }
}
/// Reports a fixed error category. Never accepts raw SQL, exception text or paths.
pub fn report_fault(fault: Fault) {
    if let Some(sender) = SENDER.get() {
        let _ = sender.try_send(Command::Fault(fault));
    }
}
pub struct Guard;
impl Drop for Guard {
    fn drop(&mut self) {
        shutdown();
    }
}
pub fn shutdown() {
    if STOPPED.swap(true, Ordering::SeqCst) {
        return;
    }
    if let Some(sender) = SENDER.get() {
        let (done, wait) = mpsc::channel();
        if sender.try_send(Command::Stop(done)).is_ok() {
            let _ = wait.recv_timeout(Duration::from_secs(4));
        }
    }
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
fn record(name: &str, session: &str, version: &str, started: Instant) -> EventRecord {
    EventRecord {
        id: uuid::Uuid::new_v4().to_string(),
        session: session.into(),
        name: name.into(),
        version: version.into(),
        timestamp_ms: now(),
        elapsed_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
    }
}

/// Call once on the application/main thread after platform initialization.
pub fn start(config: Config, exe_dir: &Path, data_dir: &Path, version: &str) -> Guard {
    if SENDER.get().is_some() {
        return Guard;
    }
    if !config.enabled {
        let _ = STATUS.set("Diagnostics disabled");
        return Guard;
    }
    if std::fs::create_dir_all(data_dir).is_err() {
        let _ = STATUS.set("Unable to create diagnostics directory");
        return Guard;
    }
    // Choose a region only before native initialization; never initialize two SDKs
    // or reinterpret a void ReportException return as upload success/failure.
    let mut active = config.clone();
    let fallback_marker = data_dir.join("use-fallback.json");
    if let Ok(bytes) = std::fs::read(&fallback_marker)
        && bytes.len() < 1024
        && let Ok(marker) = serde_json::from_slice::<FallbackMarker>(&bytes)
        && marker.primary_app_id == config.app_id
        && let Some(fallback) = &config.fallback
    {
        active.app_id = fallback.app_id.clone();
        active.region = fallback.region;
    }
    let sdk = NativeSdk::load(
        exe_dir,
        &data_dir.join(format!("native-{}", active.app_id)),
        &active,
        version,
    );
    let native = match sdk {
        Ok(sdk) => {
            let _ = STATUS.set("CrashSight initialized (delivery not verified)");
            Some(sdk)
        }
        Err(error) => {
            let _ = STATUS.set(error);
            None
        }
    };
    let session = uuid::Uuid::new_v4().to_string();
    if let Some(sdk) = native {
        sdk.set_value("app.version", version);
        sdk.set_value("app.platform", std::env::consts::OS);
        sdk.set_value("session.id", &session);
    }
    prune_panic_records(data_dir);
    let panic_file = data_dir.join(format!("panic-{session}.json"));
    install_panic_hook(panic_file, version.to_owned());
    let (sender, receiver) = mpsc::sync_channel(128);
    if SENDER.set(sender).is_err() {
        return Guard;
    }
    let data_dir = data_dir.to_owned();
    let version = version.to_owned();
    let _ = thread::Builder::new()
        .name("diagnostics".into())
        .spawn(move || {
            let started = Instant::now();
            let mut queue = Queue::open(&data_dir.join("events.db")).ok();
            if config.record_usage
                && let Some(queue) = queue.as_mut()
            {
                let _ = queue.push(&record(
                    Event::AppStarted.name(),
                    &session,
                    &version,
                    started,
                ));
            }
            replay_panics(&data_dir, native);
            let client = reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(3))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .ok();
            let mut next_upload = Instant::now() + Duration::from_secs(30);
            let mut faults_reported = 0;
            loop {
                match receiver.recv_timeout(Duration::from_millis(500)) {
                    Ok(Command::Event(event)) => {
                        if let Some(sdk) = native {
                            sdk.set_value("app.last_action", event.name());
                        }
                        if config.record_usage
                            && let Some(queue) = queue.as_mut()
                        {
                            let _ = queue.push(&record(event.name(), &session, &version, started));
                        }
                    }
                    Ok(Command::Stop(done)) => {
                        if config.record_usage
                            && let Some(queue) = queue.as_mut()
                        {
                            let _ = queue.push(&record(
                                Event::AppStopped.name(),
                                &session,
                                &version,
                                started,
                            ));
                        }
                        let _ = done.send(());
                        break;
                    }
                    Ok(Command::Fault(fault)) => {
                        // Bound repeated transient service failures to avoid flooding
                        // the exception project. Raw backend errors stay local.
                        if faults_reported < 10
                            && let Some(sdk) = native
                        {
                            let extras =
                                serde_json::json!({"session.id":session,"app.version":version})
                                    .to_string();
                            sdk.report(
                                fault.name(),
                                "Application operation failed; details remain local",
                                "",
                                &extras,
                            );
                            faults_reported += 1;
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                if Instant::now() >= next_upload {
                    if config.record_usage
                        && let (Some(queue), Some(client), Some(url)) = (
                            queue.as_mut(),
                            client.as_ref(),
                            config.analytics_endpoint.as_deref(),
                        )
                    {
                        let _ = upload(queue, client, url, config.analytics_token.as_deref());
                    }
                    next_upload = Instant::now() + Duration::from_secs(30);
                }
            }
        });
    Guard
}

#[derive(serde::Serialize, serde::Deserialize)]
struct FallbackMarker {
    primary_app_id: String,
}
/// Use only with a confirmed upload failure signal from the selected SDK version
/// or a trusted delivery monitor. Applies at next launch, never mid-process.
pub fn mark_confirmed_primary_failure(
    config: &Config,
    data_dir: &Path,
) -> Result<(), &'static str> {
    if config.fallback.is_none() {
        return Err("No fallback project configured");
    }
    std::fs::create_dir_all(data_dir).map_err(|_| "Cannot create failover state")?;
    let bytes = serde_json::to_vec(&FallbackMarker {
        primary_app_id: config.app_id.clone(),
    })
    .map_err(|_| "Cannot encode failover state")?;
    std::fs::write(data_dir.join("use-fallback.json"), bytes)
        .map_err(|_| "Cannot persist failover state")
}
fn upload(
    queue: &mut Queue,
    client: &reqwest::blocking::Client,
    url: &str,
    token: Option<&str>,
) -> Result<(), String> {
    let batch = queue.batch()?;
    if batch.is_empty() {
        return Ok(());
    }
    let body = serde_json::json!({"schema":1,"events":batch.iter().map(|(_,event)|event).collect::<Vec<_>>()});
    let mut request = client.post(url).json(&body);
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let response = request.send().map_err(|_| "Analytics transport failed")?;
    if !response.status().is_success() {
        return Err("Analytics receiver rejected batch".into());
    }
    // Contract: any 2xx means the entire batch was durably accepted. Receiver must
    // deduplicate event IDs, because a lost HTTP acknowledgement causes retry.
    queue.acknowledge(&batch.iter().map(|(seq, _)| *seq).collect::<Vec<_>>())
}

fn prune_panic_records(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut records = entries
        .flatten()
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with("panic-") && (name.ends_with(".json") || name.ends_with(".submitted"))
        })
        .collect::<Vec<_>>();
    records.sort_by_key(|entry| entry.metadata().and_then(|m| m.modified()).ok());
    let count = records.len().saturating_sub(31);
    for entry in records.into_iter().take(count) {
        let _ = std::fs::remove_file(entry.path());
    }
}

fn install_panic_hook(path: PathBuf, version: String) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Do not inspect payload: it may contain SQL, prompts, paths or credentials.
        // No SDK, network, backtrace formatting, or application locks here.
        if let Ok(mut file) = OpenOptions::new().write(true).create_new(true).open(&path) {
            let line = info.location().map(|l| l.line()).unwrap_or(0);
            let _ = write!(file, "{{\"version\":\"{version}\",\"line\":{line}}}");
        }
        previous(info);
    }));
}
fn replay_panics(dir: &Path, native: Option<&NativeSdk>) {
    let Some(sdk) = native else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries
        .flatten()
        .filter(|entry| {
            entry.file_name().to_string_lossy().starts_with("panic-")
                && entry.path().extension().is_some_and(|ext| ext == "json")
        })
        .take(8)
    {
        if !entry
            .metadata()
            .is_ok_and(|m| m.is_file() && m.len() <= 1024)
        {
            continue;
        }
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        let extras =
            serde_json::json!({"panic.line":value["line"].as_u64().unwrap_or(0),"panic.version":value["version"].as_str().unwrap_or("unknown").chars().take(64).collect::<String>(),"replayed":true})
                .to_string();
        if sdk.report(
            "RustPanic",
            "Rust panic observed; payload omitted",
            "",
            &extras,
        ) {
            let _ = std::fs::rename(&path, path.with_extension("submitted"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fallback_is_explicit_persistent_and_bound_to_primary() {
        let config=Config::from_json(br#"{"enabled":true,"app_id":"overseas","region":"global","fallback":{"app_id":"domestic","region":"china"}}"#).unwrap();
        let directory = tempfile::tempdir().unwrap();
        assert!(!directory.path().join("use-fallback.json").exists());
        mark_confirmed_primary_failure(&config, directory.path()).unwrap();
        let marker: FallbackMarker = serde_json::from_slice(
            &std::fs::read(directory.path().join("use-fallback.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(marker.primary_app_id, "overseas");
    }
    #[test]
    fn records_only_allowlisted_metadata() {
        let record = record(
            Event::ChatSent.name(),
            "random-session",
            "1.0.0",
            Instant::now(),
        );
        let value = serde_json::to_value(record).unwrap();
        assert_eq!(value["name"], "chat_sent");
        assert_eq!(value.as_object().unwrap().len(), 6);
    }
    #[test]
    fn http_failure_retains_batch_success_acknowledges_it() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
        };
        let dir = tempfile::tempdir().unwrap();
        let mut queue = Queue::open(&dir.path().join("queue.db")).unwrap();
        queue
            .push(&record("app_started", "session", "1", Instant::now()))
            .unwrap();
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(2))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        for code in [503, 204] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut header = vec![];
                let mut byte = [0];
                while !header.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).unwrap();
                    header.push(byte[0]);
                }
                let header = String::from_utf8(header).unwrap();
                let length: usize = header
                    .lines()
                    .find_map(|line| {
                        line.to_lowercase()
                            .strip_prefix("content-length: ")
                            .map(str::to_owned)
                    })
                    .unwrap()
                    .parse()
                    .unwrap();
                let mut body = vec![0; length];
                stream.read_exact(&mut body).unwrap();
                let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(json["events"][0]["name"], "app_started");
                write!(
                    stream,
                    "HTTP/1.1 {code} Test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .unwrap();
            });
            let result = upload(
                &mut queue,
                &client,
                &format!("http://{address}/events"),
                None,
            );
            server.join().unwrap();
            assert_eq!(result.is_ok(), code == 204);
            assert_eq!(queue.batch().unwrap().is_empty(), code == 204);
        }
    }
}

//! Only structured operation facts enter persistent logs; never SQL, credentials or raw errors.
use std::{
    io::Write,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
};
use uuid::Uuid;

#[derive(Clone, Copy)]
pub enum Kind {
    Note,
    Connection,
    Query,
    Create,
    Export,
    Import,
    Copy,
}
impl Kind {
    pub fn key(self) -> &'static str {
        match self {
            Self::Note => "output.operation.note",
            Self::Connection => "output.operation.connection",
            Self::Query => "output.operation.query",
            Self::Create => "output.operation.create",
            Self::Export => "output.operation.export",
            Self::Import => "output.operation.import",
            Self::Copy => "output.operation.copy",
        }
    }
    fn code(self) -> &'static str {
        match self {
            Self::Note => "note",
            Self::Connection => "connection",
            Self::Query => "query",
            Self::Create => "create",
            Self::Export => "export",
            Self::Import => "import",
            Self::Copy => "copy",
        }
    }
}
#[derive(Clone, Copy)]
pub enum Phase {
    Started,
    Preview,
    Progress,
    Succeeded,
    Failed,
    Cancelled,
    Disconnected,
}
impl Phase {
    pub fn key(self) -> &'static str {
        match self {
            Self::Started => "output.operation.started",
            Self::Preview => "output.operation.preview",
            Self::Progress => "output.operation.progress",
            Self::Succeeded => "output.operation.succeeded",
            Self::Failed => "output.operation.failed",
            Self::Cancelled => "output.operation.cancelled",
            Self::Disconnected => "output.operation.disconnected",
        }
    }
    fn code(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Preview => "preview",
            Self::Progress => "progress",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Disconnected => "disconnected",
        }
    }
}
#[derive(Clone)]
pub struct Record {
    pub task: Uuid,
    pub source: Option<Uuid>,
    pub target: Option<Uuid>,
    pub kind: Kind,
    pub phase: Phase,
    pub elapsed_ms: u64,
    pub rows: u64,
    pub bytes: u64,
    pub failure: &'static str,
}
impl Record {
    pub fn new(task: Uuid, kind: Kind, phase: Phase) -> Self {
        Self {
            task,
            source: None,
            target: None,
            kind,
            phase,
            elapsed_ms: 0,
            rows: 0,
            bytes: 0,
            failure: "-",
        }
    }
    pub fn line(&self) -> String {
        format!(
            "{} task={} source={} target={} {} {} elapsed_ms={} rows={} bytes={} class={}\n",
            chrono::Utc::now().to_rfc3339(),
            self.task,
            self.source
                .map(|id| id.to_string())
                .unwrap_or_else(|| "-".into()),
            self.target
                .map(|id| id.to_string())
                .unwrap_or_else(|| "-".into()),
            self.kind.code(),
            self.phase.code(),
            self.elapsed_ms,
            self.rows,
            self.bytes,
            self.failure
        )
    }
}
pub fn failure_class(error: &str) -> &'static str {
    let s = error.to_lowercase();
    if s.contains("取消") {
        "cancelled"
    } else if s.contains("1045") || s.contains("凭据") || s.contains("认证") {
        "authentication"
    } else if s.contains("tls") || s.contains("证书") {
        "tls"
    } else if s.contains("权限") {
        "permission"
    } else if s.contains("网络") {
        "network"
    } else if s.contains("参数不兼容") || s.contains("未找到") || s.contains("驱动") {
        "driver"
    } else if s.contains("已存在") {
        "exists"
    } else if s.contains("超时") {
        "timeout"
    } else if s.contains("约束") || s.contains("主键") || s.contains("结构") {
        "schema"
    } else if s.contains("文件") || s.contains("磁盘") {
        "file"
    } else {
        "operation"
    }
}
pub struct Journal {
    pub directory: PathBuf,
    sender: mpsc::SyncSender<String>,
    pub failed: Arc<AtomicBool>,
    dropped: Arc<AtomicUsize>,
}
impl Journal {
    pub fn new(directory: PathBuf) -> Self {
        let log_directory = directory.clone();
        let (sender, receiver) = mpsc::sync_channel::<String>(256);
        let failed = Arc::new(AtomicBool::new(false));
        let state = failed.clone();
        let dropped = Arc::new(AtomicUsize::new(0));
        let lost = dropped.clone();
        std::thread::spawn(move || {
            let id = Uuid::new_v4();
            let mut part = 0;
            let mut bytes = 0;
            let mut file = None;
            for line in receiver {
                let result = (|| -> std::io::Result<()> {
                    if file.is_none() || bytes >= 10 * 1024 * 1024 {
                        std::fs::create_dir_all(&directory)?;
                        part += 1;
                        file = Some(
                            std::fs::OpenOptions::new()
                                .write(true)
                                .create_new(true)
                                .open(directory.join(format!("operations-{id}-{part}.log")))?,
                        );
                        bytes = 0;
                    }
                    let file = file.as_mut().unwrap();
                    let skipped = lost.swap(0, Ordering::Relaxed);
                    if skipped > 0 {
                        writeln!(file, "dropped_events={skipped}")?;
                    }
                    file.write_all(line.as_bytes())?;
                    file.flush()?;
                    bytes += line.len();
                    Ok(())
                })();
                if result.is_err() {
                    state.store(true, Ordering::Relaxed);
                    file = None;
                }
            }
        });
        Self {
            directory: log_directory,
            sender,
            failed,
            dropped,
        }
    }
    pub fn write(&self, record: &Record) {
        if self.sender.try_send(record.line()).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn structured_events_reach_disk() {
        let root = tempfile::tempdir().unwrap();
        let journal = Journal::new(root.path().to_path_buf());
        let id = Uuid::new_v4();
        journal.write(&Record::new(id, Kind::Export, Phase::Started));
        journal.write(&Record::new(id, Kind::Export, Phase::Succeeded));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let text = std::fs::read_dir(root.path())
                .unwrap()
                .flatten()
                .filter_map(|f| std::fs::read_to_string(f.path()).ok())
                .collect::<String>();
            if text.contains("export succeeded") {
                assert!(text.lines().count() == 2 && text.contains(&id.to_string()));
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "journal did not flush"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    #[test]
    fn raw_error_details_never_enter_journal() {
        let mut record = Record::new(Uuid::new_v4(), Kind::Connection, Phase::Failed);
        record.failure = failure_class("ERROR 1045 authentication: hidden-detail");
        let text = record.line();
        assert!(
            text.contains("class=authentication")
                && !text.contains("hidden-detail")
                && !text.contains("1045")
        );
    }
}

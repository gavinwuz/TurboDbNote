//! Opt-in object lifetime counters. No document, SQL, path or credential payloads.
use std::sync::{
    OnceLock,
    atomic::{AtomicU64, Ordering},
    mpsc::{SyncSender, sync_channel},
};
#[derive(Clone, Copy)]
pub enum Kind {
    NoteView,
    NotePane,
    CellView,
    ScriptView,
    ScriptResults,
    TableView,
}
static CREATED: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];
static DROPPED: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];
struct Monitor {
    sender: SyncSender<()>,
    lost: AtomicU64,
}
fn monitor() -> Option<&'static Monitor> {
    static INSTANCE: OnceLock<Option<Monitor>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            let directory = std::env::current_exe().ok()?.parent()?.to_owned();
            if !directory.join("memory-observation.flag").is_file() {
                return None;
            }
            let (sender, receiver) = sync_channel(1);
            std::thread::Builder::new()
                .name("lifetime-counters".into())
                .spawn(move || {
                    use std::io::Write;
                    let path =
                        directory.join(format!("memory-lifetimes-{}.csv", std::process::id()));
                    let Ok(mut file) = std::fs::OpenOptions::new()
                        .create_new(true)
                        .write(true)
                        .open(path)
                    else {
                        eprintln!("Cannot create lifetime diagnostic log");
                        return;
                    };
                    let started = std::time::Instant::now();
                    let names = [
                        "NoteView",
                        "NotePane",
                        "CellView",
                        "ScriptView",
                        "ScriptResults",
                        "TableView",
                    ];
                    let _ = writeln!(file, "seconds,kind,created,dropped,live,coalesced");
                    loop {
                        let result = receiver.recv_timeout(std::time::Duration::from_secs(5));
                        for (i, name) in names.iter().enumerate() {
                            let dropped = DROPPED[i].load(Ordering::Relaxed);
                            let created = CREATED[i].load(Ordering::Relaxed);
                            let lost = INSTANCE
                                .get()
                                .and_then(Option::as_ref)
                                .map_or(0, |m| m.lost.load(Ordering::Relaxed));
                            if writeln!(
                                file,
                                "{:.3},{name},{created},{dropped},{},{lost}",
                                started.elapsed().as_secs_f64(),
                                created.saturating_sub(dropped)
                            )
                            .is_err()
                            {
                                eprintln!("Cannot write lifetime diagnostic log");
                                return;
                            }
                        }
                        if file.flush().is_err() {
                            eprintln!("Cannot flush lifetime diagnostic log");
                            return;
                        }
                        if matches!(result, Err(std::sync::mpsc::RecvTimeoutError::Disconnected))
                            || started.elapsed().as_secs() >= 7200
                        {
                            return;
                        }
                        std::thread::sleep(std::time::Duration::from_secs(1));
                    }
                })
                .ok()?;
            Some(Monitor {
                sender,
                lost: AtomicU64::new(0),
            })
        })
        .as_ref()
}
pub struct Guard(Option<Kind>);
impl Guard {
    pub fn new(kind: Kind) -> Self {
        if monitor().is_some() {
            CREATED[kind as usize].fetch_add(1, Ordering::Relaxed);
            Self(Some(kind))
        } else {
            Self(None)
        }
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(kind) = self.0 {
            DROPPED[kind as usize].fetch_add(1, Ordering::Relaxed);
            if let Some(m) = monitor()
                && m.sender.try_send(()).is_err()
            {
                m.lost.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

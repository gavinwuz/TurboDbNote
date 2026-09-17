use crate::preferences::Preferences;
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::Path,
    sync::{Arc, Condvar, Mutex},
    thread::JoinHandle,
};
use turbodbn_services::agent::Engine;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct SavedMessage {
    pub item: String,
    pub text: String,
    pub user: bool,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct LastChat {
    #[serde(default)]
    pub engine: Engine,
    pub version: u32,
    pub messages: Vec<SavedMessage>,
    pub thread: Option<String>,
    pub executable: String,
    pub cwd: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub interrupted: bool,
    pub elapsed_seconds: u64,
}
impl Default for LastChat {
    fn default() -> Self {
        Self {
            engine: Engine::Codex,
            version: 1,
            messages: vec![],
            thread: None,
            executable: String::new(),
            cwd: String::new(),
            model: None,
            effort: None,
            interrupted: false,
            elapsed_seconds: 0,
        }
    }
}
impl LastChat {
    fn validate(&self) -> Result<(), String> {
        if self.version != 1
            || self.messages.len() > 100
            || self.messages.iter().map(|m| m.text.len()).sum::<usize>() > 1024 * 1024
        {
            return Err("聊天历史版本或大小不受支持".into());
        }
        Ok(())
    }
    pub fn load(engine: Engine) -> Result<Self, String> {
        let path = Preferences::path()
            .map_err(|e| e.to_string())?
            .with_file_name(history_filename(engine));
        let mut snapshot = Self::load_from(&path)?;
        if snapshot.messages.is_empty() && snapshot.thread.is_none() {
            snapshot.engine = engine;
        }
        if snapshot.engine != engine {
            return Err("聊天历史所属引擎不匹配".into());
        }
        Ok(snapshot)
    }
    fn load_from(path: &Path) -> Result<Self, String> {
        let file = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e.to_string()),
        };
        let mut bytes = vec![];
        file.take(8 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > 8 * 1024 * 1024 {
            return Err("聊天历史文件过大".into());
        }
        let saved: Self = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        saved.validate()?;
        Ok(saved)
    }
    fn save_to(&self, path: &Path) -> Result<(), String> {
        self.validate()?;
        (|| -> Result<(), Box<dyn std::error::Error>> {
            let parent = path.parent().ok_or("历史目录无效")?;
            std::fs::create_dir_all(parent)?;
            let mut file = tempfile::NamedTempFile::new_in(parent)?;
            file.write_all(&serde_json::to_vec(self)?)?;
            file.as_file().sync_all()?;
            file.persist(path)?;
            Ok(())
        })()
        .map_err(|e| e.to_string())
    }
}
#[derive(Default)]
struct State {
    latest: std::collections::HashMap<Engine, LastChat>,
    closed: bool,
    error: Option<String>,
}
pub struct HistoryWriter {
    state: Arc<(Mutex<State>, Condvar)>,
    worker: Option<JoinHandle<()>>,
}
impl HistoryWriter {
    pub fn new() -> Self {
        let state = Arc::new((Mutex::new(State::default()), Condvar::new()));
        let shared = state.clone();
        let path = Preferences::path();
        let worker = std::thread::spawn(move || {
            loop {
                let (lock, wake) = &*shared;
                let mut state = lock.lock().unwrap();
                while state.latest.is_empty() && !state.closed {
                    state = wake.wait(state).unwrap();
                }
                let Some(engine) = state.latest.keys().next().copied() else {
                    break;
                };
                let snapshot = state.latest.remove(&engine).unwrap();
                drop(state);
                let result = match &path {
                    Ok(path) => snapshot.save_to(&path.with_file_name(history_filename(engine))),
                    Err(e) => Err(e.to_string()),
                };
                if let Err(error) = result {
                    lock.lock().unwrap().error = Some(error);
                }
            }
        });
        Self {
            state,
            worker: Some(worker),
        }
    }
    pub fn save(&self, snapshot: LastChat) {
        let (lock, wake) = &*self.state;
        lock.lock()
            .unwrap()
            .latest
            .insert(snapshot.engine, snapshot);
        wake.notify_one();
    }
    pub fn take_error(&self) -> Option<String> {
        self.state.0.lock().unwrap().error.take()
    }
}
fn history_filename(engine: Engine) -> &'static str {
    match engine {
        Engine::Codex => "last-chat.json",
        Engine::Claude => "last-chat-claude.json",
    }
}
impl Drop for HistoryWriter {
    fn drop(&mut self) {
        self.state.0.lock().unwrap().closed = true;
        self.state.1.notify_one();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn engines_use_distinct_files_and_pending_snapshots() {
        assert_ne!(
            history_filename(Engine::Codex),
            history_filename(Engine::Claude)
        );
        let mut pending = State::default();
        for engine in [Engine::Codex, Engine::Claude] {
            pending.latest.insert(
                engine,
                LastChat {
                    engine,
                    thread: Some(engine.name().into()),
                    ..Default::default()
                },
            );
        }
        assert_eq!(pending.latest.len(), 2);
        let legacy = serde_json::to_value(LastChat::default()).unwrap();
        let mut legacy = legacy.as_object().unwrap().clone();
        legacy.remove("engine");
        assert_eq!(
            serde_json::from_value::<LastChat>(serde_json::Value::Object(legacy))
                .unwrap()
                .engine,
            Engine::Codex
        );
    }
    #[test]
    fn replaces_snapshot_and_restores_partial_output() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("last-chat.json");
        let mut state = LastChat::default();
        state.messages.push(SavedMessage {
            item: "a".into(),
            text: "部分回复".into(),
            user: false,
        });
        state.thread = Some("thread-a".into());
        state.interrupted = true;
        state.save_to(&path).unwrap();
        let restored = LastChat::load_from(&path).unwrap();
        assert!(restored.interrupted);
        assert_eq!(restored.messages[0].text, "部分回复");
        LastChat::default().save_to(&path).unwrap();
        assert!(LastChat::load_from(&path).unwrap().messages.is_empty());
    }
    #[test]
    fn rejects_corrupt_and_future_history() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("last-chat.json");
        std::fs::write(&path, "bad json").unwrap();
        assert!(LastChat::load_from(&path).is_err());
        let state = LastChat {
            version: 99,
            ..Default::default()
        };
        assert!(state.save_to(&path).is_err());
    }
}

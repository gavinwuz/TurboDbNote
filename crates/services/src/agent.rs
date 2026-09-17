use crate::codex::{Connection, Event};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    #[default]
    Codex,
    Claude,
}
impl Engine {
    pub fn name(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude",
        }
    }
    pub fn executable_name(self) -> &'static str {
        match self {
            Self::Codex => "codex.exe",
            Self::Claude => "claude.exe",
        }
    }
}
pub fn connect(
    engine: Engine,
    path: PathBuf,
    cwd: PathBuf,
) -> (Connection, async_channel::Receiver<Event>) {
    match engine {
        Engine::Codex => Connection::start(path, cwd),
        Engine::Claude => crate::claude::start(path, cwd),
    }
}

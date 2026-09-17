use gpui::Global;
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::PathBuf,
};
use turbodbn_services::agent::Engine;

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EnginePreferences {
    pub executable: String,
    pub working_directory: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub auto_connect: bool,
    pub restore_last_session: bool,
}
impl Default for EnginePreferences {
    fn default() -> Self {
        Self {
            executable: String::new(),
            working_directory: String::new(),
            model: None,
            effort: None,
            auto_connect: true,
            restore_last_session: true,
        }
    }
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AiPreferences {
    pub codex: EnginePreferences,
    pub claude: EnginePreferences,
    pub active: Engine,
}
impl AiPreferences {
    pub fn get(&self, engine: Engine) -> &EnginePreferences {
        match engine {
            Engine::Codex => &self.codex,
            Engine::Claude => &self.claude,
        }
    }
    pub fn get_mut(&mut self, engine: Engine) -> &mut EnginePreferences {
        match engine {
            Engine::Codex => &mut self.codex,
            Engine::Claude => &mut self.claude,
        }
    }
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub dark: Option<bool>,
    pub ai: AiPreferences,
}
impl Global for Preferences {}
impl Preferences {
    pub(crate) fn path() -> std::io::Result<PathBuf> {
        let exe = std::env::current_exe()?;
        if let Some(directory) = exe.parent()
            && directory.join("portable.flag").is_file()
        {
            return Ok(directory.join("data/settings.json"));
        }
        dirs::config_dir()
            .map(|dir| dir.join("TurboDbNote/settings.json"))
            .ok_or_else(|| std::io::Error::other("无法定位用户设置目录"))
    }
    pub fn load() -> Self {
        let result = (|| -> Result<Self, Box<dyn std::error::Error>> {
            let mut data = String::new();
            std::fs::File::open(Self::path()?)?
                .take(65537)
                .read_to_string(&mut data)?;
            if data.len() > 65536 {
                return Err("设置文件过大".into());
            }
            Ok(serde_json::from_str(&data)?)
        })();
        result.unwrap_or_default()
    }
    pub fn save(&self) -> Result<(), String> {
        self.save_to(&Self::path().map_err(|e| e.to_string())?)
    }
    fn save_to(&self, path: &std::path::Path) -> Result<(), String> {
        (|| -> Result<(), Box<dyn std::error::Error>> {
            let parent = path.parent().ok_or("设置目录不存在")?;
            std::fs::create_dir_all(parent)?;
            let mut temp = tempfile::NamedTempFile::new_in(parent)?;
            temp.write_all(&serde_json::to_vec_pretty(self)?)?;
            temp.as_file().sync_all()?;
            temp.persist(path)?;
            Ok(())
        })()
        .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn providers_persist_separately_and_old_settings_default_to_codex() {
        let legacy: Preferences =
            serde_json::from_str(r#"{"ai":{"codex":{"executable":"codex.exe"}}}"#).unwrap();
        assert_eq!(legacy.ai.active, Engine::Codex);
        assert!(legacy.ai.claude.executable.is_empty());
        let mut prefs = legacy;
        let old = prefs.ai.codex.executable.clone();
        prefs.ai.get_mut(Engine::Claude).executable = "claude.exe".into();
        prefs.ai.get_mut(Engine::Claude).model = Some("sonnet".into());
        prefs.ai.active = Engine::Claude;
        let loaded: Preferences =
            serde_json::from_slice(&serde_json::to_vec(&prefs).unwrap()).unwrap();
        assert_eq!(loaded.ai.codex.executable, old);
        assert_eq!(loaded.ai.claude.model.as_deref(), Some("sonnet"));
        assert_eq!(loaded.ai.active, Engine::Claude);
    }
    #[test]
    fn auto_connect_and_restore_default_on_but_can_be_disabled_per_engine() {
        let mut prefs: Preferences =
            serde_json::from_str(r#"{"ai":{"codex":{"executable":"codex.exe"}}}"#).unwrap();
        assert!(prefs.ai.codex.auto_connect && prefs.ai.codex.restore_last_session);
        prefs.ai.claude.auto_connect = false;
        prefs.ai.claude.restore_last_session = false;
        let saved: Preferences =
            serde_json::from_slice(&serde_json::to_vec(&prefs).unwrap()).unwrap();
        assert!(saved.ai.codex.auto_connect && saved.ai.codex.restore_last_session);
        assert!(!saved.ai.claude.auto_connect && !saved.ai.claude.restore_last_session);
    }
    #[test]
    fn atomic_replacement_preserves_provider_settings() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let mut prefs = Preferences::default();
        prefs.ai.codex.executable = "C:\\工具\\codex.exe".into();
        prefs.ai.codex.model = Some("model-a".into());
        prefs.save_to(&path).unwrap();
        prefs.dark = Some(true);
        prefs.save_to(&path).unwrap();
        let loaded: Preferences = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(loaded.dark, Some(true));
        assert_eq!(loaded.ai.codex.executable, prefs.ai.codex.executable);
        assert_eq!(loaded.ai.codex.model, prefs.ai.codex.model);
    }
}

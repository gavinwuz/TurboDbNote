use gpui::Global;
use serde::{Deserialize, Serialize};
use std::{io::Read, path::PathBuf};
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralPreferences {
    pub language: String,
    pub font_family: String,
    pub font_size: f32,
    pub theme: Option<String>,
}
impl Default for GeneralPreferences {
    fn default() -> Self {
        Self {
            language: "zh-CN".into(),
            font_family: ".SystemUIFont".into(),
            font_size: 16.,
            theme: None,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderPreferences {
    pub provider: String,
    pub display_name: String,
    pub api_mode: String,
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
    pub advanced: String,
}
impl Default for ProviderPreferences {
    fn default() -> Self {
        Self {
            provider: "OpenAI".into(),
            display_name: "OpenAI".into(),
            api_mode: "OpenAI Chat Completions".into(),
            endpoint: "https://api.openai.com/v1".into(),
            api_key: String::new(),
            model: String::new(),
            advanced: "{}".into(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentExtension {
    pub name: String,
    pub executable: String,
    pub working_directory: String,
    pub install_url: String,
}
impl Default for AgentExtension {
    fn default() -> Self {
        Self {
            name: "OpenClaw".into(),
            executable: String::new(),
            working_directory: String::new(),
            install_url: "https://docs.openclaw.ai/install".into(),
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub dark: Option<bool>,
    pub ai: AiPreferences,
    pub general: GeneralPreferences,
    pub providers: Vec<ProviderPreferences>,
    #[serde(default = "default_extensions")]
    pub extensions: Vec<AgentExtension>,
    #[serde(skip)]
    pub load_error: Option<String>,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            dark: None,
            ai: AiPreferences::default(),
            general: GeneralPreferences::default(),
            providers: vec![],
            extensions: default_extensions(),
            load_error: None,
        }
    }
}
fn default_extensions() -> Vec<AgentExtension> {
    vec![AgentExtension::default()]
}
impl Global for Preferences {}
impl Preferences {
    pub(crate) fn path() -> std::io::Result<PathBuf> {
        let exe = std::env::current_exe()?;
        if let Some(directory) = exe.parent()
            && directory.join("portable.flag").is_file()
        {
            return Ok(directory.join("data/settings.db"));
        }
        dirs::config_dir()
            .map(|dir| dir.join("TurboDbNote/settings.db"))
            .ok_or_else(|| std::io::Error::other("无法定位用户设置目录"))
    }
    pub fn load() -> Self {
        match Self::path().and_then(|path| Self::load_from(&path).map_err(std::io::Error::other)) {
            Ok(prefs) => prefs,
            Err(error) => {
                eprintln!("Unable to load settings: {error}");
                Self {
                    load_error: Some(error.to_string()),
                    ..Self::default()
                }
            }
        }
    }
    fn connection(path: &std::path::Path) -> Result<rusqlite::Connection, String> {
        std::fs::create_dir_all(path.parent().ok_or("Missing settings directory")?)
            .map_err(|e| e.to_string())?;
        let connection = rusqlite::Connection::open(path).map_err(|e| e.to_string())?;
        connection
            .busy_timeout(std::time::Duration::from_millis(500))
            .map_err(|e| e.to_string())?;
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(|e| e.to_string())?;
        if version > 1 {
            return Err("Settings database was created by a newer application".into());
        }
        connection.execute_batch("CREATE TABLE IF NOT EXISTS settings (id INTEGER PRIMARY KEY CHECK(id=1), value TEXT NOT NULL); PRAGMA user_version=1;").map_err(|e| e.to_string())?;
        Ok(connection)
    }
    fn load_from(path: &std::path::Path) -> Result<Self, String> {
        use rusqlite::OptionalExtension;
        let connection = Self::connection(path)?;
        let json: Option<String> = connection
            .query_row("SELECT value FROM settings WHERE id=1", [], |row| {
                row.get(0)
            })
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some(json) = json {
            return serde_json::from_str(&json).map_err(|e| e.to_string());
        }
        let legacy = path.with_extension("json");
        let prefs = if legacy.is_file() {
            let mut data = String::new();
            std::fs::File::open(legacy)
                .map_err(|e| e.to_string())?
                .take(1_048_577)
                .read_to_string(&mut data)
                .map_err(|e| e.to_string())?;
            if data.len() > 1_048_576 {
                return Err("Settings file too large".into());
            }
            serde_json::from_str(&data).map_err(|e| e.to_string())?
        } else {
            Self::default()
        };
        prefs.save_to(path)?;
        Ok(prefs)
    }
    pub fn save(&self) -> Result<(), String> {
        if let Some(error) = &self.load_error {
            return Err(format!(
                "Settings could not be loaded; original data retained: {error}"
            ));
        }
        self.save_to(&Self::path().map_err(|e| e.to_string())?)
    }
    fn save_to(&self, path: &std::path::Path) -> Result<(), String> {
        let mut connection = Self::connection(path)?;
        let transaction = connection.transaction().map_err(|e| e.to_string())?;
        let json = serde_json::to_string(self).map_err(|e| e.to_string())?;
        transaction.execute("INSERT INTO settings(id,value) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET value=excluded.value", [json]).map_err(|e| e.to_string())?;
        transaction.commit().map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sqlite_round_trips_general_providers_and_extensions() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.db");
        let mut prefs = Preferences::default();
        prefs.general.language = "en".into();
        prefs.general.font_size = 20.;
        prefs.providers.push(ProviderPreferences {
            api_key: "test-key".into(),
            api_mode: "Anthropic Messages".into(),
            model: "test-model".into(),
            advanced: r#"{"temperature":0.5}"#.into(),
            ..Default::default()
        });
        prefs.save_to(&path).unwrap();
        let loaded = Preferences::load_from(&path).unwrap();
        assert_eq!(loaded.general, prefs.general);
        assert_eq!(loaded.providers, prefs.providers);
        assert_eq!(loaded.extensions, prefs.extensions);
    }
    #[test]
    fn invalid_legacy_and_future_schema_are_preserved() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.db");
        std::fs::write(path.with_extension("json"), "invalid").unwrap();
        assert!(Preferences::load_from(&path).is_err());
        let connection = rusqlite::Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM settings", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        connection.pragma_update(None, "user_version", 2).unwrap();
        assert!(Preferences::load_from(&path).is_err());
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
    }
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
    fn migrates_json_once_and_preserves_backup() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.db");
        let legacy = path.with_extension("json");
        std::fs::write(
            &legacy,
            r#"{"dark":true,"ai":{"codex":{"executable":"codex.exe"}}}"#,
        )
        .unwrap();
        let mut prefs = Preferences::load_from(&path).unwrap();
        assert_eq!(prefs.ai.codex.executable, "codex.exe");
        assert!(legacy.exists());
        prefs.dark = Some(false);
        prefs.save_to(&path).unwrap();
        assert_eq!(Preferences::load_from(&path).unwrap().dark, Some(false));
        assert!(
            std::fs::read(&path)
                .unwrap()
                .starts_with(b"SQLite format 3")
        );
    }
    #[test]
    fn atomic_replacement_preserves_provider_settings() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.db");
        let mut prefs = Preferences::default();
        prefs.ai.codex.executable = "C:\\工具\\codex.exe".into();
        prefs.ai.codex.model = Some("model-a".into());
        prefs.save_to(&path).unwrap();
        prefs.dark = Some(true);
        prefs.save_to(&path).unwrap();
        let loaded: Preferences = Preferences::load_from(&path).unwrap();
        assert_eq!(loaded.dark, Some(true));
        assert_eq!(loaded.ai.codex.executable, prefs.ai.codex.executable);
        assert_eq!(loaded.ai.codex.model, prefs.ai.codex.model);
    }
}

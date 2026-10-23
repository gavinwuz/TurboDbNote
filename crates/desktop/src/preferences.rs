use gpui::Global;
use serde::{Deserialize, Serialize};
use std::{io::Read, path::PathBuf};
use turbodbn_services::agent::Engine;

impl DatabasePreferences {
    pub fn timeout_defaults(&self) -> turbodbn_services::timeout_policy::Defaults {
        turbodbn_services::timeout_policy::Defaults {
            connect: self.connect_timeout_seconds,
            execution: self.execution_timeout_seconds,
        }
    }
    pub fn command_enabled(&self, key: &str) -> bool {
        self.sql_commands.as_ref().map_or_else(
            || {
                self.sql_extensions.iter().any(|v| v == key)
                    || turbodbn_services::database::notebook_query::COMMANDS
                        .iter()
                        .chain(turbodbn_services::database::notebook_query::SCRIPT_COMMANDS)
                        .any(|(id, _, enabled)| *id == key && *enabled)
            },
            |commands| commands.iter().any(|v| v == key),
        )
    }
    pub fn set_command(&mut self, key: &str, enabled: bool) {
        let mut commands = turbodbn_services::database::notebook_query::COMMANDS
            .iter()
            .chain(turbodbn_services::database::notebook_query::SCRIPT_COMMANDS)
            .filter(|(id, _, _)| self.command_enabled(id))
            .map(|(id, _, _)| (*id).to_owned())
            .collect::<Vec<_>>();
        commands.retain(|v| v != key);
        if enabled {
            commands.push(key.into());
        }
        self.sql_commands = Some(commands);
    }
}
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
    pub note_directory: String,
    pub note_autosave: bool,
    pub close_to_tray: bool,
    pub theme: Option<String>,
}
impl Default for GeneralPreferences {
    fn default() -> Self {
        Self {
            language: "zh-CN".into(),
            font_family: ".SystemUIFont".into(),
            font_size: 14.,
            note_directory: String::new(),
            note_autosave: true,
            close_to_tray: false,
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SidebarLayout {
    #[default]
    Split,
    Tree,
}
impl SidebarLayout {
    pub const fn toggled(self) -> Self {
        match self {
            Self::Split => Self::Tree,
            Self::Tree => Self::Split,
        }
    }
    pub const fn icon(self) -> &'static str {
        match self {
            Self::Split => "icons/sidebar-split.svg",
            Self::Tree => "icons/sidebar-tree.svg",
        }
    }
    pub const fn hint(self) -> &'static str {
        match self {
            Self::Split => "database.sidebar.split_hint",
            Self::Tree => "database.sidebar.tree_hint",
        }
    }
    pub const fn toggle_tip(self) -> &'static str {
        match self {
            Self::Split => "database.sidebar.switch_tree",
            Self::Tree => "database.sidebar.switch_split",
        }
    }
    pub const fn key(self) -> &'static str {
        match self {
            Self::Split => "database.sidebar.split",
            Self::Tree => "database.sidebar.tree",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DatabasePreferences {
    pub sidebar_layout: SidebarLayout,
    pub page_size: usize,
    pub connect_timeout_seconds: u64,
    pub execution_timeout_seconds: u64,
    pub sql_wrap: bool,
    pub sql_enter: bool,
    pub confirm_destructive: bool,
    pub sql_extensions: Vec<String>,
    pub sql_commands: Option<Vec<String>>,
}
impl Default for DatabasePreferences {
    fn default() -> Self {
        Self {
            sidebar_layout: SidebarLayout::Split,
            page_size: 200,
            connect_timeout_seconds: 5,
            execution_timeout_seconds: turbodbn_core::timeouts::DEFAULT_EXECUTION_SECONDS,
            sql_wrap: true,
            sql_enter: false,
            confirm_destructive: true,
            sql_extensions: vec![],
            sql_commands: None,
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub dark: Option<bool>,
    pub ai: AiPreferences,
    pub general: GeneralPreferences,
    pub database: DatabasePreferences,
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
            database: DatabasePreferences::default(),
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
            return Ok(directory.join("data/connections.db"));
        }
        dirs::config_dir()
            .map(|dir| dir.join("TurboDbNote/connections.db"))
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
        if path.file_name().is_some_and(|n| n == "connections.db") {
            return crate::app_store::settings(path);
        }
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
        if path.file_name().is_some_and(|n| n == "connections.db") {
            return crate::app_store::save(
                &path.with_file_name("settings.json"),
                "settings",
                self,
                1_048_576,
            );
        }
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
    fn global_timeout_defaults_migrate_and_persist_without_connection_overrides() {
        let old: DatabasePreferences = serde_json::from_str("{}").unwrap();
        assert_eq!(old.connect_timeout_seconds, 5);
        assert_eq!(old.execution_timeout_seconds, 300);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.db");
        let mut p = Preferences::default();
        p.database.connect_timeout_seconds = 9;
        p.database.execution_timeout_seconds = 900;
        p.save_to(&path).unwrap();
        let p = Preferences::load_from(&path).unwrap();
        assert_eq!(
            p.database.timeout_defaults(),
            turbodbn_services::timeout_policy::Defaults {
                connect: 9,
                execution: 900
            }
        );
        assert!(
            turbodbn_services::database::defaults(turbodbn_services::DatabaseKind::MySql)
                .inherit_timeouts
        );
    }
    #[test]
    fn sqlite_round_trips_general_providers_and_extensions() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.db");
        let mut prefs = Preferences::default();
        prefs.general.language = "en".into();
        prefs.general.font_size = 20.;
        prefs.general.note_directory = directory.path().join("notes").display().to_string();
        prefs.general.note_autosave = false;
        prefs.general.close_to_tray = true;
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
        assert!(loaded.general.close_to_tray);
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

#[cfg(test)]
mod database_preferences_tests {
    use super::*;
    #[test]
    fn older_settings_receive_safe_database_defaults_and_keep_font_choice() {
        let legacy: Preferences = serde_json::from_str(r#"{"general":{"font_size":18}}"#).unwrap();
        assert_eq!(legacy.general.font_size, 18.);
        assert!(legacy.general.note_autosave);
        assert!(!legacy.general.close_to_tray);
        assert!(legacy.general.note_directory.is_empty());
        assert_eq!(Preferences::default().general.font_size, 14.);
        assert_eq!(legacy.database, DatabasePreferences::default());
        let mut changed = legacy;
        changed.general.close_to_tray = true;
        changed.database.page_size = 500;
        changed.database.sql_wrap = false;
        let restored: Preferences =
            serde_json::from_str(&serde_json::to_string(&changed).unwrap()).unwrap();
        assert_eq!(restored.database, changed.database);
        assert!(restored.general.close_to_tray);
        assert!(restored.database.confirm_destructive);
        assert!(!restored.database.sql_enter);
    }
}

#[cfg(test)]
mod command_permissions_tests {
    #[test]
    fn safe_defaults_and_explicit_choices_round_trip() {
        let mut prefs = super::DatabasePreferences::default();
        assert!(prefs.command_enabled("select"));
        assert!(prefs.command_enabled("show_create"));
        assert!(prefs.command_enabled("show_databases"));
        assert!(!prefs.command_enabled("delete"));
        prefs.set_command("select", false);
        prefs.set_command("update", true);
        prefs.set_command("show_databases", false);
        let saved = serde_json::to_string(&prefs).unwrap();
        let loaded: super::DatabasePreferences = serde_json::from_str(&saved).unwrap();
        assert!(!loaded.command_enabled("select"));
        assert!(loaded.command_enabled("update"));
        assert!(!loaded.command_enabled("delete"));
        assert!(!loaded.command_enabled("show_databases"));
    }
}

#[cfg(test)]
mod sidebar_layout_tests {
    use super::*;
    #[test]
    fn old_settings_keep_split_layout_and_tree_choice_round_trips() {
        let old: DatabasePreferences = serde_json::from_str("{}").unwrap();
        assert_eq!(old.sidebar_layout, SidebarLayout::Split);
        let selected = DatabasePreferences {
            sidebar_layout: SidebarLayout::Tree,
            ..Default::default()
        };
        let restored: DatabasePreferences =
            serde_json::from_str(&serde_json::to_string(&selected).unwrap()).unwrap();
        assert_eq!(restored.sidebar_layout, SidebarLayout::Tree);
    }
}

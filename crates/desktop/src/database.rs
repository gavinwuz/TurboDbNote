#[path = "database_ddl.rs"]
mod ddl_ui;
#[path = "database_import.rs"]
mod import_ui;
#[path = "database_navigation.rs"]
mod navigation;
#[path = "database_objects.rs"]
mod objects_ui;
#[path = "database_transfer.rs"]
mod transfer_ui;
pub(crate) use transfer_ui::Connection as SavedConnection;
#[path = "database_workbench.rs"]
mod workbench_ui;
use crate::{connection_secret, connection_store, feedback};
use gpui_component::select::{SearchableVec, Select, SelectEvent, SelectItem, SelectState};
use navigation::Navigation;
use turbodbn_services::database::workbench::{self, Action, Object, Reply, Schema};
use turbodbn_services::{
    DatabaseKind,
    database::{ObjectItem, Operation, OperationKind, OperationResult},
};
pub use workbench_ui::OpenTable;
#[path = "database_tls.rs"]
mod tls_ui;
use crate::preferences::Preferences;
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Disableable, IconName, Selectable, StyledExt, WindowExt,
    button::{Button, ButtonVariants},
    dock::{Panel, PanelEvent},
    input::{Input, InputState},
    menu::{DropdownMenu, PopupMenuItem},
    switch::Switch,
};
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
};
use tls_ui::TlsPrompt;
use turbodbn_services::mysql::{self, Driver, Event, Profile, ServerInfo, Session, TlsMode};
use uuid::Uuid;

#[derive(Clone)]
pub(crate) struct DatabaseChoice(pub(crate) String);
impl SelectItem for DatabaseChoice {
    type Value = String;
    fn title(&self) -> SharedString {
        self.0.clone().into()
    }
    fn value(&self) -> &String {
        &self.0
    }
    fn display_title(&self) -> Option<AnyElement> {
        Some(div().into_any_element())
    }
}

pub struct OpenDatabase;
pub struct OpenObjects;
pub(crate) struct NoteColumnsReady(pub Uuid);
pub struct DatabaseView {
    ddl: ddl_ui::DdlState,
    mcp_target: Option<async_channel::Sender<Result<Reply, String>>>,
    pub(crate) output: Option<Entity<crate::chat::AiLog>>,
    operation_started: HashMap<Uuid, std::time::Instant>,
    focus: FocusHandle,
    sidebar_split: Entity<gpui_component::resizable::ResizableState>,
    object_database_picker: Option<Entity<SelectState<SearchableVec<String>>>>,
    object_database_context: Option<(Uuid, String, Vec<String>)>,
    object_database_subscription: Option<Subscription>,
    object_filter: Entity<InputState>,
    _object_filter: Subscription,
    catalog_objects: Vec<Object>,
    schemas: HashMap<String, Schema>,
    note_auto_attempts: HashMap<Uuid, Profile>,
    note_columns: HashMap<(Uuid, u64, String, String), Vec<workbench::Column>>,
    expanded_objects: std::collections::HashSet<String>,
    pending_object_action: Option<(Object, String)>,
    workbench_target: Option<WeakEntity<crate::table_view::TableView>>,

    choosing_type: bool,
    type_grid: bool,
    typed_drafts: HashMap<DatabaseKind, (Profile, String)>,
    navigation: Navigation,
    navigation_error: bool,
    nav_writer: async_channel::Sender<Navigation>,
    selected_database: Option<(Uuid, String)>,
    object_items: Vec<ObjectItem>,
    object_cursor: Option<String>,
    redis_pattern: String,
    object_details: Vec<(String, String)>,
    selected_object: Option<ObjectItem>,
    detail_scroll: UniformListScrollHandle,
    object_notice: String,
    object_catalog_loaded: bool,
    active_operation: Option<(Uuid, u64, OperationKind)>,
    request_sequence: u64,
    object_scroll: UniformListScrollHandle,
    _nav_task: Task<()>,
    tab: usize,
    connection_details: bool,
    driver_override: bool,
    pending_tls: Option<TlsPrompt>,
    drivers: Entity<crate::drivers::DriverManager>,
    connect_after_save: bool,
    profiles: Vec<Profile>,
    editing: Profile,
    baseline: Profile,
    fields: BTreeMap<&'static str, Entity<InputState>>,
    search: Entity<InputState>,
    database_picker: Entity<SelectState<SearchableVec<DatabaseChoice>>>,
    database_picker_items: Vec<String>,
    _database_picker_subscription: Option<Subscription>,
    status: String,
    saving: bool,
    validation_requested: bool,
    clear_password: bool,
    loaded_password: String,
    password_visible: bool,
    password_load: Option<Task<()>>,
    history: HashMap<Uuid, connection_store::History>,
    tasks: HashMap<Uuid, Task<()>>,
    sessions: HashMap<Uuid, Session>,
    states: HashMap<Uuid, String>,
    info: HashMap<Uuid, ServerInfo>,
    catalog_source: HashMap<Uuid, Profile>,
    generation: HashMap<Uuid, u64>,
    save_task: Option<Task<()>>,
    file: PathBuf,
    temporary_passwords: HashMap<Uuid, connection_secret::Cached>,
    storage_error: bool,
    _search: Subscription,
    _inputs: Vec<Subscription>,
}
const FIELDS: [(&str, &str); 16] = [
    ("sqlite_path", "选择数据库文件"),
    ("charset", "utf8mb4 / gbk / 自定义字符集"),
    ("legacy", "兼容驱动库绝对路径"),
    ("name", "连接名称"),
    ("group", "项目"),
    ("environment", "环境"),
    ("host", "主机"),
    ("type_filter", "筛选数据库类型"),
    ("port", "端口"),
    ("user", "用户名"),
    ("password", "密码（可留空）"),
    ("database", "数据库 *"),
    ("timeout", "连接超时（秒）"),
    ("ca", "CA 证书路径"),
    ("cert", "客户端证书路径"),
    ("key", "客户端私钥路径"),
];
impl DatabaseView {
    /// Connect saved Note scopes without touching the connection editor or sidebar selection.
    /// An explicit field-read may retry; automatic scope activation only attempts each profile once.
    pub(crate) fn mcp_request(
        &mut self,
        id: Uuid,
        generation: u64,
        database: String,
        action: Action,
        cx: &mut Context<Self>,
    ) -> async_channel::Receiver<Result<Reply, String>> {
        let (sender, receiver) = async_channel::bounded(1);
        if self.note_generation(id) != generation
            || self.note_connection_state(id) != Some(true)
            || self.active_operation.is_some()
        {
            let _ = sender.try_send(Err("mcp.stale".into()));
        } else if self.request_operation(
            id,
            OperationKind::Workbench(Box::new(workbench::Request { database, action })),
            cx,
        ) {
            self.mcp_target = Some(sender);
        } else {
            let _ = sender.try_send(Err("mcp.failed".into()));
        }
        receiver
    }
    pub(crate) fn ensure_note_connection(&mut self, id: Uuid, retry: bool, cx: &mut Context<Self>) {
        if self.sessions.contains_key(&id)
            || self.states.get(&id).is_some_and(|s| s.ends_with("中…"))
        {
            return;
        }
        let Some(profile) = self.note_profile(id).cloned() else {
            return;
        };
        if !retry && self.note_auto_attempts.get(&id) == Some(&profile) {
            return;
        }
        self.note_auto_attempts.insert(id, profile.clone());
        if let Some(key) = required_connection_field(&profile, false) {
            feedback::error(crate::locale::t(cx, key), cx);
            return;
        }
        if let Err(error) = profile.validate() {
            feedback::error(error, cx);
            return;
        }
        if self
            .states
            .values()
            .filter(|s| s.as_str() == "已连接" || s.ends_with("中…"))
            .count()
            >= 8
        {
            feedback::error(crate::locale::t(cx, "notes.connection_limit"), cx);
            return;
        }
        self.start_profile(false, false, None, Some(profile), cx);
    }
    pub(crate) fn note_connection_state(&self, id: Uuid) -> Option<bool> {
        match self.states.get(&id).map(String::as_str) {
            Some("已连接") => Some(true),
            Some("连接中…") => None,
            _ => Some(false),
        }
    }
    pub(crate) fn note_generation(&self, id: Uuid) -> u64 {
        self.generation.get(&id).copied().unwrap_or_default()
    }
    pub(crate) fn note_column_object(
        &self,
        id: Uuid,
        database: &str,
        table: &turbodbn_services::database::sql_assist::TableName,
    ) -> Option<Object> {
        let profile = self.note_profile(id)?;
        let schema = match profile.kind {
            DatabaseKind::PostgreSql => table.schema.clone(),
            DatabaseKind::Sqlite
                if table.schema.is_empty() || table.schema.eq_ignore_ascii_case("main") =>
            {
                String::new()
            }
            DatabaseKind::MySql | DatabaseKind::ClickHouse
                if table.schema.is_empty() || table.schema == database =>
            {
                String::new()
            }
            _ => return None,
        };
        Some(Object {
            schema,
            name: table.name.clone(),
            kind: "table".into(),
            comment: String::new(),
            parent: String::new(),
        })
    }
    pub(crate) fn cached_note_columns(
        &self,
        id: Uuid,
        database: &str,
        object: &Object,
    ) -> Option<Vec<workbench::Column>> {
        if self
            .selected_database
            .as_ref()
            .is_some_and(|(selected, db)| *selected == id && db == database)
            && let Some(schema) = self.schemas.get(&Self::object_key(object))
        {
            return Some(schema.columns.clone());
        }
        self.note_columns
            .get(&(
                id,
                self.note_generation(id),
                database.into(),
                Self::object_key(object),
            ))
            .cloned()
    }
    pub(crate) fn request_note_columns(
        &mut self,
        id: Uuid,
        database: String,
        object: Object,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.active_operation.is_some() {
            return false;
        }
        self.request_operation(
            id,
            OperationKind::Workbench(Box::new(workbench::Request {
                database,
                action: Action::NoteColumns(object),
            })),
            cx,
        )
    }
    pub(crate) fn remember_note_columns(
        &mut self,
        id: Uuid,
        database: &str,
        object: &Object,
        columns: &[workbench::Column],
    ) {
        if self.note_columns.len() >= 32 {
            self.note_columns.clear();
        }
        self.note_columns.insert(
            (
                id,
                self.note_generation(id),
                database.into(),
                Self::object_key(object),
            ),
            columns.iter().take(1024).cloned().collect(),
        );
    }
    pub(crate) fn note_tables(
        &self,
        id: Uuid,
        database: &str,
    ) -> Vec<turbodbn_services::database::sql_assist::TableName> {
        if !self.sessions.contains_key(&id)
            || self.states.get(&id).is_none_or(|s| s != "已连接")
            || self
                .selected_database
                .as_ref()
                .is_none_or(|(selected, db)| *selected != id || db != database)
            || !self.object_catalog_loaded
        {
            return vec![];
        }
        self.catalog_objects
            .iter()
            .filter(|o| matches!(o.kind.as_str(), "table" | "view"))
            .take(2000)
            .map(|o| turbodbn_services::database::sql_assist::TableName {
                schema: o.schema.clone(),
                name: o.name.clone(),
            })
            .collect()
    }
    pub(crate) fn note_databases(&self, id: Uuid) -> Vec<String> {
        self.profiles
            .iter()
            .find(|p| p.id == id)
            .map(|p| {
                let mut names = self.catalog(p);
                if !p.database.is_empty() {
                    names.push(p.database.clone());
                }
                names.sort();
                names.dedup();
                names
            })
            .unwrap_or_default()
    }
    pub(crate) fn note_profile(&self, id: Uuid) -> Option<&Profile> {
        self.profiles.iter().find(|profile| profile.id == id)
    }
    pub(crate) fn note_profiles(&self) -> Vec<Profile> {
        self.profiles.clone()
    }
    pub(crate) fn note_projects(&self) -> Vec<String> {
        let mut projects = self
            .navigation
            .categories
            .iter()
            .cloned()
            .chain(self.profiles.iter().map(|p| p.group.clone()))
            .filter(|p| !p.trim().is_empty())
            .collect::<Vec<_>>();
        projects.sort();
        projects.dedup();
        projects.sort_by_key(|name| {
            self.navigation
                .order
                .iter()
                .position(|p| p == name)
                .unwrap_or(usize::MAX)
        });
        projects
    }
    pub(crate) fn move_note_project(&mut self, from: &str, to: &str, cx: &mut Context<Self>) {
        let mut projects = self.note_projects();
        if let (Some(a), Some(b)) = (
            projects.iter().position(|p| p == from),
            projects.iter().position(|p| p == to),
        ) {
            let project = projects.remove(a);
            projects.insert(b, project);
            // Preserve the database sidebar's unassigned category position.
            let unassigned = self.navigation.order.iter().position(|p| p == "未分组");
            if let Some(position) = unassigned {
                projects.insert(position.min(projects.len()), "未分组".into());
            }
            self.navigation.order = projects;
            self.persist_navigation();
            cx.notify();
        }
    }
    pub(crate) fn ensure_note_project(&mut self, name: &str, cx: &mut Context<Self>) {
        if !name.trim().is_empty() && !self.note_projects().iter().any(|p| p == name) {
            self.navigation.categories.push(name.to_owned());
            self.persist_navigation();
            cx.notify();
        }
    }
    pub fn drivers(&self) -> Entity<crate::drivers::DriverManager> {
        self.drivers.clone()
    }

    fn browse_legacy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.editing.id;
        let selection = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("选择 MySQL 5.1–5.6 x64 libmysql.dll".into()),
        });
        cx.spawn_in(window, async move |view, cx| {
            if let Ok(Ok(Some(paths))) = selection.await
                && let Some(path) = paths.first()
            {
                let _ = cx.update(|window, cx| {
                    let _ = view.update(cx, |this, cx| {
                        if this.editing.id == id && !this.saving {
                            this.fields["legacy"].update(cx, |input, cx| {
                                input.set_value(path.display().to_string(), window, cx)
                            });
                            cx.notify();
                        }
                    });
                });
            }
        })
        .detach();
    }
    pub fn summary(&self, cx: &App) -> SharedString {
        let count = self
            .states
            .values()
            .filter(|s| s.as_str() == "已连接")
            .count();
        crate::locale::tr(
            cx,
            "notes.connection_summary",
            &[("count", &count.to_string())],
        )
    }
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let file = Preferences::path()
            .unwrap_or_else(|_| PathBuf::from("connections.db"))
            .with_file_name("connections.db");
        let status = crate::locale::t(cx, "feedback.storage.loading").to_string();
        let search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(crate::locale::t(cx, "dbwork.connections.filter"))
        });
        let fields: BTreeMap<&'static str, Entity<InputState>> = FIELDS
            .iter()
            .map(|(key, label)| {
                (
                    *key,
                    cx.new(|cx| {
                        let input = InputState::new(window, cx).placeholder(match *key {
                            "name" => crate::locale::t(cx, "dbwork.connection.name.required"),
                            "group" => crate::locale::t(cx, "dbwork.project.name"),
                            "type_filter" => crate::locale::t(cx, "grid.connection.filter"),
                            "database" => {
                                crate::locale::t(cx, "dbwork.connection.database.required")
                            }
                            _ => SharedString::from(*label),
                        });
                        if *key == "password" {
                            input.masked(true)
                        } else if *key == "port" {
                            input.validate(|value, _| {
                                value.is_empty()
                                    || (value.bytes().all(|b| b.is_ascii_digit())
                                        && value.parse::<u16>().is_ok_and(|port| port > 0))
                            })
                        } else {
                            input
                        }
                    }),
                )
            })
            .collect();
        let inputs = fields
            .values()
            .map(|input| cx.observe(input, |_, _, cx| cx.notify()))
            .collect();
        let nav_path = file.with_file_name("database-navigation.json");
        let loaded_navigation = navigation::load(&nav_path);
        let navigation_error = loaded_navigation.is_err();
        if let Err(error) = &loaded_navigation {
            let error = error.clone();
            cx.defer(move |cx| feedback::error(error, cx));
        }
        let navigation = loaded_navigation.unwrap_or_default();
        let (nav_writer, nav_errors) = navigation::writer(nav_path);
        let nav_task = cx.spawn(async move |view, cx| {
            while let Ok(error) = nav_errors.recv().await {
                if view
                    .update(cx, |this, cx| {
                        feedback::error(error.clone(), cx);
                        this.status = error;
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        let object_filter = cx.new(|cx| {
            InputState::new(window, cx).placeholder(crate::locale::t(cx, "dbwork.objects.filter"))
        });
        let mut this = Self {
            sidebar_split: cx.new(|_| gpui_component::resizable::ResizableState::default()),
            _object_filter: cx.observe(&object_filter, |_, _, cx| cx.notify()),
            object_filter,
            output: None,
            operation_started: HashMap::new(),
            object_database_picker: None,
            object_database_context: None,
            object_database_subscription: None,
            catalog_objects: Vec::new(),
            schemas: HashMap::new(),
            note_auto_attempts: HashMap::new(),
            note_columns: HashMap::new(),
            expanded_objects: std::collections::HashSet::new(),
            workbench_target: None,
            ddl: Default::default(),
            mcp_target: None,
            pending_object_action: None,
            focus: cx.focus_handle(),
            choosing_type: false,
            type_grid: true,
            typed_drafts: HashMap::new(),
            navigation,
            navigation_error,
            nav_writer,
            selected_database: None,
            object_items: Vec::new(),
            object_cursor: None,
            redis_pattern: "*".into(),
            object_details: Vec::new(),
            selected_object: None,
            detail_scroll: UniformListScrollHandle::new(),
            object_notice: String::new(),
            object_catalog_loaded: false,
            active_operation: None,
            request_sequence: 0,
            object_scroll: UniformListScrollHandle::new(),
            _nav_task: nav_task,
            tab: 0,
            connection_details: false,
            driver_override: false,
            pending_tls: None,
            drivers: cx.new(crate::drivers::DriverManager::new),
            connect_after_save: false,
            profiles: Vec::new(),
            editing: Profile::default(),
            baseline: Profile::default(),
            fields,
            search: search.clone(),
            database_picker: cx.new(|cx| {
                SelectState::new(
                    SearchableVec::new(Vec::<DatabaseChoice>::new()),
                    None,
                    window,
                    cx,
                )
                .searchable(true)
            }),
            database_picker_items: Vec::new(),
            _database_picker_subscription: None,
            status,
            saving: true,
            validation_requested: false,
            clear_password: false,
            loaded_password: String::new(),
            password_visible: false,
            password_load: None,
            history: HashMap::new(),
            tasks: HashMap::new(),
            sessions: HashMap::new(),
            states: HashMap::new(),
            info: HashMap::new(),
            catalog_source: HashMap::new(),
            generation: HashMap::new(),
            save_task: None,
            file,
            temporary_passwords: HashMap::new(),
            storage_error: false,
            _search: cx.observe(&search, |_, _, cx| cx.notify()),
            _inputs: inputs,
        };
        this.fill(Self::empty_profile(DatabaseKind::MySql), window, cx);
        let path = this.file.clone();
        let loading = cx.background_executor().spawn(async move {
            let snapshot = connection_store::load(&path)?;
            let cleanup_warning = connection_store::migrate_passwords(&path).err();
            Ok::<_, String>((snapshot, cleanup_warning))
        });
        this.save_task = Some(cx.spawn(async move |view, cx| {
            let result = loading.await;
            let _ = view.update(cx, |this, cx| {
                this.saving = false;
                match result {
                    Ok((snapshot, cleanup_warning)) => {
                        if let Some(warning) = cleanup_warning {
                            feedback::notify(
                                gpui_component::notification::NotificationType::Warning,
                                warning,
                                cx,
                            );
                        }
                        this.profiles = snapshot.profiles;
                        this.history = snapshot.history;
                        let mut used = this
                            .history
                            .iter()
                            .filter_map(|(id, h)| {
                                h.last_used_at.as_ref().map(|at| (*id, at.clone()))
                            })
                            .collect::<Vec<_>>();
                        used.sort_by(|a, b| b.1.cmp(&a.1));
                        let mut recent = used.into_iter().map(|(id, _)| id).collect::<Vec<_>>();
                        for id in &this.navigation.recent {
                            if !recent.contains(id) && this.profiles.iter().any(|p| p.id == *id) {
                                recent.push(*id);
                            }
                        }
                        recent.truncate(5);
                        this.navigation.recent = recent;
                        this.status = crate::locale::t(cx, "feedback.storage.ready").to_string();
                    }
                    Err(error) => {
                        this.storage_error = true;
                        this.status = error.clone();
                        feedback::error(error, cx);
                    }
                }
                cx.notify();
            });
        }));
        this
    }
    pub(crate) fn operation_log(
        &mut self,
        id: Uuid,
        kind: crate::operation_log::Kind,
        phase: crate::operation_log::Phase,
        error: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        use crate::operation_log::{Phase, Record};
        if matches!(phase, Phase::Started) {
            self.operation_started.insert(id, std::time::Instant::now());
        }
        let mut record = Record::new(id, kind, phase);
        record.source = Some(id);
        record.elapsed_ms = self
            .operation_started
            .get(&id)
            .map_or(0, |at| at.elapsed().as_millis() as u64);
        if let Some(error) = error {
            record.failure = crate::operation_log::failure_class(error);
        }
        if let Some(log) = &self.output {
            log.update(cx, |log, cx| log.operation(record, cx));
        }
        if matches!(
            phase,
            Phase::Succeeded | Phase::Failed | Phase::Cancelled | Phase::Disconnected
        ) {
            self.operation_started.remove(&id);
        }
    }
    fn text(&self, key: &str, cx: &App) -> String {
        self.fields[key].read(cx).value().trim().to_string()
    }
    fn fill(&mut self, profile: Profile, window: &mut Window, cx: &mut Context<Self>) {
        self.choosing_type = false;
        self.validation_requested = false;
        if !self.storage_error {
            self.status = self
                .states
                .get(&profile.id)
                .map(|s| format!("当前状态：{s}"))
                .unwrap_or_else(|| {
                    if self.profiles.iter().any(|p| p.id == profile.id) {
                        "连接配置已载入".into()
                    } else {
                        "新连接尚未保存".into()
                    }
                });
        }
        self.editing = profile.clone();
        self.baseline = profile.clone();
        self.clear_password = false;
        self.load_password(&profile, window, cx);
        let data = [
            ("sqlite_path", profile.sqlite_path),
            ("charset", profile.charset),
            ("legacy", profile.legacy_library),
            ("name", profile.name),
            ("group", profile.group),
            ("environment", profile.environment),
            ("host", profile.host),
            ("port", profile.port.to_string()),
            ("user", profile.user),
            ("password", self.loaded_password.clone()),
            ("database", profile.database),
            ("timeout", profile.timeout_seconds.to_string()),
            ("ca", profile.ca),
            ("cert", profile.client_cert),
            ("key", profile.client_key),
        ];
        for (key, value) in data {
            self.fields[key].update(cx, |input, cx| input.set_value(value, window, cx));
        }
        cx.notify();
    }
    fn raw_draft(&self, cx: &App) -> Result<Profile, String> {
        let profile = Profile {
            sqlite_path: self.text("sqlite_path", cx),
            charset: self.text("charset", cx).to_ascii_lowercase(),
            legacy_library: self.text("legacy", cx),
            name: self.text("name", cx),
            group: self.text("group", cx),
            environment: self.text("environment", cx),
            host: self.text("host", cx),
            port: self
                .text("port", cx)
                .parse()
                .map_err(|_| "端口应为 1–65535")?,
            user: self.text("user", cx),
            database: self.text("database", cx),
            timeout_seconds: self
                .text("timeout", cx)
                .parse()
                .map_err(|_| "超时应为 1–60 秒")?,
            ca: self.text("ca", cx),
            client_cert: self.text("cert", cx),
            client_key: self.text("key", cx),
            ..self.editing.clone()
        };
        Ok(profile)
    }
    fn empty_profile(kind: DatabaseKind) -> Profile {
        let mut p = turbodbn_services::database::defaults(kind);
        p.name.clear();
        p.remember_password = kind != DatabaseKind::Sqlite;
        p.tls = TlsMode::Disabled;
        p.allow_old_auth = kind == DatabaseKind::MySql;
        if kind == DatabaseKind::Redis {
            p.database = "0".into();
        }
        p
    }
    fn draft_for(&self, discover: bool, cx: &App) -> Result<Profile, String> {
        let mut p = self.raw_draft(cx)?;
        if p.kind == DatabaseKind::Redis && p.redis_cluster {
            p.database = "0".into();
        }
        if let Some(key) = required_connection_field(&p, discover) {
            return Err(crate::locale::t(cx, key).to_string());
        }
        let mut validation = p.clone();
        if discover && validation.name.is_empty() {
            validation.name = "connection-test".into();
        }
        validation.validate()?;
        Ok(p)
    }
    fn draft(&self, cx: &App) -> Result<Profile, String> {
        self.draft_for(false, cx)
    }
    pub(crate) fn dirty(&self, cx: &App) -> bool {
        self.clear_password
            || self.raw_draft(cx).ok().as_ref() != Some(&self.baseline)
            || self.fields["password"].read(cx).value().as_str() != self.loaded_password
    }
    fn select(&mut self, id: Uuid, open_editor: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        if let Some(profile) = self.profiles.iter().find(|p| p.id == id).cloned() {
            if self.dirty(cx) {
                if self.editing.id == id {
                    if open_editor {
                        cx.emit(OpenDatabase);
                    }
                    return;
                }
                if open_editor {
                    let view = cx.entity().downgrade();
                    feedback::confirm_labels(
                        crate::locale::t(cx, "dbwork.draft.switch_title"),
                        crate::locale::t(cx, "dbwork.draft.switch_body"),
                        crate::locale::t(cx, "dbwork.draft.keep"),
                        crate::locale::t(cx, "dbwork.draft.discard"),
                        window,
                        cx,
                        move |window, cx| {
                            let _ = view.update(cx, |this, cx| {
                                if !this.saving {
                                    this.discard_editor(window, cx);
                                    this.select(id, true, window, cx);
                                }
                            });
                        },
                    );
                } else {
                    feedback::error(crate::locale::t(cx, "dbwork.connection.unsaved"), cx);
                }
                return;
            }
            if !self.follow_connection(id, false, cx) {
                return;
            }
            self.fill(profile, window, cx);
            if open_editor {
                cx.emit(OpenDatabase);
            }
        }
    }
    fn disconnect(&mut self, id: Uuid, cx: &mut Context<Self>) {
        if let Some(profile) = self.note_profile(id).cloned() {
            self.note_auto_attempts.insert(id, profile);
        }
        if self.sessions.contains_key(&id) {
            self.operation_log(
                id,
                crate::operation_log::Kind::Connection,
                crate::operation_log::Phase::Disconnected,
                None,
                cx,
            );
        }

        *self.generation.entry(id).or_default() += 1;
        self.note_columns
            .retain(|(connection, _, _, _), _| *connection != id);
        self.tasks.remove(&id);
        self.sessions.remove(&id);
        if self
            .active_operation
            .as_ref()
            .is_some_and(|(connection, _, _)| *connection == id)
        {
            self.active_operation = None;
            self.cancel_table_request(cx);
            self.object_notice = "操作已取消；如正在建库，请重连确认结果".into();
        }
        self.states.insert(id, "未连接".into());
        if self
            .selected_database
            .as_ref()
            .is_some_and(|(selected, _)| *selected == id)
        {
            self.clear_object_view();
            self.object_notice = crate::locale::t(cx, "dbwork.switch.disconnected").to_string();
        }
        self.status = "已断开 / 取消".into();
        cx.notify();
    }
    fn entered_password(&self, cx: &App) -> String {
        let value = self.fields["password"].read(cx).value();
        if value.as_str() == self.loaded_password {
            String::new()
        } else {
            value.to_string()
        }
    }
    fn load_password(&mut self, profile: &Profile, window: &mut Window, cx: &mut Context<Self>) {
        self.password_load = None;
        self.password_visible = false;
        self.fields["password"].update(cx, |input, cx| input.set_masked(true, window, cx));
        self.loaded_password = self
            .temporary_passwords
            .get(&profile.id)
            .filter(|_| !profile.remember_password)
            .and_then(|secret| secret.value_for(profile))
            .unwrap_or_default();
        if !self.loaded_password.is_empty() || !profile.remember_password {
            return;
        }
        let Some(key) = self.history.get(&profile.id).and_then(|h| h.credential_id) else {
            return;
        };
        let profile = profile.clone();
        let path = self.file.clone();
        let work = cx
            .background_executor()
            .spawn(async move { connection_store::read_password(&path, key) });
        self.password_load = Some(cx.spawn_in(window, async move |view, cx| {
            let result = work.await;
            let _ = cx.update(|window, cx| {
                let _ = view.update(cx, |this, cx| {
                    this.password_load = None;
                    cx.notify();
                    if this.editing.id != profile.id
                        || this.clear_password
                        || !this.fields["password"].read(cx).value().is_empty()
                        || !this
                            .raw_draft(cx)
                            .is_ok_and(|draft| connection_secret::same_target(&draft, &profile))
                    {
                        return;
                    }
                    match result {
                        Ok(Some(value)) => {
                            this.loaded_password = value.clone();
                            this.fields["password"]
                                .update(cx, |input, cx| input.set_value(value, window, cx));
                            cx.notify();
                        }
                        Ok(None) => {}
                        Err(_) => feedback::error(
                            crate::locale::t(cx, "feedback.password.load_failed"),
                            cx,
                        ),
                    }
                });
            });
        }));
    }
    fn password_input(
        &self,
        profile: &Profile,
        cx: &App,
    ) -> Result<connection_secret::Input, String> {
        connection_secret::input(
            profile,
            self.profiles.iter().find(|p| p.id == profile.id),
            self.temporary_passwords.get(&profile.id),
            self.entered_password(cx),
            self.clear_password && self.fields["password"].read(cx).value().is_empty(),
        )
        .map_err(|key| crate::locale::t(cx, key).to_string())
    }
    fn start(&mut self, test: bool, cx: &mut Context<Self>) {
        self.start_request(test, false, cx);
    }
    fn start_request(&mut self, test: bool, discover: bool, cx: &mut Context<Self>) {
        self.start_attempt(test, discover, None, cx);
    }
    fn start_attempt(
        &mut self,
        test: bool,
        discover: bool,
        trial: Option<(TlsMode, bool)>,
        cx: &mut Context<Self>,
    ) {
        self.start_profile(test, discover, trial, None, cx);
    }
    fn start_profile(
        &mut self,
        test: bool,
        discover: bool,
        trial: Option<(TlsMode, bool)>,
        saved: Option<Profile>,
        cx: &mut Context<Self>,
    ) {
        let background = saved.is_some();
        if !background {
            self.pending_tls = None;
            self.validation_requested = !test;
        }
        let mut profile = match saved
            .map(Ok)
            .unwrap_or_else(|| self.draft_for(test || discover, cx))
        {
            Ok(p) => p,
            Err(e) => {
                feedback::error(e.clone(), cx);
                self.status = e;
                cx.notify();
                return;
            }
        };
        if !test
            && trial.is_none()
            && self.profiles.iter().find(|p| p.id == profile.id) != Some(&profile)
        {
            self.status = "请先保存连接配置，再建立持久连接；可直接测试未保存的草稿".into();
            cx.notify();
            return;
        }
        let snapshot = profile.clone();
        if test && profile.name.is_empty() {
            profile.name = "connection-test".into();
        }
        let mut catalog_source = snapshot.clone();
        if let Some((mode, _)) = trial {
            profile.tls = mode;
        }
        let attempted_mode = profile.tls;
        catalog_source.database.clear();
        profile.list_all_databases = true;
        if discover && profile.kind != DatabaseKind::PostgreSql {
            profile.database.clear();
        }
        if (mysql::uses_legacy(&profile) || profile.driver == Driver::Auto)
            && profile.legacy_library.is_empty()
            && let Some(path) = self.drivers.read(cx).path()
        {
            profile.legacy_library = path.display().to_string();
        }
        if let Err(e) = mysql::route(&profile) {
            if mysql::uses_legacy(&profile) && profile.tls != TlsMode::Disabled {
                self.pending_tls = Some(TlsPrompt::Failure {
                    profile: snapshot,
                    mode: attempted_mode,
                    issue: mysql::TlsIssue::LegacyUnsupported,
                    discover,
                });
            }
            self.status = e;
            cx.notify();
            return;
        }
        if self.sessions.len() >= 8 && !self.sessions.contains_key(&profile.id) {
            self.status = "最多同时连接 8 个数据库".into();
            cx.notify();
            return;
        }
        let id = profile.id;
        if !test && !background && !self.follow_connection(id, false, cx) {
            return;
        }
        self.disconnect(id, cx);
        self.catalog_source.insert(id, catalog_source);
        self.info.remove(&id);
        let generation = self.generation[&id];
        let password = match if background {
            Ok(
                if let Some(value) = self
                    .temporary_passwords
                    .get(&id)
                    .and_then(|s| s.value_for(&snapshot))
                {
                    connection_secret::Input::Value(value)
                } else if snapshot.remember_password
                    && self
                        .history
                        .get(&id)
                        .and_then(|h| h.credential_id)
                        .is_some()
                {
                    connection_secret::Input::Stored
                } else {
                    connection_secret::Input::Value(String::new())
                },
            )
        } else {
            self.password_input(&snapshot, cx)
        } {
            Ok(p) => p,
            Err(error) => {
                self.status = error.clone();
                feedback::error(error, cx);
                cx.notify();
                return;
            }
        };
        self.states
            .insert(id, if test { "测试中…" } else { "连接中…" }.into());
        self.status = "正在后台连接…".into();
        self.operation_log(
            id,
            crate::operation_log::Kind::Connection,
            crate::operation_log::Phase::Started,
            None,
            cx,
        );
        let credential_id = self.history.get(&id).and_then(|h| h.credential_id);
        let secret_path = self.file.clone();
        let secret = cx.background_executor().spawn(async move {
            match password {
                connection_secret::Input::Value(value) => Ok(value),
                connection_secret::Input::Stored => connection_store::read_password(
                    &secret_path,
                    credential_id.ok_or("保存的密码引用缺失，请重新保存密码")?,
                )?
                .ok_or_else(|| "已保存的凭据不存在，请重新填写密码".to_string()),
            }
        });
        self.tasks.insert(
            id,
            cx.spawn(async move |view, cx| {
                let password = match secret.await {
                    Ok(password) => password,
                    Err(error) => {
                        let _ = view.update(cx, |this, cx| {
                            if this.generation.get(&id) == Some(&generation) {
                                this.operation_log(
                                    id,
                                    crate::operation_log::Kind::Connection,
                                    crate::operation_log::Phase::Failed,
                                    Some(&error),
                                    cx,
                                );
                                feedback::error(error.clone(), cx);
                                this.status = error;
                                this.states.insert(id, "凭据错误".into());
                                cx.notify();
                            }
                        });
                        return;
                    }
                };
                let cached = connection_secret::Cached::new(&snapshot, password.clone());
                let (session, events) = Session::start(profile, password, test);
                let accepted = view
                    .update(cx, |this, _| {
                        if this.generation.get(&id) != Some(&generation) {
                            return false;
                        }
                        this.sessions.insert(id, session);
                        true
                    })
                    .unwrap_or(false);
                if !accepted {
                    return;
                }
                while let Ok(event) = events.recv().await {
                    let result =
                        view.update(cx, |this, cx| {
                            if this.generation.get(&id) != Some(&generation) {
                                return;
                            }
                            let previous_status = this.status.clone();
                            match event {
                                Event::Connected(info) => {
                                    this.operation_log(
                                        id,
                                        crate::operation_log::Kind::Connection,
                                        crate::operation_log::Phase::Succeeded,
                                        None,
                                        cx,
                                    );
                                    if this.profiles.iter().find(|p| p.id == id).is_none_or(
                                        |saved| connection_secret::same_target(saved, &snapshot),
                                    ) {
                                        this.temporary_passwords.insert(id, cached.clone());
                                    }
                                    this.remember_connection(id, cx);
                                    this.status = format!(
                                        "连接成功 · {} {} · {} ms",
                                        snapshot.kind.label(),
                                        info.version,
                                        info.elapsed_ms
                                    );
                                    if trial.is_some() {
                                        this.status.push_str(&format!(
                                            " · 本次模式：{}",
                                            tls_ui::mode_label(attempted_mode)
                                        ));
                                    }
                                    feedback::success(this.status.clone(), cx);
                                    this.states.insert(id, "已连接".into());
                                    this.info.insert(id, info);
                                    if this
                                        .selected_database
                                        .as_ref()
                                        .is_some_and(|(selected, _)| *selected == id)
                                    {
                                        this.follow_connection(id, true, cx);
                                    }
                                }
                                Event::Tested(info) => {
                                    this.operation_log(
                                        id,
                                        crate::operation_log::Kind::Connection,
                                        crate::operation_log::Phase::Succeeded,
                                        None,
                                        cx,
                                    );
                                    if this.profiles.iter().find(|p| p.id == id).is_none_or(
                                        |saved| connection_secret::same_target(saved, &snapshot),
                                    ) {
                                        this.temporary_passwords.insert(id, cached.clone());
                                    }
                                    this.status = format!(
                                        "测试成功 · {} {} · {} ms（测试连接已释放）",
                                        snapshot.kind.label(),
                                        info.version,
                                        info.elapsed_ms
                                    );
                                    if discover {
                                        this.status = format!(
                                            "检测到 {} 个数据库，可在数据库下拉中选择",
                                            info.databases.len()
                                        );
                                    }
                                    if let Some(error) = &info.database_list_error {
                                        this.status.push_str(&format!(" · {error}"));
                                    }
                                    this.states.insert(
                                        id,
                                        if discover {
                                            "已检测"
                                        } else {
                                            "测试通过"
                                        }
                                        .into(),
                                    );
                                    feedback::success(this.status.clone(), cx);
                                    this.info.insert(id, info);
                                    this.sessions.remove(&id);
                                    if let Some((mode, remember)) = trial
                                        && this.raw_draft(cx).ok().as_ref() == Some(&snapshot)
                                    {
                                        this.pending_tls = Some(TlsPrompt::Success {
                                            profile: snapshot.clone(),
                                            mode,
                                            remember,
                                        });
                                    }
                                }
                                Event::Failed(error) => {
                                    this.operation_log(
                                        id,
                                        crate::operation_log::Kind::Connection,
                                        crate::operation_log::Phase::Failed,
                                        Some(&error),
                                        cx,
                                    );
                                    if this
                                        .active_operation
                                        .as_ref()
                                        .is_some_and(|(connection, _, _)| *connection == id)
                                    {
                                        this.active_operation = None;
                                        this.cancel_table_request(cx);
                                        this.object_notice = error.clone();
                                    }
                                    if let Some(issue) = mysql::TlsIssue::from_message(&error)
                                        && attempted_mode != TlsMode::Disabled
                                        && this.raw_draft(cx).ok().as_ref() == Some(&snapshot)
                                    {
                                        this.pending_tls = Some(TlsPrompt::Failure {
                                            profile: snapshot.clone(),
                                            mode: attempted_mode,
                                            issue,
                                            discover,
                                        });
                                    }
                                    if this.pending_tls.is_none() {
                                        feedback::error(error.clone(), cx);
                                    }
                                    this.status = error;
                                    this.states.insert(id, "连接失败".into());
                                    this.sessions.remove(&id);
                                }
                                Event::Operation {
                                    id: request,
                                    result,
                                } => this.operation_result(id, request, result, cx),
                                Event::Disconnected => {
                                    if this
                                        .active_operation
                                        .as_ref()
                                        .is_some_and(|(connection, _, _)| *connection == id)
                                    {
                                        this.active_operation = None;
                                        this.cancel_table_request(cx);
                                        this.object_notice =
                                            "操作连接已断开；建库结果请重连确认".into();
                                    }
                                    if this.states.get(&id).is_none_or(|state| state != "连接失败")
                                    {
                                        this.states.insert(id, "未连接".into());
                                    }
                                    this.sessions.remove(&id);
                                }
                            }
                            if this.editing.id != id {
                                this.status = previous_status;
                            }
                            cx.notify();
                        });
                    if result.is_err() {
                        break;
                    }
                }
            }),
        );
        cx.notify();
    }
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.storage_error {
            self.status = "连接存储读取失败，已阻止覆盖原文件；请备份并修复存储后重启".into();
            cx.notify();
            return;
        }
        if self.saving {
            return;
        }
        self.validation_requested = true;
        let profile = match self.draft(cx) {
            Ok(p) => p,
            Err(e) => {
                feedback::error(e.clone(), cx);
                self.status = e;
                cx.notify();
                return;
            }
        };
        let id = profile.id;
        let mut profile = profile;
        if self.clear_password && self.fields["password"].read(cx).value().is_empty() {
            profile.remember_password = false;
        }
        let input = match self.password_input(&profile, cx) {
            Ok(p) => p,
            Err(error) => {
                self.status = error.clone();
                feedback::error(error, cx);
                cx.notify();
                return;
            }
        };
        if self.sessions.contains_key(&self.editing.id) {
            let id = self.editing.id;
            let view = cx.entity().downgrade();
            feedback::confirm(
                crate::locale::t(cx, "dbwork.edit.disconnect_title"),
                crate::locale::t(cx, "dbwork.edit.disconnect_body"),
                window,
                cx,
                move |window, cx| {
                    let _ = view.update(cx, |this, cx| {
                        if this.editing.id == id && !this.saving {
                            this.disconnect(id, cx);
                            this.save(window, cx);
                        }
                    });
                },
            );
            return;
        }
        let old_credential = self.history.get(&id).and_then(|h| h.credential_id);
        let file = self.file.clone();
        let payload = profile.clone();
        self.saving = true;
        self.status = "正在保存连接配置…".into();
        let work = cx.background_executor().spawn(async move {
            let password = match input {
                connection_secret::Input::Value(value) => value,
                connection_secret::Input::Stored => connection_store::read_password(
                    &file,
                    old_credential.ok_or("保存的密码引用缺失，请重新填写密码")?,
                )?
                .ok_or("保存的密码已缺失，请重新填写密码")?,
            };
            let history = connection_store::save_password(&file, &payload, &password)?;
            let warning = connection_store::cleanup_credentials(&file).err();
            Ok::<_, String>((password, history, warning))
        });
        self.save_task = Some(cx.spawn_in(window, async move |view, cx| {
            let result = work.await;
            let _ = cx.update(|window, cx| {
                let _ = view.update(cx, |this, cx| {
                    this.saving = false;
                    match result {
                        Ok((password, mut history, warning)) => {
                            if let Some(warning) = warning {
                                feedback::notify(
                                    gpui_component::notification::NotificationType::Warning,
                                    warning,
                                    cx,
                                );
                            }
                            if let Some(previous) = this.history.get(&id)
                                && previous.last_used_at > history.last_used_at
                            {
                                history.last_used_at = previous.last_used_at.clone();
                            }
                            this.history.insert(id, history);
                            this.note_columns
                                .retain(|(connection, _, _, _), _| *connection != id);
                            if let Some(saved) = this.profiles.iter_mut().find(|p| p.id == id) {
                                *saved = profile.clone();
                            } else {
                                this.profiles.push(profile.clone());
                            }
                            this.temporary_passwords
                                .insert(id, connection_secret::Cached::new(&profile, password));
                            this.fill(profile, window, cx);
                            this.status =
                                crate::locale::t(cx, "feedback.connection.saved").to_string();
                            feedback::success(this.status.clone(), cx);
                            if this.connect_after_save {
                                this.connect_after_save = false;
                                this.start(false, cx);
                            }
                        }
                        Err(error) => {
                            this.connect_after_save = false;
                            this.status = format!("保存失败：{error}");
                            feedback::error(this.status.clone(), cx);
                        }
                    };
                    cx.notify();
                });
            });
        }));
        cx.notify();
    }
    fn delete(&mut self, id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        self.disconnect(id, cx);
        if self.editing.id == id {
            self.clear_password = false;
        }
        self.saving = true;
        let path = self.file.clone();
        let work = cx.background_executor().spawn(async move {
            connection_store::delete(&path, id)?;
            Ok::<_, String>(connection_store::cleanup_credentials(&path).err())
        });
        self.save_task = Some(cx.spawn_in(window, async move |view, cx| {
            let result = work.await;
            let _ = cx.update(|window, cx| {
                let _ = view.update(cx, |this, cx| {
                    this.saving = false;
                    match result {
                        Ok(warning) => {
                            if let Some(warning) = warning {
                                feedback::notify(
                                    gpui_component::notification::NotificationType::Warning,
                                    warning,
                                    cx,
                                );
                            }
                            this.profiles.retain(|p| p.id != id);
                            this.temporary_passwords.remove(&id);
                            if this.editing.id == id {
                                this.fill(Self::empty_profile(DatabaseKind::MySql), window, cx);
                            }
                            this.history.remove(&id);
                            this.navigation.recent.retain(|i| *i != id);
                            this.navigation.created.remove(&id);
                            this.navigation.databases.remove(&id);
                            this.persist_navigation();
                            if this
                                .selected_database
                                .as_ref()
                                .is_some_and(|(current, _)| *current == id)
                            {
                                this.selected_database = None;
                                this.catalog_objects.clear();
                                this.schemas.clear();
                            }
                            this.status =
                                crate::locale::t(cx, "feedback.connection.deleted").to_string();
                            feedback::success(this.status.clone(), cx);
                        }
                        Err(error) => {
                            this.status = format!("删除失败：{error}");
                            feedback::error(this.status.clone(), cx);
                        }
                    };
                    cx.notify();
                });
            });
        }));
        cx.notify();
    }
    fn catalog(&self, profile: &Profile) -> Vec<String> {
        let mut context = profile.clone();
        context.database.clear();
        if self.catalog_source.get(&profile.id) != Some(&context) {
            return Vec::new();
        }
        self.info
            .get(&profile.id)
            .map(|info| info.databases.clone())
            .unwrap_or_default()
    }
    pub(crate) fn editor_busy(&self) -> bool {
        self.saving
    }
    pub(crate) fn discard_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.typed_drafts.clear();
        self.reset(window, cx);
    }
    fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let profile = self
            .profiles
            .iter()
            .find(|p| p.id == self.editing.id)
            .cloned()
            .unwrap_or_else(|| Self::empty_profile(self.editing.kind));
        self.fill(profile, window, cx);
    }
    fn connect_saved(&mut self, id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving
            || self.sessions.contains_key(&id)
            || self.states.get(&id).is_some_and(|s| s.ends_with("中…"))
        {
            return;
        }
        self.select(id, false, window, cx);
        if self.editing.id == id && !self.dirty(cx) {
            self.start(false, cx);
        }
    }
    fn connection_menu(
        &self,
        mut menu: gpui_component::menu::PopupMenu,
        id: Uuid,
        form: bool,
        cx: &Context<Self>,
    ) -> gpui_component::menu::PopupMenu {
        let view = cx.entity().downgrade();
        let saved = self.profiles.iter().any(|p| p.id == id);
        let connected = self.sessions.contains_key(&id);
        for (key, icon, action) in [
            (
                if connected {
                    "dbwork.connection.disconnect"
                } else {
                    "dbwork.connection.connect"
                },
                IconName::ArrowRight,
                "connect",
            ),
            ("dbwork.connection.edit", IconName::Settings2, "edit"),
            ("dbwork.connection.copy", IconName::Copy, "copy"),
            ("feedback.storage.title", IconName::Info, "records"),
            ("dbwork.connection.delete", IconName::Delete, "delete"),
        ] {
            let view = view.clone();
            menu = menu.item(
                PopupMenuItem::new(crate::locale::t(cx, key))
                    .icon(icon)
                    .disabled(self.saving || (!saved && action != "copy" && action != "records"))
                    .on_click(move |_, window, cx| {
                        let _ = view.update(cx, |this, cx| {
                            if this.saving {
                                return;
                            }
                            match action {
                                "connect" => {
                                    if this.sessions.contains_key(&id) {
                                        this.disconnect(id, cx);
                                        feedback::success(
                                            crate::locale::t(
                                                cx,
                                                "feedback.connection.disconnected",
                                            ),
                                            cx,
                                        );
                                    } else {
                                        this.connect_saved(id, window, cx);
                                    }
                                }
                                "edit" => this.select(id, true, window, cx),
                                "records" => this.show_storage(id, window, cx),
                                "copy" => {
                                    if this.editing.id != id && this.dirty(cx) {
                                        feedback::error(
                                            crate::locale::t(cx, "dbwork.connection.unsaved"),
                                            cx,
                                        );
                                        return;
                                    }
                                    let profile = if form {
                                        this.draft(cx).ok()
                                    } else {
                                        this.profiles.iter().find(|p| p.id == id).cloned()
                                    };
                                    if let Some(mut profile) = profile {
                                        profile.id = Uuid::new_v4();
                                        profile.name.push_str(" 副本");
                                        profile.remember_password = false;
                                        this.fill(profile, window, cx);
                                        cx.emit(OpenDatabase);
                                        feedback::success(
                                            crate::locale::t(cx, "dbwork.connection.copied"),
                                            cx,
                                        );
                                    }
                                }
                                "delete" => {
                                    let view = cx.entity().downgrade();
                                    let name = this
                                        .profiles
                                        .iter()
                                        .find(|p| p.id == id)
                                        .map(|p| p.name.clone())
                                        .unwrap_or_default();
                                    feedback::confirm(
                                        crate::locale::t(cx, "feedback.connection.delete.title"),
                                        crate::locale::tr(
                                            cx,
                                            "dbwork.connection.delete_target",
                                            &[("name", &name)],
                                        ),
                                        window,
                                        cx,
                                        move |window, cx| {
                                            let _ = view.update(cx, |this, cx| {
                                                if !this.saving
                                                    && this.profiles.iter().any(|p| p.id == id)
                                                {
                                                    this.delete(id, window, cx);
                                                }
                                            });
                                        },
                                    );
                                }
                                _ => {}
                            }
                        });
                    }),
            );
        }
        if form {
            menu = menu.separator().item(
                PopupMenuItem::new(crate::locale::t(cx, "dbwork.connection.discard"))
                    .icon(IconName::Undo2)
                    .disabled(self.saving)
                    .on_click(move |_, window, cx| {
                        let _ = view.update(cx, |this, cx| {
                            if this.editing.id == id && !this.saving {
                                this.reset(window, cx);
                            }
                        });
                    }),
            );
        }
        menu
    }
    fn more_actions(&self, cx: &Context<Self>) -> AnyElement {
        let view = cx.entity().downgrade();
        let id = self.editing.id;
        Button::new("connection-more")
            .icon(IconName::Ellipsis)
            .label("更多")
            .disabled(self.saving)
            .dropdown_menu(move |menu, _, cx| {
                if let Some(view) = view.upgrade() {
                    view.update(cx, |this, cx| this.connection_menu(menu, id, true, cx))
                } else {
                    menu
                }
            })
            .into_any_element()
    }
    fn show_storage(&self, id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        let h = self.history.get(&id).cloned().unwrap_or_default();
        let unknown = crate::locale::t(cx, "feedback.time.unknown").to_string();
        let created = connection_store::display_time(h.created_at.as_deref())
            .unwrap_or_else(|| unknown.clone());
        let updated = connection_store::display_time(h.updated_at.as_deref())
            .unwrap_or_else(|| unknown.clone());
        let used = connection_store::display_time(h.last_used_at.as_deref()).unwrap_or(unknown);
        let body = crate::locale::tr(
            cx,
            "feedback.storage.details",
            &[
                ("created", &created),
                ("updated", &updated),
                ("used", &used),
            ],
        );
        let path = self.file.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let path = path.clone();
            dialog
                .title(crate::locale::t(cx, "feedback.storage.title"))
                .w(px(560.))
                .child(
                    div()
                        .w(px(510.))
                        .h_flex()
                        .items_start()
                        .gap_2()
                        .child(gpui_component::Icon::new(IconName::Info))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .whitespace_normal()
                                .child(body.clone()),
                        ),
                )
                .footer(move |_, _, _, cx| {
                    let path = path.clone();
                    vec![
                        feedback::footer_actions()
                            .child(
                                Button::new("open-connection-folder")
                                    .icon(IconName::FolderOpen)
                                    .label(crate::locale::t(cx, "feedback.storage.open"))
                                    .on_click(move |_, _, cx| {
                                        if path.exists() {
                                            cx.reveal_path(&path);
                                        } else {
                                            feedback::error(
                                                crate::locale::t(cx, "feedback.storage.missing"),
                                                cx,
                                            );
                                        }
                                    }),
                            )
                            .child(feedback::close_button(cx)),
                    ]
                })
        });
    }
    fn password_field(&self, cx: &Context<Self>) -> AnyElement {
        let draft = self.raw_draft(cx).unwrap_or_else(|_| self.editing.clone());
        let saved = self.profiles.iter().find(|p| p.id == draft.id);
        let cached = self.temporary_passwords.get(&draft.id);
        let entered = !self.entered_password(cx).is_empty();
        let available = cached.is_some() || saved.is_some_and(|p| p.remember_password);
        let hint = if entered {
            "feedback.password.replace"
        } else if self.clear_password {
            "feedback.password.clear_pending"
        } else if available
            && (cached.is_some_and(|p| !p.matches(&draft))
                || saved.is_some_and(|p| {
                    p.remember_password && !connection_secret::same_target(p, &draft)
                }))
        {
            "feedback.password.changed_target"
        } else if cached.is_some_and(|secret| secret.matches(&draft) && secret.is_empty()) {
            "feedback.password.empty"
        } else if saved.is_some_and(|p| p.remember_password) {
            "feedback.password.stored"
        } else if cached.is_some() {
            "feedback.password.session"
        } else {
            "feedback.password.empty"
        };
        div()
            .v_flex()
            .gap_1()
            .child(crate::locale::t(cx, "feedback.password.label"))
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(
                        Input::new(&self.fields["password"])
                            .disabled(self.saving || self.password_load.is_some())
                            .flex_1(),
                    )
                    .child(
                        Button::new("toggle-connection-password")
                            .ghost()
                            .icon(if self.password_visible {
                                IconName::EyeOff
                            } else {
                                IconName::Eye
                            })
                            .tooltip(crate::locale::t(
                                cx,
                                if self.password_visible {
                                    "feedback.password.hide"
                                } else {
                                    "feedback.password.show"
                                },
                            ))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.password_visible = !this.password_visible;
                                this.fields["password"].update(cx, |input, cx| {
                                    input.set_masked(!this.password_visible, window, cx)
                                });
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("clear-connection-password")
                            .ghost()
                            .icon(IconName::Delete)
                            .tooltip(crate::locale::t(cx, "feedback.password.clear"))
                            .disabled(self.saving || (!available && !entered))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let view = cx.entity().downgrade();
                                let snapshot = this.editing.id;
                                feedback::confirm(
                                    crate::locale::t(cx, "feedback.password.clear"),
                                    crate::locale::t(cx, "feedback.password.clear.confirm"),
                                    window,
                                    cx,
                                    move |window, cx| {
                                        let _ = view.update(cx, |this, cx| {
                                            if this.editing.id == snapshot && !this.saving {
                                                this.clear_password = true;
                                                this.editing.remember_password = false;
                                                this.fields["password"].update(cx, |input, cx| {
                                                    input.set_value("", window, cx)
                                                });
                                                feedback::success(
                                                    crate::locale::t(
                                                        cx,
                                                        "feedback.password.clear_pending",
                                                    ),
                                                    cx,
                                                );
                                                cx.notify();
                                            }
                                        });
                                    },
                                );
                            })),
                    ),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(crate::locale::t(cx, hint)),
            )
            .into_any_element()
    }
    fn required_field(
        &self,
        key: &'static str,
        label: &str,
        error: &str,
        cx: &Context<Self>,
    ) -> AnyElement {
        div()
            .v_flex()
            .gap_1()
            .w_full()
            .child(crate::locale::t(cx, label))
            .child(
                Input::new(&self.fields[key])
                    .when(key == "name", |input| {
                        input.suffix(
                            Button::new("generate-connection-name")
                                .ghost()
                                .icon(
                                    gpui_component::Icon::default().path("icons/name-generate.svg"),
                                )
                                .tooltip(crate::locale::t(cx, "grid.connection.generate"))
                                .disabled(self.saving)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let p = Profile {
                                        database: this.text("database", cx),
                                        host: this.text("host", cx),
                                        sqlite_path: this.text("sqlite_path", cx),
                                        ..this.editing.clone()
                                    };
                                    let name = generated_connection_name(&p, &this.profiles);
                                    this.fields["name"]
                                        .update(cx, |input, cx| input.set_value(name, window, cx));
                                    feedback::success(
                                        crate::locale::t(cx, "grid.connection.generated"),
                                        cx,
                                    );
                                })),
                        )
                    })
                    .disabled(self.saving),
            )
            .when(
                self.validation_requested && self.text(key, cx).is_empty(),
                |el| {
                    el.child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().danger)
                            .child(crate::locale::t(cx, error)),
                    )
                },
            )
            .into_any_element()
    }
    fn field(&self, key: &'static str, label: &'static str) -> AnyElement {
        div()
            .v_flex()
            .gap_1()
            .w_full()
            .child(label)
            .child(
                Input::new(&self.fields[key])
                    .cleanable(true)
                    .disabled(self.saving),
            )
            .into_any_element()
    }
    fn sync_database_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut values = self
            .raw_draft(cx)
            .ok()
            .map(|p| self.catalog(&p))
            .unwrap_or_default();
        let mut seen = std::collections::HashSet::new();
        values.retain(|v| !v.trim().is_empty() && seen.insert(v.clone()));
        if values == self.database_picker_items {
            return;
        }
        // A new detection/context resets the transient filter; typing never changes the database input.
        let picker = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(
                    values
                        .iter()
                        .cloned()
                        .map(DatabaseChoice)
                        .collect::<Vec<_>>(),
                ),
                None,
                window,
                cx,
            )
            .searchable(true)
        });
        self._database_picker_subscription = Some(cx.subscribe_in(
            &picker,
            window,
            |this, _, event: &SelectEvent<SearchableVec<DatabaseChoice>>, window, cx| {
                if let SelectEvent::Confirm(Some(value)) = event
                    && !this.saving
                    && this
                        .raw_draft(cx)
                        .is_ok_and(|p| this.catalog(&p).contains(value))
                {
                    this.fields["database"]
                        .update(cx, |input, cx| input.set_value(value.clone(), window, cx));
                    cx.notify();
                }
            },
        ));
        self.database_picker = picker;
        self.database_picker_items = values;
    }
    fn combo(
        &self,
        key: &'static str,
        label: &'static str,
        mut values: Vec<String>,
        cx: &Context<Self>,
    ) -> AnyElement {
        values.retain(|v| !v.trim().is_empty());
        let mut seen = std::collections::HashSet::new();
        values.retain(|v| seen.insert(v.clone()));
        let dropdown = if key == "database" {
            div()
                .w(px(32.))
                .h(px(28.))
                .child(
                    Select::new(&self.database_picker)
                        .appearance(false)
                        .placeholder("")
                        .px_1()
                        .h(px(28.))
                        .menu_width(px(320.))
                        .search_placeholder(crate::locale::t(cx, "dbwork.database.filter"))
                        .disabled(self.saving || self.database_picker_items.is_empty()),
                )
                .into_any_element()
        } else {
            let view = cx.entity().downgrade();
            Button::new(key)
                .icon(IconName::ChevronDown)
                .tooltip("选择已有值，也可直接输入")
                .disabled(self.saving || values.is_empty())
                .dropdown_menu(move |mut menu, _, _| {
                    for value in &values {
                        let view = view.clone();
                        let value = value.clone();
                        let item = if key == "environment" {
                            let label = value.clone();
                            PopupMenuItem::element(move |_, _| {
                                div()
                                    .h_flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .size(px(10.))
                                            .rounded_sm()
                                            .bg(navigation::environment_color(&label)),
                                    )
                                    .child(label.clone())
                            })
                        } else {
                            PopupMenuItem::new(value.clone())
                        };
                        menu = menu.item(item.on_click(move |_, window, cx| {
                            let _ = view.update(cx, |this, cx| {
                                if this.saving {
                                    return;
                                }
                                this.fields[key].update(cx, |input, cx| {
                                    input.set_value(value.clone(), window, cx)
                                });
                                cx.notify();
                            });
                        }));
                    }
                    menu.scrollable(true).max_h(px(320.))
                })
                .into_any_element()
        };
        div()
            .v_flex()
            .flex_1()
            .min_w_0()
            .gap_1()
            .text_sm()
            .child(if key == "database" {
                crate::locale::t(cx, "dbwork.connection.database")
            } else if key == "group" {
                crate::locale::t(cx, "dbwork.project.field")
            } else {
                SharedString::from(label)
            })
            .child(
                Input::new(&self.fields[key])
                    .cleanable(true)
                    .disabled(self.saving)
                    .suffix(dropdown),
            )
            .when(
                key == "database" && self.validation_requested && self.text(key, cx).is_empty(),
                |el| {
                    el.child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().danger)
                            .child(crate::locale::t(cx, "dbwork.connection.database.required")),
                    )
                },
            )
            .into_any_element()
    }
    fn choices(
        &self,
        id: &'static str,
        label: String,
        values: Vec<(&'static str, &'static str)>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let view = cx.entity().downgrade();
        Button::new(id)
            .label(label)
            .icon(IconName::ChevronDown)
            .disabled(self.saving)
            .dropdown_menu(move |mut menu, _, _| {
                for (title, value) in &values {
                    let view = view.clone();
                    let value = *value;
                    menu = menu.item(PopupMenuItem::new(*title).on_click(move |_, window, cx| {
                        if id == "tls" && value != "verify" {
                            let source = view.clone();
                            let snapshot = view.upgrade().map(|v| v.read(cx).editing.id);
                            feedback::confirm(
                                crate::locale::t(cx, "feedback.tls.confirm"),
                                crate::locale::t(cx, "feedback.tls.warning"),
                                window,
                                cx,
                                move |_, cx| {
                                    let _ = source.update(cx, |this, cx| {
                                        if Some(this.editing.id) == snapshot && !this.saving {
                                            this.editing.tls = if value == "off" {
                                                TlsMode::Disabled
                                            } else {
                                                TlsMode::Required
                                            };
                                            cx.notify();
                                        }
                                    });
                                },
                            );
                            return;
                        }
                        let _ = view.update(cx, |this, cx| {
                            match id {
                                "driver" => {
                                    this.editing.driver = match value {
                                        "legacy" => Driver::Legacy,
                                        "modern" => Driver::Modern,
                                        _ => Driver::Auto,
                                    }
                                }
                                "tls" => {
                                    this.editing.tls = match value {
                                        "off" => TlsMode::Disabled,
                                        "required" => TlsMode::Required,
                                        _ => TlsMode::VerifyIdentity,
                                    }
                                }
                                "charset" => this.editing.charset = value.into(),
                                "target" => {
                                    this.editing.target_version = value.into();
                                    this.editing.charset = this.text("charset", cx);
                                    if value == "4.0" {
                                        this.editing.charset = "server-default".into();
                                    } else if mysql::uses_legacy(&this.editing)
                                        && this.editing.charset == "utf8mb4"
                                    {
                                        this.editing.charset = "utf8".into();
                                    }
                                    let charset = this.editing.charset.clone();
                                    this.fields["charset"].update(cx, |input, cx| {
                                        input.set_value(charset, window, cx)
                                    });
                                }
                                _ => {}
                            }
                            cx.notify();
                        });
                    }));
                }
                menu
            })
            .into_any_element()
    }
}
impl EventEmitter<OpenDatabase> for DatabaseView {}
impl EventEmitter<OpenObjects> for DatabaseView {}
impl EventEmitter<NoteColumnsReady> for DatabaseView {}
impl EventEmitter<PanelEvent> for DatabaseView {}
impl Focusable for DatabaseView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for DatabaseView {
    fn panel_name(&self) -> &'static str {
        "database-manager"
    }
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::tabs::title(
            "database-manager",
            IconName::GalleryVerticalEnd,
            crate::locale::t(cx, "database.connection.title"),
            cx,
        )
    }
    fn dropdown_menu(
        &mut self,
        menu: gpui_component::menu::PopupMenu,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_component::menu::PopupMenu {
        crate::tabs::editor_menu(menu, "database-manager", cx)
    }
    fn closable(&self, _: &App) -> bool {
        false
    }
}
impl Render for DatabaseView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_database_picker(window, cx);
        if let Some(prompt) = self.pending_tls.take() {
            cx.defer_in(window, move |this, window, cx| {
                this.show_tls_prompt(prompt, window, cx)
            });
        }
        if self.choosing_type {
            return self.render_type_picker(cx);
        }
        let id = self.editing.id;
        let running = self.sessions.contains_key(&id)
            || self.states.get(&id).is_some_and(|s| s.ends_with("中…"));
        div()
            .id("connection-editor")
            .size_full()
            .max_w(px(800.))
            .min_h_0()
            .v_flex()
            .gap_4()
            .p_4()
            .track_focus(&self.focus)
            .child(div().h_flex().justify_between().child(div().text_lg().child(format!("{} 连接",self.editing.kind.label())))
                .when(!self.profiles.iter().any(|p|p.id==id),|el|el.child(Button::new("choose-database-type").icon(IconName::ArrowLeft).label("选择类型")
                    .on_click(cx.listener(|this,_,_,cx|{this.choosing_type=true;cx.notify();})))))
            .child(div().h_flex().gap_2().children(["连接信息", "TLS / SSL", "高级"].into_iter().enumerate().map(|(index,label)| {
                Button::new(("connection-tab",index)).label(label).selected(self.tab==index).disabled(index==1&&self.editing.kind==DatabaseKind::Sqlite)
                    .on_click(cx.listener(move |this,_,_,cx|{this.tab=index;cx.notify();}))
            })))
            .child(div().id("connection-form-body").flex_1().min_h_0().overflow_y_scroll().v_flex().gap_3()
            .when(self.tab==0,|el|el
            .child(self.required_field("name", "dbwork.connection.name", "dbwork.connection.name.required",cx))
            .child(
                div()
                    .h_flex()
                    .gap_3()
                    .child(self.combo("group", "",self.profiles.iter().map(|p|p.group.clone()).chain(self.navigation.categories.clone()).collect(),cx))
                    .child(self.combo("environment", "环境（可自定义）",["开发","测试","预发","正式"].into_iter().map(String::from).chain(self.profiles.iter().map(|p|p.environment.clone())).chain(self.navigation.environments.clone()).collect(),cx)),
            )
            .when(self.editing.kind!=DatabaseKind::Sqlite,|el|el
            .child(
                div()
                    .h_flex()
                    .gap_3()
                    .child(self.field("host", "主机"))
                    .child(self.field("port", "端口")),
            )
            .when(self.editing.kind==DatabaseKind::ClickHouse,|el|el.child("ClickHouse HTTP：通常 8123；HTTPS：通常 8443，请与 TLS 模式对应。"))
            .child(self.field("user", if self.editing.kind==DatabaseKind::Redis {"用户名（可选，Redis ACL）"}else{"用户名"}))
            .child(self.password_field(cx))
            .child(
                Switch::new("remember-db-password")
                    .label(crate::locale::t(cx, "feedback.password.remember"))
                    .checked(self.editing.remember_password)
                    .disabled(self.saving)
                    .on_click(cx.listener(|this, value: &bool, _, cx| {
                        this.editing.remember_password = *value;
                        cx.notify();
                    })),
            )
            .when(!self.editing.remember_password, |el| el.child(
                div().h_flex().items_start().gap_2().text_sm().text_color(cx.theme().muted_foreground)
                    .child(gpui_component::Icon::new(IconName::Info))
                    .child(div().flex_1().min_w_0().whitespace_normal().child(crate::locale::t(cx,
                        if self.profiles.iter().any(|p| p.id == id && p.remember_password) {
                            "feedback.password.memory_pending"
                        } else { "feedback.password.memory_only" })))
            ))
            )
            .when(self.editing.kind==DatabaseKind::Sqlite,|el|el
                .child(self.required_field("sqlite_path","dbwork.connection.file","dbwork.connection.file.required",cx))
                .child(div().h_flex().gap_2()
                    .child(Button::new("sqlite-browse").icon(IconName::FolderOpen).label("选择文件").disabled(self.saving).on_click(cx.listener(|this,_,window,cx|this.browse_sqlite(false,window,cx))))
                    .child(Button::new("sqlite-create").icon(IconName::Plus).label("新建文件").disabled(self.saving).on_click(cx.listener(|this,_,window,cx|this.browse_sqlite(true,window,cx)))))
                .child(Switch::new("sqlite-read-only").label("只读连接").checked(self.editing.sqlite_read_only).disabled(self.saving)
                    .on_click(cx.listener(|this,v:&bool,_,cx|{this.editing.sqlite_read_only = *v;cx.notify();}))))
            .when(self.editing.kind==DatabaseKind::Redis,|el|el.child(Switch::new("redis-cluster").label("Redis 集群（仅 DB 0，浏览当前节点）").checked(self.editing.redis_cluster).disabled(self.saving)
                .on_click(cx.listener(|this,v:&bool,_,cx|{this.editing.redis_cluster = *v;cx.notify();}))))
            .when(self.editing.kind!=DatabaseKind::Sqlite,|el|el
            .child(div().h_flex().gap_2().items_end()
                .child(div().flex_1().min_w_0().child(self.combo("database","数据库 *",self.raw_draft(cx).ok().map(|p|self.catalog(&p)).unwrap_or_default(),cx)))
                .child(Button::new("detect-databases").flex_shrink_0().icon(IconName::Search).label("检测")
                    .tooltip("使用当前连接信息获取账号可见的数据库，不保存草稿")
                    .disabled(self.saving||running).on_click(cx.listener(|this,_,_,cx|this.start_request(true,true,cx)))))
            )
            )
            .when(self.tab==2,|el|el
            .when(self.editing.kind==DatabaseKind::MySql,|el|el
            .child(Button::new("driver-override").ghost().icon(IconName::Settings2).label(crate::locale::t(cx,"dbwork.driver_override"))
                  .on_click(cx.listener(|this,_,_,cx|{this.driver_override = !this.driver_override;cx.notify();})))
              .when(self.driver_override, |el| el
.child(
                div()
                    .h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(self.choices(
                        "driver",
                        format!(
                            "驱动：{}",
                            match self.editing.driver {
                                Driver::Auto => "自动选择",
                                Driver::Modern => "现代 MySQL",
                                Driver::Legacy => "旧版兼容",
                            }
                        ),
                        vec![
                            ("自动选择", "auto"),
                            ("现代 MySQL", "modern"),
                            ("旧版兼容", "legacy"),
                        ],
                        cx,
                    ))
                    .child(self.choices(
                        "target",
                        format!("目标：{}", self.editing.target_version),
                        vec![
                            ("自动检测", "自动检测"),
                            ("5.6", "5.6"),
                            ("5.7", "5.7"),
                            ("8.0", "8.0"),
                            ("5.0–5.5（兼容）", "5.0–5.5"),
                            ("4.1（兼容）", "4.1"),
                            ("4.0（兼容）", "4.0"),
                        ],
                        cx,
                    ))
                    ,
            ))
              .child(self.combo("charset","字符集（可自定义）",["utf8mb4","utf8","gb2312","gbk","latin1","server-default"].into_iter().map(String::from).collect(),cx))
            .when(mysql::uses_legacy(&self.editing) || self.editing.driver == Driver::Auto,|el|el
                .when(self.driver_override, |el| el.child(self.field("legacy","自定义 DLL 路径（留空使用已安装驱动）"))
                .child(Button::new("browse-legacy-driver").label("浏览驱动库").disabled(self.saving)
                    .on_click(cx.listener(|this,_,window,cx|this.browse_legacy(window,cx)))))
                .child(Switch::new("allow-old-auth").label("允许 pre-4.1 旧密码认证（仅兼容连接）")
                    .checked(self.editing.allow_old_auth).disabled(self.saving)
                    .on_click(cx.listener(|this,checked:&bool,window,cx|{
                        if !*checked{this.editing.allow_old_auth=false;cx.notify();return;}
                        let view=cx.entity().downgrade();let id=this.editing.id;
                        feedback::confirm(crate::locale::t(cx,"feedback.auth.confirm"),crate::locale::t(cx,"feedback.auth.warning"),window,cx,move|_,cx|{let _=view.update(cx,|this,cx|{if this.editing.id==id&&!this.saving{this.editing.allow_old_auth=true;cx.notify();}});});
                    })))
                .when(self.driver_override, |el| el.child("兼容驱动运行于独立进程。当前需显式禁用 TLS；4.0 使用服务器默认字符集。请选择可信驱动库。")))
            )
            .child(self.field("timeout", "连接/测试总超时（1–60 秒）"))
            .when(self.editing.kind==DatabaseKind::MySql && self.driver_override,|el|el.child(self.drivers.clone()))
            )
            .when(self.tab==1,|el|el
            .child(self.choices(
                "tls",
                format!(
                    "TLS：{}",
                    match self.editing.tls {
                        TlsMode::VerifyIdentity => "加密并验证身份",
                        TlsMode::Required => "仅加密",
                        TlsMode::Disabled => "不加密",
                    }
                ),
                vec![
                    ("加密并验证身份（推荐）", "verify"),
                    ("仅加密（不验证身份）", "required"),
                    ("不加密", "off"),
                ],
                cx,
            ))
            .when(mysql::uses_legacy(&self.editing),|el|el.child(div().v_flex().gap_2()
                .child("当前兼容驱动暂不支持 TLS，可明确切换为不加密。")
                .child(Button::new("legacy-disable-tls").icon(IconName::Settings2).label("切换为不加密")
                    .disabled(self.saving||self.editing.tls==TlsMode::Disabled)
                    .on_click(cx.listener(|this,_,window,cx|{let view=cx.entity().downgrade();let id=this.editing.id;feedback::confirm(crate::locale::t(cx,"feedback.tls.confirm"),crate::locale::t(cx,"feedback.tls.warning"),window,cx,move|_,cx|{let _=view.update(cx,|this,cx|{if this.editing.id==id&&!this.saving{this.editing.tls=TlsMode::Disabled;cx.notify();}});});})))))
            .when(tls_ui::is_local_host(&self.text("host",cx)),|el|el.child("本机地址：可按本地开发需求选择不加密，应用不会自动修改模式。"))
            .child(self.field("ca", "CA 证书路径（可选）"))
            .child(self.field("cert", "客户端证书路径（可选）"))
            .child(self.field("key", "客户端私钥路径（可选）"))
            )
            )
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        Button::new("test-db").icon(IconName::CircleCheck)
                            .label("测试")
                            .disabled(self.saving || running)
                            .on_click(cx.listener(|this, _, _, cx| this.start(true, cx))),
                    )
                    .child(
                        Button::new("connect-db").icon(IconName::ArrowRight)
                            .primary()
                            .label("保存并连接")
                            .disabled(self.saving || running)
                            .on_click(cx.listener(|this, _, window, cx| {this.connect_after_save=true;this.save(window,cx);if !this.saving {this.connect_after_save=false;}})),
                    )
                    .when(running,|el|el.child(
                        Button::new("cancel-db").icon(IconName::Close)
                            .label(if self.states.get(&id).is_some_and(|s|s=="已连接") {"断开"}else{"取消"})
                            .disabled(!running)
                            .on_click(cx.listener(move |this, _, _, cx| {this.disconnect(id,cx);feedback::success(crate::locale::t(cx,"feedback.connection.disconnected"),cx);})),
                    ))
                    .child(
                        Button::new("save-db").icon(IconName::Check)
                            .label("保存")
                            .disabled(self.saving)
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    )
                    .child(self.more_actions(cx)),
            )
            .when_some(self.info.get(&id), |el, info| {
                el.child(Button::new("connection-details").ghost().icon(IconName::Info).label(if self.connection_details {"收起连接详情"}else{"连接详情"})
                    .on_click(cx.listener(|this,_,_,cx|{this.connection_details = !this.connection_details;cx.notify();})))
                    .when(self.connection_details,|el|el.child(div().v_flex().gap_1().text_sm()
                        .child(format!("{} {} · 会话 {} · {} ms",self.editing.kind.label(),info.version,info.session_id,info.elapsed_ms))
                        .child(format!("字符集：{} · 数据库：{}",info.charset,info.database.as_deref().unwrap_or("未选择")))
                        .child(format!("TLS：{}",if info.tls_cipher.is_empty(){"未加密"}else{&info.tls_cipher}))
                        .when_some(info.database_list_error.as_ref(),|el,error|el.child(error.clone()))))
            }).into_any_element()
    }
}

fn required_connection_field(profile: &Profile, discover: bool) -> Option<&'static str> {
    if !discover && profile.name.trim().is_empty() {
        Some("dbwork.connection.name.required")
    } else if profile.kind == DatabaseKind::Sqlite {
        profile
            .sqlite_path
            .trim()
            .is_empty()
            .then_some("dbwork.connection.file.required")
    } else {
        (!discover && profile.database.trim().is_empty())
            .then_some("dbwork.connection.database.required")
    }
}

fn generated_connection_name(profile: &Profile, profiles: &[Profile]) -> String {
    let prefix = if !profile.database.trim().is_empty() {
        profile.database.clone()
    } else if profile.kind == DatabaseKind::Sqlite {
        std::path::Path::new(&profile.sqlite_path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "SQLite".into())
    } else {
        profile.kind.label().to_string()
    };
    let prefix = prefix.chars().take(18).collect::<String>();
    let used = |name: &str| {
        profiles
            .iter()
            .any(|p| p.id != profile.id && p.name.to_lowercase() == name.to_lowercase())
    };
    if !used(&prefix) {
        return prefix;
    }
    let host = if profile.host == "127.0.0.1" || profile.host.eq_ignore_ascii_case("localhost") {
        "local".into()
    } else {
        profile
            .host
            .rsplit('.')
            .next()
            .unwrap_or("db")
            .chars()
            .take(8)
            .collect::<String>()
    };
    let base = format!("{prefix}@{host}");
    if !used(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|name| !used(name))
        .unwrap()
}

#[cfg(test)]
mod connection_form_tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn generated_names_prefer_database_and_resolve_collisions() {
        let mut draft = DatabaseView::empty_profile(DatabaseKind::MySql);
        draft.database = "demo".into();
        draft.host = "127.0.0.1".into();
        let mut saved = draft.clone();
        saved.id = Uuid::new_v4();
        saved.name = "DEMO".into();
        assert_eq!(generated_connection_name(&draft, &[]), "demo");
        assert_eq!(
            generated_connection_name(&draft, std::slice::from_ref(&saved)),
            "demo@local"
        );
        let mut second = saved.clone();
        second.id = Uuid::new_v4();
        second.name = "demo@local".into();
        assert_eq!(
            generated_connection_name(&draft, &[saved, second]),
            "demo@local-2"
        );
    }
    #[::core::prelude::v1::test]
    fn explicit_names_and_database_targets_are_required() {
        for kind in DatabaseKind::ALL {
            let mut profile = DatabaseView::empty_profile(kind);
            assert_eq!(profile.remember_password, kind != DatabaseKind::Sqlite);
            assert!(profile.name.is_empty());
            assert_eq!(
                required_connection_field(&profile, false),
                Some("dbwork.connection.name.required")
            );
            if kind != DatabaseKind::Sqlite {
                profile.database.clear();
                assert_eq!(required_connection_field(&profile, true), None);
            }
            profile.name = "My connection".into();
            if kind == DatabaseKind::Sqlite {
                assert_eq!(
                    required_connection_field(&profile, false),
                    Some("dbwork.connection.file.required")
                );
                profile.sqlite_path = "C:/fixture.db".into();
            } else {
                profile.database.clear();
                assert_eq!(
                    required_connection_field(&profile, false),
                    Some("dbwork.connection.database.required")
                );
                assert_eq!(required_connection_field(&profile, true), None);
                profile.database = if kind == DatabaseKind::Redis {
                    "0"
                } else {
                    "demo"
                }
                .into();
            }
            assert_eq!(required_connection_field(&profile, false), None);
        }
    }
}

#[cfg(test)]
mod new_connection_defaults_tests {
    #[test]
    fn new_drafts_have_requested_defaults_without_changing_legacy_decode() {
        let new = super::DatabaseView::empty_profile(turbodbn_services::DatabaseKind::MySql);
        assert_eq!(new.tls, turbodbn_services::mysql::TlsMode::Disabled);
        assert!(new.allow_old_auth);
        let old: turbodbn_services::mysql::Profile = serde_json::from_str("{}").unwrap();
        assert_eq!(old.tls, turbodbn_services::mysql::TlsMode::VerifyIdentity);
        assert!(!old.allow_old_auth);
    }
}

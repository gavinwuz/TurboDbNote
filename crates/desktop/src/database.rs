#[path = "database_navigation.rs"]
mod navigation;
#[path = "database_objects.rs"]
mod objects_ui;
use navigation::Navigation;
use turbodbn_services::{
    DatabaseKind,
    database::{ObjectItem, Operation, OperationKind, OperationResult},
};
#[path = "database_tls.rs"]
mod tls_ui;
use crate::preferences::Preferences;
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Disableable, IconName, Selectable, StyledExt,
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
use turbodbn_services::{
    credentials,
    mysql::{self, Driver, Event, Profile, ServerInfo, Session, TlsMode},
};
use uuid::Uuid;

pub struct OpenDatabase;
pub struct OpenObjects;
pub struct DatabaseView {
    focus: FocusHandle,
    choosing_type: bool,
    typed_drafts: HashMap<DatabaseKind, (Profile, String)>,
    navigation: Navigation,
    nav_writer: async_channel::Sender<Navigation>,
    db_filters: HashMap<Uuid, Entity<InputState>>,
    db_filter_subscriptions: HashMap<Uuid, Subscription>,
    db_limits: HashMap<Uuid, usize>,
    db_scrolls: HashMap<Uuid, UniformListScrollHandle>,
    selected_database: Option<(Uuid, String)>,
    object_items: Vec<ObjectItem>,
    object_cursor: Option<String>,
    object_details: Vec<(String, String)>,
    selected_object: Option<ObjectItem>,
    detail_scroll: UniformListScrollHandle,
    object_notice: String,
    active_operation: Option<(Uuid, u64, OperationKind)>,
    request_sequence: u64,
    object_scroll: UniformListScrollHandle,
    _nav_task: Task<()>,
    tab: usize,
    connection_details: bool,
    pending_tls: Option<TlsPrompt>,
    drivers: Entity<crate::drivers::DriverManager>,
    connect_after_save: bool,
    profiles: Vec<Profile>,
    editing: Profile,
    baseline: Profile,
    fields: BTreeMap<&'static str, Entity<InputState>>,
    search: Entity<InputState>,
    status: String,
    saving: bool,
    confirm_delete: bool,
    tasks: HashMap<Uuid, Task<()>>,
    sessions: HashMap<Uuid, Session>,
    states: HashMap<Uuid, String>,
    info: HashMap<Uuid, ServerInfo>,
    catalog_source: HashMap<Uuid, Profile>,
    collapsed: std::collections::HashSet<Uuid>,
    generation: HashMap<Uuid, u64>,
    save_task: Option<Task<()>>,
    file: PathBuf,
    temporary_passwords: HashMap<Uuid, String>,
    storage_error: bool,
    _search: Subscription,
    _inputs: Vec<Subscription>,
}
const FIELDS: [(&str, &str); 15] = [
    ("sqlite_path", "选择数据库文件"),
    ("charset", "utf8mb4 / gbk / 自定义字符集"),
    ("legacy", "兼容驱动库绝对路径"),
    ("name", "连接名称"),
    ("group", "分组"),
    ("environment", "环境"),
    ("host", "主机"),
    ("port", "端口"),
    ("user", "用户名"),
    ("password", "密码（可留空）"),
    ("database", "默认数据库（可选）"),
    ("timeout", "连接超时（秒）"),
    ("ca", "CA 证书路径"),
    ("cert", "客户端证书路径"),
    ("key", "客户端私钥路径"),
];
impl DatabaseView {
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
                        if this.editing.id == id && !this.saving && !this.sessions.contains_key(&id)
                        {
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
    pub fn summary(&self) -> String {
        let count = self
            .states
            .values()
            .filter(|s| s.as_str() == "已连接")
            .count();
        format!("数据库连接：{count} 个    ·    内存示例    ·    SQL Notebook")
    }
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let file = Preferences::path()
            .unwrap_or_else(|_| PathBuf::from("connections.json"))
            .with_file_name("connections.json");
        let loaded = mysql::load_profiles(&file);
        let storage_error = loaded.is_err();
        let status = loaded
            .as_ref()
            .err()
            .map(|e| format!("读取连接失败：{e}"))
            .unwrap_or("新建连接后可测试、保存或连接".into());
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder("搜索连接 / 分组 / 数据库"));
        let fields: BTreeMap<&'static str, Entity<InputState>> = FIELDS
            .iter()
            .map(|(key, label)| {
                (
                    *key,
                    cx.new(|cx| {
                        let input = InputState::new(window, cx).placeholder(*label);
                        if *key == "password" {
                            input.masked(true)
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
        let navigation = navigation::load(&nav_path).unwrap_or_default();
        let (nav_writer, nav_errors) = navigation::writer(nav_path);
        let nav_task = cx.spawn(async move |view, cx| {
            while let Ok(error) = nav_errors.recv().await {
                if view
                    .update(cx, |this, cx| {
                        this.status = error;
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        let mut this = Self {
            focus: cx.focus_handle(),
            choosing_type: false,
            typed_drafts: HashMap::new(),
            navigation,
            nav_writer,
            db_filters: HashMap::new(),
            db_filter_subscriptions: HashMap::new(),
            db_limits: HashMap::new(),
            db_scrolls: HashMap::new(),
            selected_database: None,
            object_items: Vec::new(),
            object_cursor: None,
            object_details: Vec::new(),
            selected_object: None,
            detail_scroll: UniformListScrollHandle::new(),
            object_notice: String::new(),
            active_operation: None,
            request_sequence: 0,
            object_scroll: UniformListScrollHandle::new(),
            _nav_task: nav_task,
            tab: 0,
            connection_details: false,
            pending_tls: None,
            drivers: cx.new(crate::drivers::DriverManager::new),
            connect_after_save: false,
            profiles: loaded.unwrap_or_default(),
            editing: Profile::default(),
            baseline: Profile::default(),
            fields,
            search: search.clone(),
            status,
            saving: false,
            confirm_delete: false,
            tasks: HashMap::new(),
            sessions: HashMap::new(),
            states: HashMap::new(),
            info: HashMap::new(),
            catalog_source: HashMap::new(),
            collapsed: std::collections::HashSet::new(),
            generation: HashMap::new(),
            save_task: None,
            file,
            temporary_passwords: HashMap::new(),
            storage_error,
            _search: cx.observe(&search, |_, _, cx| cx.notify()),
            _inputs: inputs,
        };
        this.fill(Profile::default(), window, cx);
        this
    }
    fn text(&self, key: &str, cx: &App) -> String {
        self.fields[key].read(cx).value().trim().to_string()
    }
    fn fill(&mut self, profile: Profile, window: &mut Window, cx: &mut Context<Self>) {
        self.choosing_type = false;
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
        self.confirm_delete = false;
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
            ("password", String::new()),
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
    fn draft(&self, cx: &App) -> Result<Profile, String> {
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
        profile.validate()?;
        Ok(profile)
    }
    fn dirty(&self, cx: &App) -> bool {
        self.draft(cx).ok().as_ref() != Some(&self.baseline)
            || !self.fields["password"].read(cx).value().is_empty()
    }
    fn select(&mut self, id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        if let Some(profile) = self.profiles.iter().find(|p| p.id == id).cloned() {
            // Explicit discard confirmation is inline; selection changes never touch live sessions.
            if self.dirty(cx) {
                self.status = "请先保存当前修改，或点击“取消修改”后选择其他连接".into();
                // The tab may have been closed through the shared tab router; retain and reopen its draft.
                cx.emit(OpenDatabase);
                cx.notify();
                return;
            }
            self.fill(profile, window, cx);
            cx.emit(OpenDatabase);
        }
    }
    fn disconnect(&mut self, id: Uuid, cx: &mut Context<Self>) {
        *self.generation.entry(id).or_default() += 1;
        self.tasks.remove(&id);
        self.sessions.remove(&id);
        if self
            .active_operation
            .as_ref()
            .is_some_and(|(connection, _, _)| *connection == id)
        {
            self.active_operation = None;
            self.object_notice = "操作已取消；如正在建库，请重连确认结果".into();
        }
        self.states.insert(id, "未连接".into());
        self.status = "已断开 / 取消".into();
        cx.notify();
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
        self.pending_tls = None;
        let mut profile = match self.draft(cx) {
            Ok(p) => p,
            Err(e) => {
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
        let mut catalog_source = profile.clone();
        if let Some((mode, _)) = trial {
            profile.tls = mode;
        }
        let attempted_mode = profile.tls;
        catalog_source.database.clear();
        if discover {
            profile.list_all_databases = true;
            if profile.kind != DatabaseKind::PostgreSql {
                profile.database.clear();
            }
        }
        if mysql::uses_legacy(&profile)
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
        self.disconnect(id, cx);
        self.catalog_source.insert(id, catalog_source);
        self.info.remove(&id);
        let generation = self.generation[&id];
        let entered = self.fields["password"].read(cx).value().to_string();
        let password = if entered.is_empty() {
            self.temporary_passwords
                .get(&id)
                .cloned()
                .unwrap_or_default()
        } else {
            entered
        };
        let stored = self
            .profiles
            .iter()
            .any(|p| p.id == id && p.remember_password);
        self.states
            .insert(id, if test { "测试中…" } else { "连接中…" }.into());
        self.status = "正在后台连接…".into();
        let secret = cx.background_executor().spawn(async move {
            if password.is_empty() && stored {
                credentials::read(id)?.ok_or("已保存的凭据不存在，请重新填写密码".into())
            } else {
                Ok(password)
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
                                this.status = error;
                                this.states.insert(id, "凭据错误".into());
                                cx.notify();
                            }
                        });
                        return;
                    }
                };
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
                    let result = view.update(cx, |this, cx| {
                        if this.generation.get(&id) != Some(&generation) {
                            return;
                        }
                        let previous_status = this.status.clone();
                        match event {
                            Event::Connected(info) => {
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
                                this.states.insert(id, "已连接".into());
                                this.info.insert(id, info);
                            }
                            Event::Tested(info) => {
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
                                this.info.insert(id, info);
                                this.sessions.remove(&id);
                                if let Some((mode, remember)) = trial
                                    && this.draft(cx).ok().as_ref() == Some(&snapshot)
                                {
                                    this.pending_tls = Some(TlsPrompt::Success {
                                        profile: snapshot.clone(),
                                        mode,
                                        remember,
                                    });
                                }
                            }
                            Event::Failed(error) => {
                                if this
                                    .active_operation
                                    .as_ref()
                                    .is_some_and(|(connection, _, _)| *connection == id)
                                {
                                    this.active_operation = None;
                                    this.object_notice = error.clone();
                                }
                                if let Some(issue) = mysql::TlsIssue::from_message(&error)
                                    && this.draft(cx).ok().as_ref() == Some(&snapshot)
                                {
                                    this.pending_tls = Some(TlsPrompt::Failure {
                                        profile: snapshot.clone(),
                                        mode: attempted_mode,
                                        issue,
                                    });
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
            self.status =
                "连接配置读取失败，已阻止覆盖原文件；请备份并修复 connections.json 后重启".into();
            cx.notify();
            return;
        }
        if self.saving {
            return;
        }
        let profile = match self.draft(cx) {
            Ok(p) => p,
            Err(e) => {
                self.status = e;
                cx.notify();
                return;
            }
        };
        let id = profile.id;
        let entered = self.fields["password"].read(cx).value().to_string();
        let password = if entered.is_empty() {
            self.temporary_passwords
                .get(&id)
                .cloned()
                .unwrap_or_default()
        } else {
            entered
        };
        let old_saved = self
            .profiles
            .iter()
            .any(|p| p.id == id && p.remember_password);
        let mut profiles = self.profiles.clone();
        if let Some(old) = profiles.iter_mut().find(|p| p.id == id) {
            *old = profile.clone();
        } else {
            profiles.push(profile.clone());
        }
        let file = self.file.clone();
        let payload = profiles.clone();
        let remember = profile.remember_password;
        self.saving = true;
        self.status = "正在保存连接配置…".into();
        let work = cx.background_executor().spawn(async move {
            let old = if old_saved {
                credentials::read(id)?
            } else {
                None
            };
            if remember && old_saved && password.is_empty() && old.is_none() {
                return Err("系统凭据已缺失，请重新填写密码".into());
            }
            if remember && (!password.is_empty() || !old_saved) {
                credentials::write(id, &password)?;
            }
            if let Err(error) = mysql::save_profiles(&file, payload) {
                if remember {
                    if let Some(old) = old {
                        let _ = credentials::write(id, &old);
                    } else {
                        let _ = credentials::delete(id);
                    }
                }
                return Err(error);
            }
            if !remember && old_saved {
                credentials::delete(id)?;
            }
            Ok(if remember {
                String::new()
            } else if password.is_empty() && old_saved {
                old.unwrap_or_default()
            } else {
                password
            })
        });
        self.save_task = Some(cx.spawn_in(window, async move |view, cx| {
            let result = work.await;
            let _ = cx.update(|window, cx| {
                let _ = view.update(cx, |this, cx| {
                    this.saving = false;
                    match result {
                        Ok(password) => {
                            this.profiles = profiles;
                            if profile.remember_password {
                                this.temporary_passwords.remove(&id);
                            } else {
                                this.temporary_passwords.insert(id, password);
                            }
                            this.fill(profile, window, cx);
                            this.status = "连接已保存；密码不写入配置文件".into();
                            if this.connect_after_save {
                                this.connect_after_save = false;
                                this.start(false, cx);
                            }
                        }
                        Err(error) => {
                            this.connect_after_save = false;
                            this.status = format!("保存失败：{error}");
                        }
                    };
                    cx.notify();
                });
            });
        }));
        cx.notify();
    }
    fn delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.editing.id;
        self.disconnect(id, cx);
        self.confirm_delete = false;
        self.saving = true;
        let profiles = self
            .profiles
            .iter()
            .filter(|p| p.id != id)
            .cloned()
            .collect::<Vec<_>>();
        let payload = profiles.clone();
        let path = self.file.clone();
        let work = cx.background_executor().spawn(async move {
            mysql::save_profiles(&path, payload)?;
            credentials::delete(id)
        });
        self.save_task = Some(cx.spawn_in(window, async move |view, cx| {
            let result = work.await;
            let _ = cx.update(|window, cx| {
                let _ = view.update(cx, |this, cx| {
                    this.saving = false;
                    match result {
                        Ok(()) => {
                            this.profiles = profiles;
                            this.temporary_passwords.remove(&id);
                            this.fill(Profile::default(), window, cx);
                            this.status = "连接已删除".into();
                        }
                        Err(error) => this.status = format!("删除失败：{error}"),
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
    fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let profile = self
            .profiles
            .iter()
            .find(|p| p.id == self.editing.id)
            .cloned()
            .unwrap_or_default();
        self.fill(profile, window, cx);
    }
    fn more_actions(&self, cx: &Context<Self>) -> AnyElement {
        let view = cx.entity().downgrade();
        let saved = self.profiles.iter().any(|p| p.id == self.editing.id);
        Button::new("connection-more")
            .icon(IconName::Ellipsis)
            .label("更多")
            .disabled(self.saving)
            .dropdown_menu(move |menu, _, _| {
                let reset = view.clone();
                let copy = view.clone();
                let delete = view.clone();
                menu.item(
                    PopupMenuItem::new("取消修改")
                        .icon(IconName::Undo2)
                        .on_click(move |_, window, cx| {
                            let _ = reset.update(cx, |this, cx| this.reset(window, cx));
                        }),
                )
                .item(
                    PopupMenuItem::new("复制配置（不含密码）")
                        .icon(IconName::Copy)
                        .on_click(move |_, window, cx| {
                            let _ = copy.update(cx, |this, cx| {
                                if let Ok(mut profile) = this.draft(cx) {
                                    profile.id = Uuid::new_v4();
                                    profile.name.push_str(" 副本");
                                    profile.remember_password = false;
                                    this.fill(profile, window, cx);
                                }
                            });
                        }),
                )
                .item(
                    PopupMenuItem::new("删除连接")
                        .icon(IconName::Delete)
                        .disabled(!saved)
                        .on_click(move |_, _, cx| {
                            let _ = delete.update(cx, |this, cx| {
                                this.confirm_delete = true;
                                cx.notify();
                            });
                        }),
                )
            })
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
                    .disabled(self.saving || self.sessions.contains_key(&self.editing.id)),
            )
            .into_any_element()
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
        let view = cx.entity().downgrade();
        let dropdown = Button::new(key)
            .icon(IconName::ChevronDown)
            .tooltip("选择已有值，也可直接输入")
            .disabled(
                self.saving || self.sessions.contains_key(&self.editing.id) || values.is_empty(),
            )
            .dropdown_menu(move |mut menu, _, _| {
                for value in &values {
                    let view = view.clone();
                    let value = value.clone();
                    menu = menu.item(PopupMenuItem::new(value.clone()).on_click(
                        move |_, window, cx| {
                            let _ = view.update(cx, |this, cx| {
                                if this.saving || this.sessions.contains_key(&this.editing.id) {
                                    return;
                                }
                                this.fields[key].update(cx, |input, cx| {
                                    input.set_value(value.clone(), window, cx)
                                });
                                cx.notify();
                            });
                        },
                    ));
                }
                menu.scrollable(true).max_h(px(320.))
            });
        div()
            .v_flex()
            .flex_1()
            .min_w_0()
            .gap_1()
            .text_sm()
            .child(label)
            .child(
                Input::new(&self.fields[key])
                    .disabled(self.saving || self.sessions.contains_key(&self.editing.id))
                    .suffix(dropdown),
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
            .disabled(self.saving || self.sessions.contains_key(&self.editing.id))
            .dropdown_menu(move |mut menu, _, _| {
                for (title, value) in &values {
                    let view = view.clone();
                    let value = *value;
                    menu = menu.item(PopupMenuItem::new(*title).on_click(move |_, window, cx| {
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
            crate::locale::t(cx, "数据库连接"),
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
            .child(self.field("name", "连接名称"))
            .child(
                div()
                    .h_flex()
                    .gap_3()
                    .child(self.combo("group", "分组（可输入新分组）",self.profiles.iter().map(|p|p.group.clone()).collect(),cx))
                    .child(self.combo("environment", "环境（可自定义）",["开发","测试","预发","正式"].into_iter().map(String::from).chain(self.profiles.iter().map(|p|p.environment.clone())).collect(),cx)),
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
            .child(self.field("password", "密码（已有系统凭据时留空沿用）"))
            .child(
                Switch::new("remember-db-password")
                    .label("使用 Windows 凭据管理器保存密码")
                    .checked(self.editing.remember_password)
                    .disabled(self.saving)
                    .on_click(cx.listener(|this, value: &bool, _, cx| {
                        this.editing.remember_password = *value;
                        cx.notify();
                    })),
            )
            )
            .when(self.editing.kind==DatabaseKind::Sqlite,|el|el
                .child(self.field("sqlite_path","数据库文件"))
                .child(div().h_flex().gap_2()
                    .child(Button::new("sqlite-browse").icon(IconName::FolderOpen).label("选择文件").disabled(running||self.saving).on_click(cx.listener(|this,_,window,cx|this.browse_sqlite(false,window,cx))))
                    .child(Button::new("sqlite-create").icon(IconName::Plus).label("新建文件").disabled(running||self.saving).on_click(cx.listener(|this,_,window,cx|this.browse_sqlite(true,window,cx)))))
                .child(Switch::new("sqlite-read-only").label("只读连接").checked(self.editing.sqlite_read_only).disabled(running||self.saving)
                    .on_click(cx.listener(|this,v:&bool,_,cx|{this.editing.sqlite_read_only = *v;cx.notify();}))))
            .when(self.editing.kind==DatabaseKind::Redis,|el|el.child(Switch::new("redis-cluster").label("Redis 集群（仅 DB 0，浏览当前节点）").checked(self.editing.redis_cluster).disabled(running||self.saving)
                .on_click(cx.listener(|this,v:&bool,_,cx|{this.editing.redis_cluster = *v;cx.notify();}))))
            .when(self.editing.kind!=DatabaseKind::Sqlite,|el|el
            .child(div().h_flex().gap_2().items_end()
                .child(self.combo("database","数据库（留空显示全部可见库）",self.draft(cx).ok().map(|p|self.catalog(&p)).unwrap_or_default(),cx))
                .child(Button::new("detect-databases").icon(IconName::Search).label("检测")
                    .tooltip("使用当前连接信息获取账号可见的数据库，不保存草稿")
                    .disabled(self.saving||running).on_click(cx.listener(|this,_,_,cx|this.start_request(true,true,cx)))))
            )
            )
            .when(self.tab==2,|el|el
            .when(self.editing.kind==DatabaseKind::MySql,|el|el
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
            )
            .child(self.combo("charset","字符集（可自定义）",["utf8mb4","utf8","gb2312","gbk","latin1","server-default"].into_iter().map(String::from).collect(),cx))
            .when(mysql::uses_legacy(&self.editing),|el|el
                .child(self.field("legacy","自定义 DLL 路径（留空使用已安装驱动）"))
                .child(Button::new("browse-legacy-driver").label("浏览驱动库").disabled(running||self.saving)
                    .on_click(cx.listener(|this,_,window,cx|this.browse_legacy(window,cx))))
                .child(Switch::new("allow-old-auth").label("允许 pre-4.1 旧密码认证（仅兼容连接）")
                    .checked(self.editing.allow_old_auth).disabled(running||self.saving)
                    .on_click(cx.listener(|this,checked:&bool,_,cx|{this.editing.allow_old_auth= *checked;cx.notify();})))
                .child("兼容驱动运行于独立进程。当前需显式禁用 TLS；4.0 使用服务器默认字符集。请选择可信驱动库。"))
            )
            .child(self.field("timeout", "连接/测试总超时（1–60 秒）"))
            .when(self.editing.kind==DatabaseKind::MySql,|el|el.child(self.drivers.clone()))
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
                    .disabled(running||self.saving||self.editing.tls==TlsMode::Disabled)
                    .on_click(cx.listener(|this,_,_,cx|{this.editing.tls=TlsMode::Disabled;cx.notify();})))))
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
                            .on_click(cx.listener(move |this, _, _, cx| this.disconnect(id, cx))),
                    ))
                    .child(
                        Button::new("save-db").icon(IconName::Check)
                            .label("保存")
                            .disabled(self.saving || running)
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    )
                    .child(self.more_actions(cx)),
            )
            .when(self.confirm_delete, |el| {
                el.child(
                    div()
                        .h_flex()
                        .gap_2()
                        .child("删除连接及其系统凭据？")
                        .child(
                            Button::new("confirm-delete-db").label("确认删除").on_click(
                                cx.listener(|this, _, window, cx| this.delete(window, cx)),
                            ),
                        )
                        .child(Button::new("keep-db").label("取消").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.confirm_delete = false;
                                cx.notify();
                            },
                        ))),
                )
            })
            .child(
                div()
                    .p_3()
                    .rounded_md()
                    .bg(cx.theme().muted)
                    .child(self.status.clone()),
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

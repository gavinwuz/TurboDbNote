use super::*;
use crate::transfer::{
    self, Contents, Control, Endpoint, Event as TransferEvent, Job, Mode, Options, Preview,
    Progress, WriteMode,
};
use gpui_component::{Sizable, scroll::ScrollableElement};
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct Connection {
    pub(crate) profile: Profile,
    baseline: Profile,
    cached: Option<String>,
    credential: Option<Uuid>,
    store: PathBuf,
    pub(crate) generation: u64,
}
impl Connection {
    pub(crate) fn database_label(&self) -> String {
        if self.profile.is_file_database() {
            std::path::Path::new(&self.profile.sqlite_path)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        } else {
            self.profile.database.clone()
        }
    }
    pub(crate) fn endpoint(&self, database: String) -> Result<Endpoint, String> {
        let password = if let Some(value) = &self.cached {
            value.clone()
        } else if let Some(id) = self.credential {
            connection_store::read_password(&self.store, id)?
                .ok_or("保存的凭据已不存在，请重新连接后再执行任务")?
        } else {
            String::new()
        };
        let mut profile = self.profile.resolve_timeouts();
        profile.database = database;
        Ok(Endpoint { profile, password })
    }
}
struct TransferView {
    owner: WeakEntity<DatabaseView>,
    mode: Mode,
    export_flow: bool,
    source_charset: String,
    collation: Entity<InputState>,
    target_databases: Vec<String>,
    source_databases: Vec<String>,
    source_catalog_task: Option<Task<()>>,
    source_catalog_sequence: usize,
    catalog_sequence: usize,
    permissions: transfer::ReadPermissions,
    target_catalog_task: Option<Task<()>>,
    log_id: Uuid,
    log_started: std::time::Instant,
    log_progress: std::time::Instant,
    choices: Vec<Connection>,
    source: usize,
    target: usize,
    source_db: Entity<InputState>,
    target_db: Entity<InputState>,
    file: Entity<InputState>,
    preview: Preview,
    catalog: Vec<transfer::ObjectChoice>,
    catalog_task: Option<Task<()>>,
    catalog_control: Arc<Control>,
    catalog_error: bool,
    catalog_notice: String,
    folded_objects: std::collections::HashSet<String>,
    options: Options,
    batch: Entity<InputState>,
    table_filter: Entity<InputState>,
    table_scroll: ScrollHandle,
    dialog_scroll: ScrollHandle,
    sample_table: Option<String>,
    preview_file: Option<Arc<tempfile::NamedTempFile>>,
    schema: bool,
    data: bool,
    reviewed: Option<String>,
    running: bool,
    previewing: bool,
    progress: Progress,
    result: Option<Result<(), String>>,
    control: Arc<Control>,
    task: Option<Task<()>>,
    subscriptions: Vec<Subscription>,
}
impl Drop for TransferView {
    fn drop(&mut self) {
        self.control.cancel();
        self.catalog_control.cancel();
    }
}
impl DatabaseView {
    pub(crate) fn saved_connection(&self, id: Uuid, cx: &App) -> Result<Connection, String> {
        let p = self
            .profiles
            .iter()
            .find(|p| p.id == id)
            .ok_or("连接已移除，请重新选择已保存连接")?;
        Ok(Connection {
            baseline: p.clone(),
            profile: {
                let mut profile = self
                    .info
                    .get(&id)
                    .map(|info| mysql::detected_profile(p.clone(), &info.version))
                    .unwrap_or_else(|| p.clone());
                if (mysql::uses_legacy(&profile) || profile.driver == mysql::Driver::Auto)
                    && profile.legacy_library.is_empty()
                    && let Some(path) = self.drivers.read(cx).path()
                {
                    profile.legacy_library = path.display().to_string();
                }
                profile
            },
            cached: self
                .temporary_passwords
                .get(&p.id)
                .and_then(|secret| secret.value_for(p)),
            credential: self.history.get(&p.id).and_then(|h| h.credential_id),
            store: self.file.clone(),
            generation: self.generation.get(&p.id).copied().unwrap_or_default(),
        })
    }
    pub(crate) fn connection_snapshot_current(&self, snapshot: &Connection) -> bool {
        self.generation
            .get(&snapshot.profile.id)
            .copied()
            .unwrap_or_default()
            == snapshot.generation
            && self.profiles.iter().any(|p| p == &snapshot.baseline)
    }

    pub(super) fn transfer_dialog(
        &mut self,
        mode: Mode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((id, database)) = self.selected_database.clone() else {
            return;
        };
        let Some(profile) = self.profiles.iter().find(|p| p.id == id) else {
            return;
        };
        if profile.driver_id == "doris"
            || !matches!(
                profile.kind,
                DatabaseKind::MySql | DatabaseKind::PostgreSql | DatabaseKind::Sqlite
            )
        {
            feedback::error(crate::locale::t(cx, "transfer.unsupported"), cx);
            return;
        }
        let choices = self
            .profiles
            .iter()
            .filter(|p| p.kind == profile.kind)
            .filter_map(|p| self.saved_connection(p.id, cx).ok())
            .collect::<Vec<_>>();
        let current = choices.iter().position(|p| p.profile.id == id).unwrap_or(0);
        let owner = cx.entity().downgrade();
        let view =
            cx.new(|cx| TransferView::new(mode, choices, current, database, owner, window, cx));
        let observer = std::rc::Rc::new(cx.observe(&view, |_, _, cx| cx.refresh_windows()));
        window.open_dialog(cx, move |dialog, _, cx| {
            let _keep = &observer;
            let running = view.read(cx).running;
            let buttons = view.clone();
            dialog
                .title(crate::locale::t(cx, "transfer.title"))
                .w(px(920.))
                .overlay_closable(false)
                .close_button(!running)
                .keyboard(!running)
                .child({
                    let scroll = view.read(cx).dialog_scroll.clone();
                    div()
                        .relative()
                        .max_h(px(560.))
                        .child(
                            div()
                                .id("transfer-window-scroll")
                                .max_h(px(560.))
                                .pr_4()
                                .track_scroll(&scroll)
                                .overflow_y_scroll()
                                .child(view.clone()),
                        )
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .right_0()
                                .bottom_0()
                                .w(px(14.))
                                .child(
                                    gpui_component::scroll::Scrollbar::vertical(&scroll)
                                        .scrollbar_show(
                                            gpui_component::scroll::ScrollbarShow::Always,
                                        ),
                                ),
                        )
                })
                .footer(move |_, _, _, cx| {
                    let running = buttons.read(cx).running;
                    let disabled = running
                        || buttons.read(cx).previewing
                        || buttons.read(cx).catalog_task.is_some()
                        || (buttons.read(cx).schema
                            && buttons
                                .read(cx)
                                .permissions
                                .blocks(&buttons.read(cx).options))
                        || buttons.read(cx).result.is_some();
                    let cancel = buttons.clone();
                    let start = buttons.clone();
                    vec![
                        feedback::footer_actions()
                            .child(
                                Button::new("close-transfer")
                                    .icon(IconName::Close)
                                    .label(crate::locale::t(
                                        cx,
                                        if running {
                                            "transfer.cancel"
                                        } else {
                                            "feedback.close"
                                        },
                                    ))
                                    .on_click(move |_, window, cx| {
                                        if cancel.read(cx).running {
                                            cancel.read(cx).control.cancel();
                                            cx.refresh_windows();
                                        } else {
                                            window.close_dialog(cx);
                                        }
                                    }),
                            )
                            .child(
                                Button::new("execute-transfer")
                                    .primary()
                                    .icon(IconName::Check)
                                    .label(crate::locale::t(cx, "transfer.execute"))
                                    .disabled(disabled)
                                    .on_click(move |_, window, cx| {
                                        start.update(cx, |this, cx| this.confirm(window, cx));
                                    }),
                            ),
                    ]
                })
        });
    }
}
impl TransferView {
    fn new(
        mode: Mode,
        choices: Vec<Connection>,
        current: usize,
        database: String,
        owner: WeakEntity<DatabaseView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let source = if mode == Mode::ImportConnection {
            choices
                .iter()
                .enumerate()
                .find(|(index, _)| *index != current)
                .map(|(index, _)| index)
                .unwrap_or(current)
        } else {
            current
        };
        let source_database = if source == current {
            database.clone()
        } else {
            choices[source].database_label()
        };
        let source_kind = choices[source].profile.kind;
        let legacy_source = mysql::uses_legacy(&choices[source].profile);
        let source_db = cx.new(|cx| InputState::new(window, cx).default_value(source_database));
        let target = if mode == Mode::Copy {
            if choices[current].profile.kind == DatabaseKind::Sqlite {
                String::new()
            } else {
                format!("{database}_copy")
            }
        } else {
            database
        };
        let target_db = cx.new(|cx| InputState::new(window, cx).default_value(target));
        let file = cx.new(|cx| InputState::new(window, cx));
        let batch = cx.new(|cx| InputState::new(window, cx).default_value("1000"));
        let table_filter = cx.new(|cx| {
            InputState::new(window, cx).placeholder(crate::locale::t(cx, "transfer.filter_objects"))
        });
        let collation = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(crate::locale::t(cx, "transfer.collation_default"))
        });
        let mut subscriptions = [
            source_db.clone(),
            target_db.clone(),
            file.clone(),
            batch.clone(),
            collation.clone(),
        ]
        .into_iter()
        .map(|input| {
            cx.subscribe(&input, |this, _, event, cx| {
                if matches!(event, gpui_component::input::InputEvent::Change) {
                    this.reviewed = None;
                    this.result = None;
                    cx.notify();
                }
            })
        })
        .collect::<Vec<_>>();
        subscriptions.push(cx.observe(&table_filter, |_, _, cx| cx.notify()));
        subscriptions.push(cx.subscribe(&source_db, |this, _, event, cx| {
            if matches!(event, gpui_component::input::InputEvent::Change) {
                this.options.tables = None;
                this.load_catalog(cx);
            }
        }));
        cx.defer_in(window, |this, _, cx| {
            this.load_catalog(cx);
            this.load_source_databases(cx);
            if this.mode == Mode::ImportConnection
                && this.choices[this.target].profile.kind != DatabaseKind::Sqlite
            {
                this.load_target_databases(cx);
            }
        });
        Self {
            owner,
            mode,
            export_flow: mode == Mode::Export,
            source_charset: String::new(),
            target_databases: Vec::new(),
            source_databases: Vec::new(),
            source_catalog_task: None,
            source_catalog_sequence: 0,
            catalog_sequence: 0,
            permissions: transfer::ReadPermissions::default(),
            target_catalog_task: None,
            collation,
            log_id: Uuid::new_v4(),
            log_started: std::time::Instant::now(),
            log_progress: std::time::Instant::now(),
            choices,
            source,
            target: current,
            source_db,
            target_db,
            file,
            preview: Preview::default(),
            catalog: vec![],
            catalog_task: None,
            catalog_control: Arc::default(),
            catalog_error: false,
            catalog_notice: String::new(),
            folded_objects: Default::default(),
            options: Options {
                views: (mode == Mode::Export && !legacy_source)
                    || source_kind != DatabaseKind::MySql,
                triggers: (mode == Mode::Export && !legacy_source)
                    || source_kind != DatabaseKind::MySql,
                routines: source_kind == DatabaseKind::PostgreSql
                    || (mode == Mode::Export
                        && source_kind == DatabaseKind::MySql
                        && !legacy_source),
                events: mode == Mode::Export
                    && source_kind == DatabaseKind::MySql
                    && !legacy_source,
                ..Default::default()
            },
            batch,
            table_filter,
            table_scroll: ScrollHandle::new(),
            dialog_scroll: ScrollHandle::new(),
            sample_table: None,
            preview_file: None,
            schema: true,
            data: mode != Mode::Copy,
            reviewed: None,
            running: false,
            previewing: false,
            progress: Progress::default(),
            result: None,
            control: Arc::default(),
            task: None,
            subscriptions,
        }
    }
    fn load_catalog(&mut self, cx: &mut Context<Self>) {
        self.catalog_control.cancel();
        self.catalog_sequence += 1;
        let sequence = self.catalog_sequence;
        self.catalog_task = None;
        self.catalog.clear();
        self.catalog_error = false;
        self.catalog_notice.clear();
        self.permissions = transfer::ReadPermissions::default();
        self.options.excluded_objects.clear();
        if self.mode == Mode::ImportFile {
            return;
        }
        let saved = self.choices[self.source].clone();
        let source = self.source;
        let snapshot = saved.clone();
        let database = self.source_db.read(cx).value().trim().to_string();
        if database.is_empty() {
            return;
        }
        self.catalog_control = Arc::default();
        let control = self.catalog_control.clone();
        self.write_log(crate::operation_log::Phase::Preview, None, cx);
        self.catalog_task = Some(cx.spawn(async move |view, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(400))
                .await;
            let requested = database.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    saved.endpoint(requested).and_then(|endpoint| {
                        let (preview, notice) = transfer::catalog(&endpoint, &control)?;
                        let permissions = transfer::read_permissions(&endpoint, &preview, &control);
                        Ok((preview, notice, permissions))
                    })
                })
                .await;
            let _ = view.update(cx, |this, cx| {
                if this.catalog_sequence != sequence {
                    return;
                }
                this.catalog_task = None;
                if this.source != source
                    || this.source_db.read(cx).value().trim() != database
                    || this
                        .owner
                        .upgrade()
                        .is_none_or(|owner| !owner.read(cx).connection_snapshot_current(&snapshot))
                {
                    cx.notify();
                    return;
                }
                match result {
                    Ok((preview, notice, permissions)) => {
                        this.permissions = permissions;
                        // Metadata failures are advisory: keep Execute available and let the
                        // user opt out of unreadable categories; never silently omit objects.
                        this.catalog_notice = notice;
                        this.catalog = preview.objects;
                        this.write_log(crate::operation_log::Phase::Succeeded, None, cx);
                    }
                    Err(error) => {
                        this.catalog_error = true;
                        this.write_log(crate::operation_log::Phase::Failed, Some(&error), cx);
                        feedback::error(error, cx);
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn uses_page_backup(&self, cx: &App) -> bool {
        self.choices[self.source].profile.kind == DatabaseKind::Sqlite
            && self.schema
            && self.data
            && self.options.tables.is_none()
            && self.options.views
            && self.options.triggers
            && self.options.excluded_objects.is_empty()
            && (self.mode != Mode::Export
                || !std::path::Path::new(self.file.read(cx).value().as_str())
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("sql")))
    }
    fn write_log(
        &mut self,
        phase: crate::operation_log::Phase,
        error: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        use crate::operation_log::{Kind, Phase, Record};
        if matches!(phase, Phase::Started | Phase::Preview) {
            self.log_started = std::time::Instant::now();
        }
        let kind = match self.mode {
            Mode::Export => Kind::Export,
            Mode::Copy => Kind::Copy,
            _ => Kind::Import,
        };
        let mut record = Record::new(self.log_id, kind, phase);
        if self.mode != Mode::ImportFile {
            record.source = Some(self.choices[self.source].profile.id);
        }
        if self.mode != Mode::Export {
            record.target = Some(self.choices[self.target].profile.id);
        }
        record.elapsed_ms = self.log_started.elapsed().as_millis() as u64;
        record.rows = self.progress.rows;
        record.context = self.choices[self.source].profile.name.clone();
        record.bytes = self.progress.bytes;
        if let Some(error) = error {
            record.failure = crate::operation_log::failure_class(error);
        }
        if let Some(owner) = self.owner.upgrade()
            && let Some(log) = owner.read(cx).output.clone()
        {
            log.update(cx, |log, cx| log.operation(record, cx));
        }
    }
    fn destination(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        self.mode = mode;
        if mode == Mode::ImportConnection
            && self.choices[self.target].profile.kind != DatabaseKind::Sqlite
        {
            self.load_target_databases(cx);
        }
        self.reviewed = None;
        self.result = None;
        self.schema = mode != Mode::ImportConnection;
        self.source_charset.clear();
        if self.choices[self.source].profile.kind == DatabaseKind::MySql {
            self.options.views =
                mode == Mode::Export && !mysql::uses_legacy(&self.choices[self.source].profile);
            self.options.triggers =
                mode == Mode::Export && !mysql::uses_legacy(&self.choices[self.source].profile);
            self.options.routines =
                mode == Mode::Export && !mysql::uses_legacy(&self.choices[self.source].profile);
            self.options.events =
                mode == Mode::Export && !mysql::uses_legacy(&self.choices[self.source].profile);
        }
        self.data = true;
        self.options.write_mode = if mode == Mode::ImportConnection
            && matches!(
                self.choices[self.target].profile.kind,
                DatabaseKind::MySql | DatabaseKind::Sqlite
            )
            && !mysql::uses_legacy(&self.choices[self.target].profile)
        {
            WriteMode::Upsert
        } else {
            WriteMode::Append
        };
        let value = if mode == Mode::Copy {
            if self.choices[self.source].profile.kind == DatabaseKind::Sqlite {
                String::new()
            } else {
                format!("{}_copy", self.source_db.read(cx).value())
            }
        } else {
            self.choices[self.target].database_label()
        };
        self.target_db
            .update(cx, |input, cx| input.set_value(value, window, cx));
        cx.notify();
    }
    fn signature(&self, cx: &App) -> String {
        serde_json::to_string(&(
            self.mode as u8,
            self.source,
            self.target,
            self.source_db.read(cx).value().as_str(),
            self.target_db.read(cx).value().as_str(),
            self.collation.read(cx).value().as_str(),
            self.file.read(cx).value().as_str(),
            self.schema,
            self.data,
            &self.options,
            self.batch.read(cx).value().as_str(),
        ))
        .unwrap_or_default()
    }
    fn browse_target(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let directory = dirs::document_dir().unwrap_or_else(|| PathBuf::from("."));
        let pick = cx.prompt_for_new_path(&directory, Some("database_copy.db"));
        cx.spawn_in(window, async move |view, cx| {
            if let Ok(Ok(Some(path))) = pick.await {
                let _ = cx.update(|window, cx| {
                    view.update(cx, |this, cx| {
                        this.target_db.update(cx, |input, cx| {
                            input.set_value(path.display().to_string(), window, cx)
                        })
                    })
                });
            }
        })
        .detach();
    }
    fn load_source_databases(&mut self, cx: &mut Context<Self>) {
        self.source_databases.clear();
        self.source_catalog_sequence += 1;
        let sequence = self.source_catalog_sequence;
        if self.choices[self.source].profile.kind == DatabaseKind::Sqlite {
            return;
        }
        let index = self.source;
        let saved = self.choices[index].clone();
        let snapshot = saved.clone();
        self.source_catalog_task = Some(cx.spawn(async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let database = saved.database_label();
                    saved
                        .endpoint(database)
                        .and_then(|endpoint| transfer::database_names(&endpoint))
                })
                .await;
            let _ = view.update(cx, |this, cx| {
                if this.source_catalog_sequence != sequence
                    || this.source != index
                    || this
                        .owner
                        .upgrade()
                        .is_none_or(|owner| !owner.read(cx).connection_snapshot_current(&snapshot))
                {
                    return;
                }
                this.source_catalog_task = None;
                match result {
                    Ok(names) => this.source_databases = names,
                    Err(error) => feedback::error(error, cx),
                }
                cx.notify();
            });
        }));
    }
    fn load_target_databases(&mut self, cx: &mut Context<Self>) {
        self.target_databases.clear();
        let index = self.target;
        let saved = self.choices[index].clone();
        let snapshot = saved.clone();
        let database = saved.database_label();
        self.target_catalog_task = Some(cx.spawn(async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    saved
                        .endpoint(database)
                        .and_then(|endpoint| transfer::database_names(&endpoint))
                })
                .await;
            let _ = view.update(cx, |this, cx| {
                if this.target != index
                    || this
                        .owner
                        .upgrade()
                        .is_none_or(|owner| !owner.read(cx).connection_snapshot_current(&snapshot))
                {
                    return;
                }
                this.target_catalog_task = None;
                match result {
                    Ok(names) => this.target_databases = names,
                    Err(error) => feedback::error(error, cx),
                }
                cx.notify();
            });
        }));
    }
    fn connection_picker(&self, source: bool, cx: &Context<Self>) -> AnyElement {
        let selected = if source { self.source } else { self.target };
        let values = self
            .choices
            .iter()
            .enumerate()
            .map(|(i, p)| (i, p.profile.clone()))
            .collect::<Vec<_>>();
        let view = cx.entity();
        Button::new(if source {
            "transfer-source"
        } else {
            "transfer-target"
        })
        .child(crate::connection_choice::identity(
            &self.choices[selected].profile,
        ))
        .disabled(self.running || self.previewing || (source && self.mode == Mode::Copy))
        .dropdown_menu(move |mut menu, _, _| {
            for (index, profile) in &values {
                let index = *index;
                let view = view.clone();
                menu = menu.item(crate::connection_choice::item(profile).on_click(
                    move |_, window, cx| {
                        view.update(cx, |this, cx| {
                            if source {
                                this.source = index;
                                this.load_source_databases(cx);
                                this.preview = Preview::default();
                                this.options.tables = None;
                                this.sample_table = None;
                                let value = this.choices[index].database_label();
                                this.source_db
                                    .update(cx, |input, cx| input.set_value(value, window, cx));
                            } else {
                                this.target = index;
                                this.target_databases.clear();
                                if this.mode == Mode::ImportConnection
                                    && this.choices[index].profile.kind != DatabaseKind::Sqlite
                                {
                                    this.load_target_databases(cx);
                                }
                                if this.mode != Mode::Copy {
                                    let value = this.choices[index].database_label();
                                    this.target_db
                                        .update(cx, |input, cx| input.set_value(value, window, cx));
                                }
                            }
                            this.reviewed = None;
                            this.result = None;
                            cx.notify();
                        })
                    },
                ));
            }
            menu.scrollable(true).min_w(px(360.)).max_h(px(320.))
        })
        .into_any_element()
    }
    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let export = self.mode == Mode::Export;
        let sqlite = self.choices[self.source].profile.kind == DatabaseKind::Sqlite;
        let pick = if export {
            let ext = if sqlite
                && self.schema
                && self.data
                && self.options.tables.is_none()
                && self.options.views
                && self.options.triggers
            {
                "db"
            } else {
                "sql"
            };
            let raw = self.source_db.read(cx).value();
            let raw = if sqlite {
                std::path::Path::new(raw.as_str())
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            } else {
                raw.to_string()
            };
            let stem = raw
                .chars()
                .map(|c| {
                    if c.is_control() || "<>:\"/\\|?*".contains(c) {
                        '_'
                    } else {
                        c
                    }
                })
                .collect::<String>();
            let stem = if stem.trim_matches([' ', '.']).is_empty() {
                "database"
            } else {
                stem.trim_matches([' ', '.'])
            };
            let name = format!(
                "{stem}_{}.{}",
                chrono::Local::now().format("%Y%m%d_%H%M%S_%3f"),
                ext
            );
            let directory = dirs::document_dir().unwrap_or_else(|| PathBuf::from("."));
            let task = cx.prompt_for_new_path(&directory, Some(&name));
            cx.background_executor()
                .spawn(async move { task.await.ok().and_then(Result::ok).flatten() })
        } else {
            let task = cx.prompt_for_paths(PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some(crate::locale::t(cx, "transfer.file")),
            });
            cx.background_executor().spawn(async move {
                task.await
                    .ok()
                    .and_then(Result::ok)
                    .flatten()
                    .and_then(|paths| paths.into_iter().next())
            })
        };
        cx.spawn_in(window, async move |view, cx| {
            if let Some(path) = pick.await {
                let _ = cx.update(|window, cx| {
                    view.update(cx, |this, cx| {
                        this.file.update(cx, |input, cx| {
                            input.set_value(path.display().to_string(), window, cx)
                        })
                    })
                });
            }
        })
        .detach();
    }
    fn preview(&mut self, cx: &mut Context<Self>) {
        if self.running || self.previewing || self.catalog_task.is_some() {
            return;
        }
        let source = self.choices[self.source].clone();
        let database = self.source_db.read(cx).value().trim().to_string();
        let file = PathBuf::from(self.file.read(cx).value().as_str());
        let mode = self.mode;
        let target = self.choices[self.target].clone();
        let target_database = self.target_db.read(cx).value().trim().to_owned();
        self.write_log(crate::operation_log::Phase::Preview, None, cx);
        let signature = self.signature(cx);
        self.previewing = true;
        let sample = self
            .sample_table
            .clone()
            .filter(|name| {
                self.options
                    .tables
                    .as_ref()
                    .is_none_or(|tables| tables.contains(name))
            })
            .or_else(|| {
                self.options
                    .tables
                    .as_ref()
                    .and_then(|tables| tables.first().cloned())
            });
        self.result = None;
        self.reviewed = None;
        self.control = Arc::default();
        let control = self.control.clone();
        let mut options = self.options.clone();
        options.batch_size = if self.uses_page_backup(cx) {
            1000
        } else {
            self.batch.read(cx).value().parse().unwrap_or(0)
        };
        let contents = Contents {
            schema: self.schema,
            data: self.data,
        };
        let work = cx.background_executor().spawn(async move {
            let source_endpoint = if mode == Mode::ImportFile {
                Endpoint {
                    profile: source.profile.clone(),
                    password: String::new(),
                }
            } else {
                source.endpoint(database.clone())?
            };
            let target_endpoint = if mode == Mode::Export {
                None
            } else {
                Some(target.endpoint(
                    if mode == Mode::Copy && target.profile.kind == DatabaseKind::MySql {
                        String::new()
                    } else if mode == Mode::Copy {
                        target.profile.database.clone()
                    } else {
                        target_database.clone()
                    },
                )?)
            };
            let charset = transfer::preflight(
                &source_endpoint,
                target_endpoint.as_ref(),
                mode,
                &target_database,
                &control,
            )?;
            transfer::validate_options(
                &source_endpoint,
                target_endpoint.as_ref(),
                mode,
                contents,
                &options,
                &control,
            )?;
            let result = if mode == Mode::ImportFile {
                use std::io::Read;
                let snapshot = Arc::new(transfer::freeze_file(&file, &control)?);
                let mut bytes = vec![];
                std::fs::File::open(snapshot.path())
                    .map_err(|_| "无法读取导入文件")?
                    .take(32768)
                    .read_to_end(&mut bytes)
                    .map_err(|_| "无法读取导入文件")?;
                if bytes.starts_with(b"SQLite format 3\0") {
                    if source.profile.kind != DatabaseKind::Sqlite {
                        return Err("SQLite 文件只能恢复到 SQLite 连接".into());
                    }
                    let mut endpoint = source.endpoint(database)?;
                    endpoint.profile.sqlite_path = snapshot.path().display().to_string();
                    transfer::preview_grid(&endpoint, sample.as_deref())
                        .map(|grid| (grid, Some(snapshot)))
                } else {
                    Ok((
                        Preview {
                            table: file
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned(),
                            columns: vec!["#".into(), "SQL".into()],
                            rows: String::from_utf8_lossy(&bytes)
                                .lines()
                                .take(200)
                                .enumerate()
                                .map(|(i, line)| vec![(i + 1).to_string(), line.to_owned()])
                                .collect(),
                            ..Default::default()
                        },
                        Some(snapshot),
                    ))
                }
            } else {
                transfer::preview_grid(&source_endpoint, sample.as_deref()).map(|grid| (grid, None))
            };
            result.map(|(preview, file)| (preview, file, charset))
        });
        self.task = Some(cx.spawn(async move |view, cx| {
            let result = work.await;
            let _ = view.update(cx, |this, cx| {
                this.previewing = false;
                match result {
                    Ok((preview, file, charset)) if this.signature(cx) == signature => {
                        this.source_charset = charset;
                        this.write_log(crate::operation_log::Phase::Succeeded, None, cx);
                        this.preview = preview;
                        this.preview_file = file;
                        this.reviewed = Some(signature);
                    }
                    Ok(_) => {}
                    Err(error) => {
                        this.write_log(crate::operation_log::Phase::Failed, Some(&error), cx);
                        feedback::error(error, cx);
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.running || self.previewing {
            return;
        }
        if self.schema && self.permissions.blocks(&self.options) {
            feedback::error(crate::locale::t(cx, "transfer.permission_selected"), cx);
            return;
        }
        if matches!(self.mode, Mode::Export | Mode::ImportFile)
            && self.file.read(cx).value().trim().is_empty()
        {
            feedback::error(crate::locale::t(cx, "transfer.choose_file"), cx);
            return;
        }
        if self.mode == Mode::Copy && self.target_db.read(cx).value().trim().is_empty() {
            feedback::error(crate::locale::t(cx, "transfer.new_required"), cx);
            return;
        }
        if !self.schema && !self.data {
            feedback::error(crate::locale::t(cx, "transfer.choose_contents"), cx);
            return;
        }
        let batch_value = if self.uses_page_backup(cx) {
            Ok(1000)
        } else {
            self.batch.read(cx).value().parse::<usize>()
        };
        let Ok(batch) = batch_value else {
            feedback::error(crate::locale::t(cx, "transfer.invalid_batch"), cx);
            return;
        };
        if !(1..=10000).contains(&batch) {
            feedback::error(crate::locale::t(cx, "transfer.invalid_batch"), cx);
            return;
        }
        let signature = self.signature(cx);
        let view = cx.entity().downgrade();
        let target = if self.mode == Mode::Export {
            self.file.read(cx).value().to_string()
        } else {
            format!(
                "{} / {}",
                self.choices[self.target].profile.name,
                self.target_db.read(cx).value()
            )
        };
        let target = if self.mode == Mode::Export {
            target
        } else {
            format!(
                "{target} · {}",
                self.choices[self.target].profile.environment
            )
        };
        let source = if self.mode == Mode::ImportFile {
            self.file.read(cx).value().to_string()
        } else {
            format!(
                "{} / {}",
                self.choices[self.source].profile.name,
                self.source_db.read(cx).value()
            )
        };
        let mut body = format!(
            "{}: {source}\n{}: {target}\n{}",
            crate::locale::t(
                cx,
                if self.mode == Mode::ImportFile {
                    "transfer.input_file"
                } else {
                    "transfer.source"
                }
            ),
            crate::locale::t(
                cx,
                if self.mode == Mode::Export {
                    "transfer.output_file"
                } else {
                    "transfer.target"
                }
            ),
            crate::locale::t(
                cx,
                match self.mode {
                    Mode::Export => "transfer.confirm_export",
                    Mode::Copy => "transfer.confirm_copy",
                    Mode::ImportConnection => "transfer.confirm_connection",
                    Mode::ImportFile => "transfer.confirm_body",
                }
            )
        );
        if self.mode != Mode::ImportFile {
            let tables = self
                .options
                .tables
                .as_ref()
                .map(|tables| tables.join(", "))
                .unwrap_or_else(|| crate::locale::t(cx, "transfer.all_tables").to_string());
            body.push('\n');
            body.push_str(&crate::locale::tr(
                cx,
                "transfer.confirm_tables",
                &[("tables", &tables)],
            ));
        }
        if self.mode == Mode::ImportConnection {
            body.push('\n');
            body.push_str(&crate::locale::t(
                cx,
                match self.options.write_mode {
                    WriteMode::Append => "transfer.mode.append",
                    WriteMode::Replace => "transfer.mode.replace_warning",
                    WriteMode::Upsert => "transfer.mode.upsert",
                },
            ));
        }
        feedback::confirm(
            crate::locale::t(cx, "transfer.execute"),
            body,
            window,
            cx,
            move |_, cx| {
                let _ = view.update(cx, |this, cx| {
                    if !this.running && this.signature(cx) == signature {
                        this.start(cx);
                    }
                });
            },
        );
    }
    fn start(&mut self, cx: &mut Context<Self>) {
        let valid = self.owner.upgrade().is_some_and(|owner| {
            let owner = owner.read(cx);
            let indices = match self.mode {
                Mode::Export => vec![self.source],
                Mode::ImportFile => vec![self.target],
                _ => vec![self.source, self.target],
            };
            indices.into_iter().all(|index| {
                let choice = &self.choices[index];
                owner
                    .generation
                    .get(&choice.profile.id)
                    .copied()
                    .unwrap_or_default()
                    == choice.generation
                    && owner.profiles.iter().any(|p| p == &choice.baseline)
            })
        });
        if !valid {
            feedback::error(crate::locale::t(cx, "transfer.connection_changed"), cx);
            return;
        }

        let source = self.choices[self.source].clone();
        let target = self.choices[self.target].clone();
        let source_db = self.source_db.read(cx).value().trim().to_string();
        let target_db = self.target_db.read(cx).value().trim().to_string();
        let frozen = if self.reviewed.as_ref() == Some(&self.signature(cx)) {
            self.preview_file.clone()
        } else {
            None
        };
        let file = if self.mode == Mode::ImportFile {
            frozen
                .as_ref()
                .map(|file| file.path().to_path_buf())
                .unwrap_or_else(|| PathBuf::from(self.file.read(cx).value().as_str()))
        } else {
            PathBuf::from(self.file.read(cx).value().as_str())
        };
        let mode = self.mode;
        let mut options = self.options.clone();
        options.batch_size = self.batch.read(cx).value().parse().unwrap_or(1000);
        options.collation = (self.mode == Mode::Copy
            && self.choices[self.target].profile.kind == DatabaseKind::MySql)
            .then(|| self.collation.read(cx).value().trim().to_string())
            .filter(|s| !s.is_empty());
        let contents = Contents {
            schema: self.schema,
            data: self.data,
        };
        self.control = Arc::default();
        let control = self.control.clone();
        let (tx, rx) = async_channel::bounded(16);
        self.running = true;
        self.write_log(crate::operation_log::Phase::Started, None, cx);
        self.progress = Progress {
            importing: self.mode == Mode::ImportFile,
            ..Default::default()
        };
        self.result = None;
        std::thread::spawn(move || {
            let _keep_file = frozen;
            let result = (|| {
                let source = if mode == Mode::ImportFile {
                    Endpoint {
                        profile: source.profile.clone(),
                        password: String::new(),
                    }
                } else {
                    source.endpoint(source_db)?
                };
                let target = if mode == Mode::Export {
                    None
                } else {
                    Some(target.endpoint(if mode == Mode::Copy {
                        if target.profile.kind == DatabaseKind::MySql {
                            String::new()
                        } else {
                            target.profile.database.clone()
                        }
                    } else {
                        target_db.clone()
                    })?)
                };
                Job {
                    mode,
                    source,
                    target,
                    database: target_db,
                    file,
                    contents,
                    options,
                }
                .run(control.clone(), &tx)
            })();
            let result = if result.is_err() && control.is_cancelled() {
                Err("任务已取消；已提交的目标内容可能保留，请核对结果".into())
            } else {
                result
            };
            let _ = tx.send_blocking(TransferEvent::Done(result));
        });
        self.task = Some(cx.spawn(async move |view, cx| {
            while let Ok(event) = rx.recv().await {
                let done = matches!(&event, TransferEvent::Done(_));
                if view
                    .update(cx, |this, cx| {
                        match event {
                            TransferEvent::Progress(progress) => {
                                this.progress = progress;
                                if this.log_progress.elapsed() >= std::time::Duration::from_secs(1)
                                {
                                    this.write_log(crate::operation_log::Phase::Progress, None, cx);
                                    this.log_progress = std::time::Instant::now();
                                }
                            }
                            TransferEvent::Done(result) => {
                                this.write_log(
                                    if this.control.is_cancelled() {
                                        crate::operation_log::Phase::Cancelled
                                    } else if result.is_ok() {
                                        crate::operation_log::Phase::Succeeded
                                    } else {
                                        crate::operation_log::Phase::Failed
                                    },
                                    result.as_ref().err().map(String::as_str),
                                    cx,
                                );
                                this.running = false;
                                this.preview_file = None;
                                if let Err(error) = &result {
                                    feedback::error(error.clone(), cx);
                                } else {
                                    feedback::success(crate::locale::t(cx, "transfer.done"), cx);
                                }
                                if result.is_ok() && this.mode == Mode::Copy {
                                    let profile = this.choices[this.target].profile.clone();
                                    let name = this.target_db.read(cx).value().trim().to_string();
                                    let _ = this.owner.update(cx, |owner, cx| {
                                        if matches!(
                                            profile.kind,
                                            DatabaseKind::MySql | DatabaseKind::PostgreSql
                                        ) && owner.profiles.iter().any(|p| {
                                            p.id == profile.id
                                                && connection_secret::same_target(p, &profile)
                                        }) && let Some(info) = owner.info.get_mut(&profile.id)
                                            && !info.databases.contains(&name)
                                        {
                                            info.databases.push(name);
                                        }
                                        cx.notify();
                                    });
                                }
                                this.result = Some(result);
                            }
                        }
                        cx.notify();
                    })
                    .is_err()
                    || done
                {
                    break;
                }
            }
        }));
        cx.notify();
    }
}
impl TransferView {
    fn render_progress(&self, cx: &Context<Self>) -> Div {
        let source = if self.mode == Mode::ImportFile {
            self.file.read(cx).value().to_string()
        } else {
            format!(
                "{} / {}",
                self.choices[self.source].profile.name,
                self.source_db.read(cx).value()
            )
        };
        let done = self.result.as_ref().is_some_and(Result::is_ok);
        let percent = if done {
            100.
        } else {
            self.progress
                .total
                .filter(|n| *n > 0)
                .map(|total| 100. * self.progress.completed as f32 / total as f32)
                .unwrap_or(0.)
                .min(99.)
        };
        div()
            .v_flex()
            .gap_3()
            .when(done && self.mode == Mode::Export, |el| {
                el.child(
                    Button::new("export-open-folder")
                        .icon(IconName::FolderOpen)
                        .label(crate::locale::t(cx, "transfer.open_folder"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            crate::feedback::reveal_parent(
                                PathBuf::from(this.file.read(cx).value().as_str()),
                                cx,
                            )
                        })),
                )
            })
            .child(format!(
                "{} → {}",
                source,
                if self.mode == Mode::Export {
                    self.file.read(cx).value().to_string()
                } else {
                    format!(
                        "{} / {}",
                        self.choices[self.target].profile.name,
                        self.target_db.read(cx).value()
                    )
                }
            ))
            .when(self.running, |el| {
                el.child(
                    div()
                        .h_flex()
                        .gap_2()
                        .child(gpui_component::spinner::Spinner::new().small())
                        .child(crate::locale::t(
                            cx,
                            if self.control.is_cancelled() {
                                "transfer.cancelling"
                            } else if self.progress.importing {
                                "transfer.importing"
                            } else {
                                "transfer.exporting"
                            },
                        )),
                )
            })
            .child(gpui_component::progress::Progress::new().value(percent))
            .child(crate::locale::tr(
                cx,
                "transfer.progress",
                &[
                    ("bytes", &self.progress.bytes.to_string()),
                    (
                        "rows",
                        &if matches!(self.progress.unit, transfer::Unit::Tables) {
                            self.progress.rows.to_string()
                        } else {
                            "—".into()
                        },
                    ),
                    ("done", &self.progress.completed.to_string()),
                    (
                        "total",
                        &self
                            .progress
                            .total
                            .map(|n| n.to_string())
                            .unwrap_or_else(|| "—".into()),
                    ),
                    (
                        "unit",
                        crate::locale::t(
                            cx,
                            match self.progress.unit {
                                transfer::Unit::Bytes => "transfer.unit.bytes",
                                transfer::Unit::Tables => "transfer.unit.tables",
                                transfer::Unit::Pages => "transfer.unit.pages",
                                transfer::Unit::Unknown => "transfer.unit.unknown",
                            },
                        )
                        .as_ref(),
                    ),
                ],
            ))
            .when_some(self.result.as_ref(), |el, result| {
                el.child(
                    div()
                        .h_flex()
                        .gap_2()
                        .child(if result.is_ok() {
                            IconName::CircleCheck
                        } else {
                            IconName::Info
                        })
                        .child(match result {
                            Ok(()) => crate::locale::t(
                                cx,
                                if self.mode == Mode::Export {
                                    "transfer.export_done"
                                } else {
                                    "transfer.done"
                                },
                            )
                            .to_string(),
                            Err(_) => crate::locale::t(cx, "transfer.failed").to_string(),
                        }),
                )
            })
    }
}
impl TransferView {
    fn file_field(&self, locked: bool, cx: &Context<Self>) -> Div {
        div()
            .h_flex()
            .gap_2()
            .child(
                Input::new(&self.file)
                    .cleanable(true)
                    .disabled(locked)
                    .flex_1(),
            )
            .child(
                Button::new("transfer-browse")
                    .icon(IconName::FolderOpen)
                    .disabled(locked)
                    .on_click(cx.listener(|this, _, window, cx| this.browse(window, cx))),
            )
    }
    fn render_preview(&self, cx: &Context<Self>) -> Div {
        let sql = self.mode == Mode::ImportFile && self.preview.objects.is_empty();
        let width = if sql { px(680.) } else { px(150.) };
        let mut grid = div().v_flex().gap_1().child(self.preview.table.clone());
        let mut rows = div().v_flex().min_w_full();
        let mut header = div()
            .h_flex()
            .bg(cx.theme().secondary)
            .border_b_1()
            .border_color(cx.theme().border);
        for (index, name) in self.preview.columns.iter().enumerate() {
            header = header.child(
                div()
                    .w(if sql && index == 0 { px(44.) } else { width })
                    .flex_shrink_0()
                    .px_2()
                    .py_1()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(name.clone()),
            );
        }
        rows = rows.child(header);
        for row in &self.preview.rows {
            let mut line = div().h_flex().border_b_1().border_color(cx.theme().border);
            for (index, value) in row.iter().enumerate() {
                line = line.child(
                    div()
                        .w(if sql && index == 0 { px(44.) } else { width })
                        .flex_shrink_0()
                        .px_2()
                        .py_1()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .child(value.clone()),
                );
            }
            rows = rows.child(line);
        }
        grid = grid.child(
            div()
                .id("transfer-preview-grid")
                .max_h(px(190.))
                .border_1()
                .border_color(cx.theme().border)
                .text_sm()
                .child(rows)
                .overflow_scrollbar(),
        );
        grid
    }
}
impl Render for TransferView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _keep = &self.subscriptions;
        if self.running || self.result.is_some() {
            return self.render_progress(cx);
        }
        let locked = self.running || self.previewing;
        let kind = self.choices[self.source].profile.kind;
        let sqlite = kind == DatabaseKind::Sqlite;
        let legacy = mysql::uses_legacy(&self.choices[self.source].profile);
        let file_import = self.mode == Mode::ImportFile;
        let mut left = div().v_flex().gap_2().flex_1().min_w_0();
        if file_import {
            left = left.child(self.file_field(locked, cx));
        } else {
            left = left.child(self.connection_picker(true, cx));
            if self.mode == Mode::Copy || sqlite {
                left = left.child(crate::locale::tr(
                    cx,
                    "transfer.source_database",
                    &[("database", self.source_db.read(cx).value().as_str())],
                ));
                if self.mode == Mode::Copy {
                    let charset = if self.source_charset.is_empty() {
                        crate::locale::t(cx, "transfer.charset_pending").to_string()
                    } else {
                        self.source_charset.clone()
                    };
                    left = left.child(crate::locale::tr(
                        cx,
                        "transfer.source_charset",
                        &[("charset", &charset)],
                    ));
                }
            } else {
                let mut names = self.source_databases.clone();
                if let Some(owner) = self.owner.upgrade()
                    && let Some(info) = owner
                        .read(cx)
                        .info
                        .get(&self.choices[self.source].profile.id)
                {
                    for name in &info.databases {
                        if !names.contains(name) {
                            names.push(name.clone());
                        }
                    }
                }
                let selected = self.source_db.read(cx).value().to_string();
                let search = if names.contains(&selected) {
                    String::new()
                } else {
                    selected.to_lowercase()
                };
                let owner = cx.entity().downgrade();
                left =
                    left.child(crate::locale::t(cx, "transfer.source_database_picker"))
                        .child(
                            div()
                                .h_flex()
                                .gap_2()
                                .child(
                                    Input::new(&self.source_db)
                                        .cleanable(true)
                                        .disabled(locked)
                                        .flex_1(),
                                )
                                .child(
                                    Button::new("source-database-list")
                                        .ghost()
                                        .icon(IconName::ChevronDown)
                                        .disabled(locked)
                                        .dropdown_menu(move |mut menu, _, _| {
                                            for name in names.iter().filter(|name| {
                                                search.is_empty()
                                                    || name.to_lowercase().contains(&search)
                                            }) {
                                                let owner = owner.clone();
                                                let name = name.clone();
                                                menu = menu.item(
                                                    PopupMenuItem::new(name.clone()).on_click(
                                                        move |_, window, cx| {
                                                            let _ = owner.update(cx, |this, cx| {
                                                                this.source_db.update(
                                                                    cx,
                                                                    |input, cx| {
                                                                        input.set_value(
                                                                            name.clone(),
                                                                            window,
                                                                            cx,
                                                                        )
                                                                    },
                                                                )
                                                            });
                                                        },
                                                    ),
                                                );
                                            }
                                            menu.scrollable(true).min_w(px(360.)).max_h(px(320.))
                                        }),
                                )
                                .child(
                                    Button::new("refresh-source-databases")
                                        .ghost()
                                        .icon(crate::assets::refresh_icon())
                                        .disabled(locked || self.source_catalog_task.is_some())
                                        .tooltip(crate::locale::t(cx, "dbwork.refresh"))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.load_source_databases(cx)
                                        })),
                                ),
                        );
            }
        }
        let mut right = div().v_flex().gap_2().flex_1().min_w_0();
        if self.mode == Mode::Export {
            right = right.child(self.file_field(locked, cx));
        } else {
            right = right.child(self.connection_picker(false, cx));
            if self.mode == Mode::Copy || !sqlite {
                right = right
                    .child(crate::locale::t(
                        cx,
                        if self.mode == Mode::Copy {
                            "transfer.new_database"
                        } else {
                            "transfer.database"
                        },
                    ))
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(
                                Input::new(&self.target_db)
                                    .cleanable(true)
                                    .disabled(locked)
                                    .flex_1(),
                            )
                            .when(self.mode == Mode::ImportConnection && !sqlite, |el| {
                                let mut names = self
                                    .owner
                                    .upgrade()
                                    .and_then(|owner| {
                                        owner
                                            .read(cx)
                                            .info
                                            .get(&self.choices[self.target].profile.id)
                                            .map(|info| info.databases.clone())
                                    })
                                    .unwrap_or_default();
                                for name in &self.target_databases {
                                    if !names.contains(name) {
                                        names.push(name.clone());
                                    }
                                }
                                el.child(
                                    Button::new("target-database-list")
                                        .ghost()
                                        .icon(IconName::ChevronDown)
                                        .tooltip(crate::locale::t(cx, "transfer.database"))
                                        .disabled(locked)
                                        .dropdown_menu({
                                            let owner = cx.entity().downgrade();
                                            move |mut menu, _, _| {
                                                for name in &names {
                                                    let owner = owner.clone();
                                                    let name = name.clone();
                                                    menu = menu.item(
                                                        PopupMenuItem::new(name.clone()).on_click(
                                                            move |_, window, cx| {
                                                                let _ =
                                                                    owner.update(cx, |this, cx| {
                                                                        this.target_db.update(
                                                                            cx,
                                                                            |input, cx| {
                                                                                input.set_value(
                                                                                    name.clone(),
                                                                                    window,
                                                                                    cx,
                                                                                )
                                                                            },
                                                                        );
                                                                    });
                                                            },
                                                        ),
                                                    );
                                                }
                                                menu.scrollable(true)
                                                    .min_w(px(360.))
                                                    .max_h(px(320.))
                                            }
                                        }),
                                )
                            })
                            .when(sqlite && self.mode == Mode::Copy, |el| {
                                el.child(
                                    Button::new("new-sqlite-target")
                                        .icon(IconName::FolderOpen)
                                        .disabled(locked)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.browse_target(window, cx)
                                        })),
                                )
                            }),
                    );
            } else {
                right = right.child(self.choices[self.target].database_label());
            }
            if self.mode == Mode::Copy && kind == DatabaseKind::MySql && !legacy {
                right = right
                    .child(crate::locale::t(cx, "transfer.collation"))
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(
                                Input::new(&self.collation)
                                    .cleanable(true)
                                    .disabled(locked)
                                    .flex_1(),
                            )
                            .child(
                                Button::new("collation-presets")
                                    .ghost()
                                    .icon(IconName::ChevronDown)
                                    .disabled(locked)
                                    .dropdown_menu({
                                        let owner = cx.entity().downgrade();
                                        move |mut menu, _, _| {
                                            for value in [
                                                "utf8mb4_general_ci",
                                                "utf8mb4_unicode_ci",
                                                "utf8mb4_bin",
                                                "utf8_general_ci",
                                                "utf8_unicode_ci",
                                                "utf8_bin",
                                            ] {
                                                let owner = owner.clone();
                                                menu =
                                                    menu.item(PopupMenuItem::new(value).on_click(
                                                        move |_, window, cx| {
                                                            let _ = owner.update(cx, |this, cx| {
                                                                this.collation.update(
                                                                    cx,
                                                                    |input, cx| {
                                                                        input.set_value(
                                                                            value, window, cx,
                                                                        )
                                                                    },
                                                                )
                                                            });
                                                        },
                                                    ));
                                            }
                                            menu
                                        }
                                    }),
                            ),
                    );
            }
            if self.mode == Mode::Copy && !sqlite {
                right = right.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(crate::locale::t(cx, "dbwork.copy.charset")),
                );
            }
        }
        // Shared title and control rows keep both endpoints aligned even when their details differ.
        let endpoints = div()
            .v_flex()
            .gap_2()
            .child(
                div()
                    .h_flex()
                    .gap_4()
                    .child(div().flex_1().min_w_0().child(crate::locale::t(
                        cx,
                        if file_import {
                            "transfer.input_file"
                        } else {
                            "transfer.source"
                        },
                    )))
                    .child(div().w_4().flex_shrink_0())
                    .child(div().flex_1().min_w_0().child(crate::locale::t(
                        cx,
                        if self.mode == Mode::Export {
                            "transfer.output_file"
                        } else {
                            "transfer.target"
                        },
                    ))),
            )
            .child(
                div()
                    .h_flex()
                    .items_start()
                    .gap_4()
                    .child(left)
                    .child(
                        div()
                            .w_4()
                            .h(px(28.))
                            .flex_shrink_0()
                            .h_flex()
                            .items_center()
                            .child(IconName::ArrowRight),
                    )
                    .child(right),
            );
        let mut body = div().v_flex().gap_3();
        if self.export_flow {
            let mut destinations = div().h_flex().gap_2();
            for (index, mode, key) in [
                (0usize, Mode::Export, "transfer.destination.file"),
                (1, Mode::Copy, "transfer.destination.new"),
                (2, Mode::ImportConnection, "transfer.destination.existing"),
            ] {
                destinations = destinations.child(
                    Button::new(("transfer-destination", index))
                        .label(crate::locale::t(cx, key))
                        .selected(self.mode == mode)
                        .disabled(locked)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.destination(mode, window, cx)
                        })),
                );
            }
            body = body.child(destinations);
        }
        body = body.child(endpoints);
        if self.mode == Mode::Export {
            body = body.child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(crate::locale::t(cx, "transfer.line_endings"))
                    .child(
                        Button::new("export-line-endings")
                            .label(if self.options.crlf { "CRLF" } else { "LF" })
                            .disabled(locked)
                            .tooltip(crate::locale::t(cx, "transfer.line_endings"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.options.crlf = !this.options.crlf;
                                this.reviewed = None;
                                cx.notify();
                            })),
                    ),
            );
        }
        if !file_import {
            let params = div()
                .h_flex()
                .flex_wrap()
                .gap_3()
                .child(
                    gpui_component::checkbox::Checkbox::new("transfer-schema")
                        .label(crate::locale::t(cx, "transfer.schema"))
                        .checked(self.schema)
                        .disabled(locked || self.mode == Mode::Copy)
                        .on_click(cx.listener(|this, value, _, cx| {
                            this.schema = *value;
                            this.options.write_mode = WriteMode::Append;
                            this.reviewed = None;
                            cx.notify();
                        })),
                )
                .child(
                    gpui_component::checkbox::Checkbox::new("transfer-data")
                        .label(crate::locale::t(cx, "transfer.data"))
                        .checked(self.data)
                        .disabled(locked)
                        .on_click(cx.listener(|this, value, _, cx| {
                            this.data = *value;
                            if !*value {
                                this.options.write_mode = WriteMode::Append;
                            }
                            this.reviewed = None;
                            cx.notify();
                        })),
                );
            body = body.child(params);
            if self.data {
                let mode_label = match self.options.write_mode {
                    WriteMode::Append => "transfer.mode.append",
                    WriteMode::Replace => "transfer.mode.replace",
                    WriteMode::Upsert => "transfer.mode.upsert",
                };
                let view = cx.entity();
                body = body.child(
                    div()
                        .h_flex()
                        .gap_3()
                        .when(self.mode == Mode::ImportConnection, |el| {
                            el.child(
                                Button::new("transfer-write-mode")
                                    .label(crate::locale::t(cx, mode_label))
                                    .dropdown_caret(true)
                                    .disabled(locked || self.schema)
                                    .dropdown_menu(move |mut menu, _, cx| {
                                        for (mode, label) in [
                                            (WriteMode::Append, "transfer.mode.append"),
                                            (WriteMode::Replace, "transfer.mode.replace"),
                                            (WriteMode::Upsert, "transfer.mode.upsert"),
                                        ] {
                                            let view = view.clone();
                                            menu = menu.item(
                                                PopupMenuItem::new(crate::locale::t(cx, label))
                                                    .disabled(
                                                        mode != WriteMode::Append
                                                            && (kind == DatabaseKind::PostgreSql
                                                                || legacy),
                                                    )
                                                    .on_click(move |_, _, cx| {
                                                        view.update(cx, |this, cx| {
                                                            this.options.write_mode = mode;
                                                            this.reviewed = None;
                                                            cx.notify();
                                                        });
                                                    }),
                                            );
                                        }
                                        menu
                                    }),
                            )
                        })
                        .child(crate::locale::t(cx, "transfer.batch"))
                        .child(
                            Input::new(&self.batch)
                                .cleanable(true)
                                .w(px(90.))
                                .disabled(locked || self.uses_page_backup(cx)),
                        ),
                );
            }
            if self.uses_page_backup(cx) {
                body = body.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(crate::locale::t(cx, "transfer.page_backup")),
                );
            }
            body = body.child(Input::new(&self.table_filter).cleanable(true));
            let tables = self
                .catalog
                .iter()
                .filter(|o| o.kind == "table")
                .map(|o| o.name.clone())
                .collect::<Vec<_>>();
            let table_query = self.table_filter.read(cx).value().to_lowercase();
            if !tables.is_empty()
                && !legacy
                && tables
                    .iter()
                    .any(|name| name.to_lowercase().contains(&table_query))
            {
                let all = tables.clone();
                let query = self.table_filter.read(cx).value().to_lowercase();
                let folded = self.folded_objects.contains("table") && query.is_empty();
                let mut list = div().v_flex().pl_16();
                for (index, name) in tables
                    .iter()
                    .enumerate()
                    .filter(|(_, name)| name.to_lowercase().contains(&query))
                {
                    let selected = self
                        .options
                        .tables
                        .as_ref()
                        .is_none_or(|items| items.contains(name));
                    let name = name.clone();
                    let all = all.clone();
                    let sample = name.clone();
                    list = list.child(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(
                                gpui_component::checkbox::Checkbox::new(("transfer-table", index))
                                    .label(name.clone())
                                    .checked(selected)
                                    .disabled(locked)
                                    .on_click(cx.listener(move |this, value: &bool, _, cx| {
                                        let items =
                                            this.options.tables.get_or_insert_with(|| all.clone());
                                        if *value {
                                            if !items.contains(&name) {
                                                items.push(name.clone());
                                            }
                                        } else {
                                            items.retain(|n| n != &name);
                                        }
                                        this.reviewed = None;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new(("sample-table", index))
                                    .ghost()
                                    .icon(IconName::Search)
                                    .tooltip(crate::locale::t(cx, "transfer.preview"))
                                    .disabled(locked)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.sample_table = Some(sample.clone());
                                        this.preview(cx);
                                    })),
                            ),
                    );
                }
                body = body
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(
                                Button::new("fold-transfer-tables")
                                    .ghost()
                                    .icon(if folded {
                                        IconName::ChevronRight
                                    } else {
                                        IconName::ChevronDown
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if !this.folded_objects.remove("table") {
                                            this.folded_objects.insert("table".into());
                                        }
                                        cx.notify();
                                    })),
                            )
                            .child(
                                gpui_component::checkbox::Checkbox::new("all-transfer-tables")
                                    .label(format!(
                                        "{} ({})",
                                        crate::locale::t(cx, "transfer.objects.tables"),
                                        tables.len()
                                    ))
                                    .checked(self.options.tables.is_none())
                                    .disabled(locked)
                                    .on_click(cx.listener(|this, value, _, cx| {
                                        this.options.tables =
                                            if *value { None } else { Some(Vec::new()) };
                                        this.reviewed = None;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .when(!folded, |el| {
                        el.child(
                            div()
                                .relative()
                                .h(px((tables
                                    .iter()
                                    .filter(|name| name.to_lowercase().contains(&query))
                                    .count()
                                    .min(6)
                                    * 28) as f32))
                                .min_h_0()
                                .child(
                                    div()
                                        .id("transfer-tables")
                                        .size_full()
                                        .pr_4()
                                        .track_scroll(&self.table_scroll)
                                        .overflow_y_scroll()
                                        .child(list),
                                )
                                .child(
                                    div()
                                        .absolute()
                                        .top_0()
                                        .right_0()
                                        .bottom_0()
                                        .w(px(14.))
                                        .child(
                                            gpui_component::scroll::Scrollbar::vertical(
                                                &self.table_scroll,
                                            )
                                            .scrollbar_show(
                                                gpui_component::scroll::ScrollbarShow::Always,
                                            ),
                                        ),
                                ),
                        )
                    });
            }
        }
        if !file_import {
            for (group, label) in [
                ("view", "transfer.objects.views"),
                ("trigger", "transfer.objects.triggers"),
                ("routine", "transfer.objects.routines"),
                ("event", "transfer.objects.events"),
            ] {
                let query = self.table_filter.read(cx).value().to_lowercase();
                let names = self
                    .catalog
                    .iter()
                    .filter(|o| {
                        o.kind == group
                            || (group == "routine"
                                && matches!(o.kind.as_str(), "function" | "procedure"))
                    })
                    .map(|o| (o.kind.clone(), o.name.clone()))
                    .collect::<Vec<_>>();
                let total = names.len();
                let names = names
                    .into_iter()
                    .filter(|(_, name)| name.to_lowercase().contains(&query))
                    .collect::<Vec<_>>();
                if names.is_empty() && !query.is_empty() {
                    continue;
                }
                let folded = self.folded_objects.contains(group) && query.is_empty();
                let group_denied = self.permissions.denied(group);
                let checked = match group {
                    "view" => self.options.views,
                    "trigger" => self.options.triggers,
                    "routine" => self.options.routines,
                    _ => self.options.events,
                };
                let supported = sqlite && matches!(group, "view" | "trigger")
                    || kind == DatabaseKind::MySql
                        && !legacy
                        && matches!(
                            self.mode,
                            Mode::Export | Mode::Copy | Mode::ImportConnection
                        );
                body = body.child(
                    div()
                        .h_flex()
                        .gap_2()
                        .child(
                            Button::new(SharedString::from(format!("object-fold-{group}")))
                                .ghost()
                                .icon(if folded {
                                    IconName::ChevronRight
                                } else {
                                    IconName::ChevronDown
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if !this.folded_objects.remove(group) {
                                        this.folded_objects.insert(group.into());
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(
                            gpui_component::checkbox::Checkbox::new(SharedString::from(format!(
                                "object-group-{group}"
                            )))
                            .label(format!("{} ({})", crate::locale::t(cx, label), total))
                            .checked(checked && self.schema)
                            .disabled(
                                locked
                                    || !supported
                                    || group_denied
                                    || !self.schema
                                    || (group == "view" && self.options.tables.is_some()),
                            )
                            .on_click(cx.listener(
                                move |this, value, _, cx| {
                                    match group {
                                        "view" => this.options.views = *value,
                                        "trigger" => this.options.triggers = *value,
                                        "routine" => this.options.routines = *value,
                                        _ => this.options.events = *value,
                                    };
                                    this.reviewed = None;
                                    cx.notify();
                                },
                            )),
                        ),
                );
                if !folded {
                    let individual = sqlite || (kind == DatabaseKind::MySql && group == "view");
                    let checked = checked
                        && self.schema
                        && !(group == "view" && self.options.tables.is_some());
                    let exclusions = self.options.excluded_objects.clone();
                    let count = names.len();
                    body = body.child(
                        uniform_list(
                            SharedString::from(format!("object-list-{group}")),
                            count,
                            cx.processor(move |_, range: std::ops::Range<usize>, _, cx| {
                                range
                                    .map(|index| {
                                        let object = names[index].clone();
                                        let selected = checked && !exclusions.contains(&object);
                                        gpui_component::checkbox::Checkbox::new(SharedString::from(
                                            format!("export-object-{group}-{index}"),
                                        ))
                                        .label(object.1.clone())
                                        .tooltip(move |window, cx| {
                                            gpui_component::tooltip::Tooltip::new(crate::locale::t(
                                                cx,
                                                if group_denied {
                                                    "transfer.permission_denied"
                                                } else if !individual {
                                                    "transfer.type_selection_only"
                                                } else {
                                                    "transfer.permission_advisory"
                                                },
                                            ))
                                            .build(window, cx)
                                        })
                                        .checked(selected)
                                        .disabled(
                                            locked
                                                || !individual
                                                || !checked
                                                || !supported
                                                || group_denied,
                                        )
                                        .on_click(cx.listener(move |this, value: &bool, _, cx| {
                                            this.options.excluded_objects.retain(|o| o != &object);
                                            if !*value {
                                                this.options.excluded_objects.push(object.clone());
                                            }
                                            this.reviewed = None;
                                            cx.notify();
                                        }))
                                        .into_any_element()
                                    })
                                    .collect()
                            }),
                        )
                        .h(px((count.min(4) * 28) as f32))
                        .ml_16(),
                    );
                }
            }
            if self
                .permissions
                .0
                .values()
                .any(|p| *p == transfer::ReadPermission::Denied)
            {
                let types = [
                    ("view", "transfer.objects.views"),
                    ("trigger", "transfer.objects.triggers"),
                    ("routine", "transfer.objects.routines"),
                    ("event", "transfer.objects.events"),
                ]
                .into_iter()
                .filter(|(kind, _)| self.permissions.denied(kind))
                .map(|(_, key)| crate::locale::t(cx, key).to_string())
                .collect::<Vec<_>>()
                .join(", ");
                body =
                    body.child(div().text_sm().text_color(cx.theme().warning).child(
                        crate::locale::tr(
                            cx,
                            "transfer.permission_denied_types",
                            &[("types", &types)],
                        ),
                    ))
                    .child(
                        Button::new("remove-denied-transfer-objects")
                            .icon(IconName::Close)
                            .label(crate::locale::t(cx, "transfer.permission_remove"))
                            .disabled(locked)
                            .on_click(cx.listener(|this, _, _, cx| {
                                for kind in ["view", "trigger", "routine", "event"] {
                                    if this.permissions.denied(kind) {
                                        match kind {
                                            "view" => this.options.views = false,
                                            "trigger" => this.options.triggers = false,
                                            "routine" => this.options.routines = false,
                                            _ => this.options.events = false,
                                        }
                                    }
                                }
                                this.reviewed = None;
                                cx.notify();
                            })),
                    );
            }
            if self
                .permissions
                .0
                .values()
                .any(|permission| *permission == transfer::ReadPermission::Unknown)
            {
                body = body.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(crate::locale::t(cx, "transfer.permission_unknown")),
                );
            }
            if !self.permissions.0.is_empty() {
                body = body.child(
                    Button::new("recheck-transfer-permissions")
                        .ghost()
                        .icon(crate::assets::refresh_icon())
                        .label(crate::locale::t(cx, "transfer.permission_recheck"))
                        .disabled(locked || self.catalog_task.is_some())
                        .on_click(cx.listener(|this, _, _, cx| this.load_catalog(cx))),
                );
            }
            if !self.catalog_notice.is_empty() {
                body = body.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().warning)
                        .child(self.catalog_notice.clone()),
                );
            }
            if self.catalog_task.is_some() {
                body = body.child(crate::locale::t(cx, "transfer.catalog_loading"));
            }
            if self.catalog_error {
                body = body.child(
                    Button::new("reload-transfer-catalog")
                        .icon(crate::assets::refresh_icon())
                        .label(crate::locale::t(cx, "transfer.catalog_retry"))
                        .on_click(cx.listener(|this, _, _, cx| this.load_catalog(cx))),
                );
            }
        }
        if self.mode == Mode::Export
            && self.schema
            && (!sqlite
                || std::path::Path::new(self.file.read(cx).value().as_str())
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("sql")))
        {
            body = body.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(crate::locale::t(cx, "transfer.create_if_missing")),
            );
        }
        body = body
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(crate::locale::t(
                        cx,
                        if file_import {
                            "transfer.file_scope"
                        } else {
                            "transfer.scope"
                        },
                    )),
            )
            .when(file_import, |el| {
                el.child(
                    Button::new("preview-input-file")
                        .icon(IconName::Search)
                        .label(crate::locale::t(cx, "transfer.preview"))
                        .disabled(locked)
                        .on_click(cx.listener(|this, _, _, cx| this.preview(cx))),
                )
            })
            .when(!self.preview.columns.is_empty(), |el| {
                el.child(self.render_preview(cx))
            });
        if self.previewing {
            body = body.child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(gpui_component::spinner::Spinner::new().small())
                    .child(crate::locale::t(cx, "transfer.previewing")),
            );
        }
        body
    }
}

use crate::{
    database::{DatabaseView, SavedConnection},
    feedback,
    locale::{t, tr},
};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Disableable, IconName, Selectable, Sizable, StyledExt, WindowExt,
    button::{Button, ButtonVariants},
    scroll::{Scrollbar, ScrollbarShow},
    text::TextView,
};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};
use turbodbn_core::notebook::SqlScript;
use turbodbn_services::{
    database::{
        Operation, OperationKind, OperationResult,
        notebook_query::Plan,
        workbench::{Action, Reply, Request},
    },
    mysql::{Event, Session},
    sql_script::{self, Entry, Reader},
};

pub enum ScriptEvent {
    Run(bool),
    Reindexed(SqlScript),
    SelectTarget,
    PermissionBlocked(crate::note_permissions::Advice),
    Finished { run: u64, success: bool },
}
enum PreflightError {
    Other(String),
    NoteAccess(crate::note_permissions::Advice),
    Permissions(Vec<sql_script::PermissionBlock>),
}
impl From<String> for PreflightError {
    fn from(error: String) -> Self {
        Self::Other(error)
    }
}
impl From<&str> for PreflightError {
    fn from(error: &str) -> Self {
        Self::Other(error.into())
    }
}
pub struct ScriptView {
    pub path: PathBuf,
    pub script: SqlScript,
    connections: Entity<DatabaseView>,
    rows: Vec<Entry>,
    page: u64,
    issues_only: bool,
    select_last_issue: bool,
    list_scroll: UniformListScrollHandle,
    selected: u64,
    preview: SharedString,
    preview_markdown: SharedString,
    preview_comments: SharedString,
    preview_sql: SharedString,
    preview_range: SharedString,
    show_comments: bool,
    status: SharedString,
    page_epoch: u64,
    page_task: Option<Task<()>>,
    preview_epoch: u64,
    preview_task: Option<Task<()>>,
    pub active: bool,
    locked: bool,
    access_mode: turbodbn_services::access_mode::AccessMode,
    run_id: u64,
    next: u64,
    end: u64,
    completed: u64,
    frozen: Option<Arc<Mutex<Reader>>>,
    binding: Option<(SavedConnection, String)>,
    session: Option<Session>,
    task: Option<Task<()>>,
    risk_task: Option<Task<()>>,
    cancel: Arc<AtomicBool>,
    operation: u64,
    pending_request: Option<(bool, [u8; 32])>,
    diagnostic_task: Option<Task<()>>,
    diagnostic_running: bool,
    diagnostic_job: Option<(uuid::Uuid, Instant)>,
    executing: bool,
    current_sql: String,
    current_location: Option<(u64, u64)>,
    failed_location: Option<(u64, u64)>,
    job: uuid::Uuid,
    started: Instant,
    last_progress: Instant,
    indexing: bool,
    index_error: bool,
    authorized: bool,
    authorize_next: u64,
    preparing_authorization: bool,
    refreshing_guards: bool,
    write_plan: Option<Plan>,
    expected_guards: std::collections::HashMap<(String, String), [u8; 32]>,
    server_legacy: bool,
    authorization_summary: Option<sql_script::AuthorizationSummary>,
    results: Entity<crate::script_results::ScriptResults>,
    _connections_observe: Subscription,
}
impl EventEmitter<ScriptEvent> for ScriptView {}
impl ScriptView {
    pub fn set_access_mode(
        &mut self,
        mode: turbodbn_services::access_mode::AccessMode,
        cx: &mut Context<Self>,
    ) {
        self.access_mode = mode;
        cx.notify();
    }
    pub fn scope_changed(&mut self, cx: &mut Context<Self>) {
        self.stop_diagnostics(cx);
        if self.active {
            self.stop(cx);
        }
        self.binding = None;
        self.failed_location = None;
        self.results.update(cx, |results, cx| results.clear(cx));
        // File indexing is independent of the effective connection and must survive defaults.
        if !self.indexing {
            self.status = "".into();
        }
        cx.notify();
    }
    pub fn copy_path(&self, cx: &mut Context<Self>) {
        if let Some(parent) = self.path.parent() {
            let path = parent
                .join(".turbodbnote-scripts")
                .join(format!("{}.sql", self.script.source_hash));
            cx.write_to_clipboard(ClipboardItem::new_string(
                path.to_string_lossy().into_owned(),
            ));
            feedback::success(t(cx, "feedback.copied"), cx);
        }
    }
    pub fn is_indexing(&self) -> bool {
        self.indexing
    }
    pub fn generation(&self) -> u64 {
        self.run_id
    }
    pub fn new(
        path: PathBuf,
        script: SqlScript,
        connections: Entity<DatabaseView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let results =
            cx.new(|cx| crate::script_results::ScriptResults::new(connections.clone(), window, cx));
        let observed = cx.observe(&connections, |this, _, cx| {
            if this.diagnostic_running
                && this.binding.as_ref().is_none_or(|(saved, _)| {
                    !this.connections.read(cx).connection_snapshot_current(saved)
                })
            {
                this.stop_diagnostics(cx);
            }
            if this.active && !this.current(this.run_id, cx) {
                this.fail("notes.preview_changed".into(), cx);
            }
        });
        cx.defer_in(window, |this, _, cx| this.refresh_index(cx));
        Self {
            path,
            script,
            connections,
            rows: vec![],
            page: 0,
            issues_only: false,
            select_last_issue: false,
            list_scroll: UniformListScrollHandle::new(),
            selected: 0,
            preview: "".into(),
            preview_markdown: "".into(),
            preview_comments: "".into(),
            preview_sql: "".into(),
            preview_range: "".into(),
            show_comments: false,
            status: "".into(),
            page_epoch: 0,
            page_task: None,
            preview_epoch: 0,
            preview_task: None,
            active: false,
            locked: false,
            access_mode: Default::default(),
            run_id: 0,
            next: 0,
            end: 0,
            completed: 0,
            frozen: None,
            binding: None,
            session: None,
            task: None,
            risk_task: None,
            cancel: Arc::new(AtomicBool::new(false)),
            operation: 0,
            pending_request: None,
            diagnostic_task: None,
            diagnostic_running: false,
            diagnostic_job: None,
            executing: false,
            current_sql: String::new(),
            current_location: None,
            failed_location: None,
            job: uuid::Uuid::new_v4(),
            started: Instant::now(),
            last_progress: Instant::now(),
            indexing: true,
            index_error: false,
            authorized: false,
            authorize_next: 0,
            preparing_authorization: false,
            refreshing_guards: false,
            write_plan: None,
            expected_guards: std::collections::HashMap::new(),
            server_legacy: false,
            authorization_summary: None,
            results,
            _connections_observe: observed,
        }
    }
    fn refresh_index(&mut self, cx: &mut Context<Self>) {
        self.indexing = true;
        self.index_error = false;
        let path = self.path.clone();
        let snapshot = path.clone();
        let script = self.script.clone();
        let cancel = self.cancel.clone();
        self.status = t(cx, "script.reindexing");
        let work = cx
            .background_executor()
            .spawn(async move { sql_script::refresh(&path, &script, &cancel) });
        self.page_task = Some(cx.spawn(async move |owner, cx| {
            let result = work.await;
            let _ = owner.update(cx, |this, cx| {
                if this.path != snapshot {
                    this.refresh_index(cx);
                    return;
                }
                this.indexing = false;
                match result {
                    Ok(script) => {
                        if script != this.script {
                            this.script = script.clone();
                            cx.emit(ScriptEvent::Reindexed(script));
                        }
                        this.status = "".into();
                        this.load_page(0, cx);
                    }
                    Err(error) => {
                        this.index_error = true;
                        this.status = crate::locale::message(cx, &error);
                    }
                }
                cx.notify();
            });
        }));
    }
    pub fn set_locked(&mut self, locked: bool, cx: &mut Context<Self>) {
        if self.locked != locked {
            self.locked = locked;
            cx.notify();
        }
    }
    fn load_page(&mut self, page: u64, cx: &mut Context<Self>) {
        self.page = page;
        self.page_epoch += 1;
        let epoch = self.page_epoch;
        let path = self.path.clone();
        let script = self.script.clone();
        let issues = self.issues_only;
        let last = self.select_last_issue;
        self.select_last_issue = false;
        self.preview_epoch += 1;
        self.list_scroll = UniformListScrollHandle::new();
        let work = cx.background_executor().spawn(async move {
            if issues {
                sql_script::issue_page(&path, &script, page)
            } else {
                sql_script::page(&path, &script, page)
            }
        });
        self.page_task = Some(cx.spawn(async move |owner, cx| {
            let result = work.await;
            let _ = owner.update(cx, |this, cx| {
                if this.page_epoch != epoch {
                    return;
                }
                match result {
                    Ok(rows) => {
                        this.rows = rows;
                        if last && let Some(row) = this.rows.last() {
                            this.selected = row.number;
                        }
                        if !this.rows.iter().any(|row| row.number == this.selected)
                            && let Some(first) = this.rows.first()
                        {
                            this.selected = first.number;
                            this.preview = "".into();
                            this.preview_markdown = "".into();
                        }
                        if issues && !this.rows.is_empty() {
                            this.select(this.selected, cx);
                        }
                    }
                    Err(error) => this.status = crate::locale::message(cx, &error),
                }
                cx.notify();
            });
        }));
    }
    fn toggle_issues(&mut self, cx: &mut Context<Self>) {
        self.issues_only = !self.issues_only;
        let page = if self.issues_only {
            0
        } else {
            self.selected / sql_script::PAGE_SIZE * sql_script::PAGE_SIZE
        };
        self.load_page(page, cx);
    }
    fn step_issue(&mut self, next: bool, cx: &mut Context<Self>) {
        let index = self
            .rows
            .iter()
            .position(|row| row.number == self.selected)
            .unwrap_or(0);
        if let Some(row) = if next {
            self.rows.get(index + 1)
        } else {
            index.checked_sub(1).and_then(|i| self.rows.get(i))
        } {
            self.select(row.number, cx);
        } else if next && self.page + sql_script::PAGE_SIZE < self.script.unsupported {
            self.load_page(self.page + sql_script::PAGE_SIZE, cx);
        } else if !next && self.page > 0 {
            self.select_last_issue = true;
            self.load_page(self.page - sql_script::PAGE_SIZE, cx);
        }
    }
    fn select(&mut self, number: u64, cx: &mut Context<Self>) {
        self.selected = number;
        self.preview_epoch += 1;
        let epoch = self.preview_epoch;
        let path = self.path.clone();
        let script = self.script.clone();
        self.preview = "".into();
        self.preview_markdown = "".into();
        self.preview_sql = "".into();
        self.preview_comments = "".into();
        self.preview_range = "".into();
        self.show_comments = false;
        let work = cx
            .background_executor()
            .spawn(async move { sql_script::preview(&path, &script, number) });
        self.preview_task = Some(cx.spawn(async move |owner, cx| {
            let result = work.await;
            let _ = owner.update(cx, |this, cx| {
                if this.preview_epoch != epoch {
                    return;
                }
                match result {
                    Ok(sql) => {
                        this.set_preview(sql, cx);
                    }
                    Err(error) => this.status = crate::locale::message(cx, &error),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn set_preview(&mut self, source: String, cx: &App) {
        let line = self
            .rows
            .iter()
            .find(|r| r.number == self.selected)
            .map(|r| r.line)
            .or_else(|| self.current_location.map(|(_, line)| line))
            .unwrap_or(1);
        let parts = sql_script::statement_preview(source, line, self.script.dialect);
        self.preview_markdown =
            crate::cell::sql_reading(&parts.sql, sql_script::kind(self.script.dialect)).into();
        self.preview_range = tr(
            cx,
            "script.source_range",
            &[
                ("sql_line", &parts.sql_line.to_string()),
                ("start", &parts.source_line.to_string()),
                ("end", &parts.end_line.to_string()),
            ],
        );
        self.preview_comments = parts.comments.into();
        self.preview_sql = parts.sql.into();
        self.preview = parts.source.into();
    }
    fn diagnostic_log(
        &self,
        job: uuid::Uuid,
        phase: crate::operation_log::Phase,
        elapsed: u64,
        error: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        if let Some(output) = self.connections.read(cx).output.clone() {
            let mut record =
                crate::operation_log::Record::new(job, crate::operation_log::Kind::Query, phase);
            record.source = self.binding.as_ref().map(|(saved, _)| saved.profile.id);
            record.elapsed_ms = elapsed;
            if let Some(error) = error {
                record.failure = crate::operation_log::failure_class(error);
            }
            output.update(cx, |output, cx| output.operation(record, cx));
        }
    }
    fn stop_diagnostics(&mut self, cx: &mut Context<Self>) {
        if let Some((job, start)) = self.diagnostic_job.take() {
            self.diagnostic_log(
                job,
                crate::operation_log::Phase::Cancelled,
                start.elapsed().as_millis() as u64,
                None,
                cx,
            );
        }
        self.diagnostic_task = None;
        self.diagnostic_running = false;
    }
    fn diagnose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active || self.diagnostic_running {
            return;
        }
        let Some((saved, database)) = self.binding.clone() else {
            return;
        };
        if !self
            .connections
            .read(cx)
            .connection_snapshot_current(&saved)
        {
            feedback::error(t(cx, "notes.preview_changed"), cx);
            return;
        }
        let original = saved.clone();
        let target = format!("{} / {}", saved.profile.name, database);
        let expected_database = database.clone();
        let run = self.run_id;
        let job = uuid::Uuid::new_v4();
        let start = Instant::now();
        self.diagnostic_job = Some((job, start));
        self.diagnostic_running = true;
        self.diagnostic_log(job, crate::operation_log::Phase::Started, 0, None, cx);
        self.diagnostic_task = Some(cx.spawn_in(window, async move |owner, cx| {
            let endpoint = cx
                .background_executor()
                .spawn(async move { saved.endpoint(database) })
                .await;
            let result = async {
                let endpoint = endpoint?;
                let (session, events) = Session::start(endpoint.profile, endpoint.password, false);
                while let Ok(event) = events.recv().await {
                    match event {
                        Event::Connected(info) => {
                            if info.database.as_deref() != Some(expected_database.as_str()) {
                                return Err("script.error.request_mismatch".into());
                            }
                            session
                                .request(Operation {
                                    id: 1,
                                    kind: OperationKind::Workbench(Box::new(Request {
                                        database: expected_database.clone(),
                                        action: Action::ScriptDiagnostics,
                                    })),
                                })
                                .map_err(|_| "script.error.execution")?;
                        }
                        Event::Operation { id: 1, result } => {
                            return match result {
                                Ok(OperationResult::Workbench(reply)) => match *reply {
                                    Reply::ScriptDiagnostics(report) => Ok(report),
                                    _ => Err("script.error.request_mismatch".into()),
                                },
                                Err(error) => Err(error),
                                _ => Err("script.error.request_mismatch".into()),
                            };
                        }
                        Event::Failed(error) => return Err(error),
                        Event::Disconnected => return Err("script.error.execution".into()),
                        _ => {}
                    }
                }
                Err("script.error.execution".into())
            }
            .await;
            let _ = owner.update_in(cx, |this, window, cx| {
                this.diagnostic_job = None;
                this.diagnostic_running = false;
                this.diagnostic_log(
                    job,
                    if result.is_ok() {
                        crate::operation_log::Phase::Succeeded
                    } else {
                        crate::operation_log::Phase::Failed
                    },
                    start.elapsed().as_millis() as u64,
                    result.as_ref().err().map(String::as_str),
                    cx,
                );
                if this.run_id != run
                    || !this
                        .connections
                        .read(cx)
                        .connection_snapshot_current(&original)
                {
                    cx.notify();
                    return;
                }
                match result {
                    Err(error) => feedback::error(crate::locale::message(cx, &error), cx),
                    Ok(report) => {
                        let grants = if report.grants_available {
                            if report.direct_privileges.is_empty() {
                                t(cx, "script.diagnostics.not_seen").to_string()
                            } else {
                                report.direct_privileges.join(", ")
                            }
                        } else {
                            t(cx, "script.diagnostics.grants_unavailable").to_string()
                        };
                        let text = tr(
                            cx,
                            "script.diagnostics.report",
                            &[
                                ("version", &report.version),
                                ("account", &report.account),
                                ("database", &report.database),
                                (
                                    "binlog",
                                    if report.binary_log.is_empty() {
                                        "?"
                                    } else {
                                        &report.binary_log
                                    },
                                ),
                                (
                                    "trust",
                                    if report.trust_creators.is_empty() {
                                        "?"
                                    } else {
                                        &report.trust_creators
                                    },
                                ),
                                ("grants", &grants),
                            ],
                        );
                        window.open_dialog(cx, move |dialog, _, cx| {
                            dialog
                                .title(t(cx, "script.diagnostics.title"))
                                .w(px(700.))
                                .child(
                                    div()
                                        .v_flex()
                                        .gap_3()
                                        .child(target.clone())
                                        .child(div().whitespace_normal().child(text.clone()))
                                        .child(t(cx, "script.diagnostics.hint"))
                                        .child(
                                            feedback::actions()
                                                .child(feedback::close_button(cx))
                                                .child(
                                                    Button::new("copy-script-diagnostics")
                                                        .icon(IconName::Copy)
                                                        .label(t(cx, "script.diagnostics.copy"))
                                                        .on_click({
                                                            let text = text.clone();
                                                            move |_, _, cx| {
                                                                cx.write_to_clipboard(
                                                                    ClipboardItem::new_string(
                                                                        text.to_string(),
                                                                    ),
                                                                );
                                                                feedback::success(
                                                                    t(cx, "feedback.copied"),
                                                                    cx,
                                                                );
                                                            }
                                                        }),
                                                ),
                                        ),
                                )
                        });
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn log(&self, phase: crate::operation_log::Phase, error: Option<&str>, cx: &mut Context<Self>) {
        if let Some(output) = self.connections.read(cx).output.clone() {
            let mut record = crate::operation_log::Record::new(
                self.job,
                crate::operation_log::Kind::Query,
                phase,
            );
            record.elapsed_ms = self.started.elapsed().as_millis() as u64;
            record.rows = self.completed;
            record.source = self.binding.as_ref().map(|(saved, _)| saved.profile.id);
            if let Some(error) = error {
                record.failure = crate::operation_log::failure_class(error);
            }
            output.update(cx, |log, cx| log.operation(record, cx));
        }
    }
    fn current(&self, run: u64, cx: &App) -> bool {
        self.active
            && self.run_id == run
            && self.binding.as_ref().is_some_and(|(saved, _)| {
                self.connections.read(cx).connection_snapshot_current(saved)
            })
    }
    pub fn run(
        &mut self,
        saved: SavedConnection,
        database: String,
        selected: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.active || self.indexing || self.diagnostic_running {
            return false;
        }
        if saved.profile.kind != sql_script::kind(self.script.dialect) {
            feedback::error(t(cx, "script.error.dialect"), cx);
            return false;
        }
        if !selected && self.script.unsupported > 0 {
            feedback::error(t(cx, "script.error.unsupported"), cx);
            return false;
        }
        self.active = true;
        self.run_id += 1;
        let run = self.run_id;
        self.completed = 0;
        self.current_location = None;
        self.failed_location = None;
        self.current_sql.clear();
        self.authorized = false;
        self.preparing_authorization = false;
        self.refreshing_guards = false;
        self.write_plan = None;
        self.expected_guards.clear();
        self.authorization_summary = None;
        self.next = if selected { self.selected } else { 0 };
        self.end = if selected {
            self.selected + 1
        } else {
            self.script.statements
        };
        self.authorize_next = self.next;
        self.operation = 0;
        self.pending_request = None;
        self.executing = false;
        self.job = uuid::Uuid::new_v4();
        self.started = Instant::now();
        self.last_progress = self.started;
        self.binding = Some((saved.clone(), database.clone()));
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        self.status = t(cx, "script.verifying");
        self.results.update(cx, |results, cx| {
            results.reset(saved.clone(), database.clone(), self.end - self.next, cx)
        });
        self.log(crate::operation_log::Phase::Started, None, cx);
        let path = self.path.clone();
        let script = self.script.clone();
        let range = self.next..self.end;
        let allowed = turbodbn_services::database::notebook_query::COMMANDS
            .iter()
            .chain(turbodbn_services::database::notebook_query::SCRIPT_COMMANDS)
            .filter(|(key, _, _)| {
                cx.global::<crate::preferences::Preferences>()
                    .database
                    .command_enabled(key)
            })
            .map(|(key, _, _)| key.to_string())
            .collect::<Vec<_>>();
        let mode = self.access_mode;
        let work = cx.background_executor().spawn(async move {
            let mut reader = sql_script::freeze(&path, &script, &cancel)?;
            let mut blocked = Vec::new();
            let mut mode_blocked = None;
            if selected && saved.profile.kind == turbodbn_services::DatabaseKind::MySql {
                let mut context = false;
                for number in 0..script.statements {
                    if cancel.load(Ordering::Relaxed) {
                        return Err("script.cancelled".into());
                    }
                    let sql = reader.raw_statement(number)?;
                    turbodbn_services::database::notebook_query::validate_script_target(
                        saved.profile.kind,
                        &sql,
                        &database,
                    )?;
                    let expanded =
                        turbodbn_services::database::workbench::script_context::expand(&sql, None)?;
                    if let Some(control) =
                        turbodbn_services::database::workbench::script_context::control(&expanded)?
                    {
                        context |= !matches!(
                            control,
                            turbodbn_services::database::workbench::script_context::Control::Use(_)
                        );
                    }
                }
                if context {
                    return Err("script.error.context_required".into());
                }
            }
            let mut sequence =
                turbodbn_services::database::workbench::script_context::Sequence::default();
            for number in range {
                if cancel.load(Ordering::Relaxed) {
                    return Err(PreflightError::Other("script.cancelled".into()));
                }
                let sql = reader.statement(number)?;
                if mode_blocked.is_none()
                    && !turbodbn_services::database::notebook_query::note_mode_allows(
                        mode,
                        saved.profile.kind,
                        &sql,
                        true,
                    )
                {
                    let key = turbodbn_services::database::notebook_query::script_command_required(
                        saved.profile.kind,
                        &sql,
                    )
                    .ok_or("script.error.unsupported")?;
                    let required = if turbodbn_services::database::notebook_query::note_mode_allows(
                        turbodbn_services::access_mode::AccessMode::ReadWrite,
                        saved.profile.kind,
                        &sql,
                        true,
                    ) {
                        turbodbn_services::access_mode::AccessMode::ReadWrite
                    } else {
                        turbodbn_services::access_mode::AccessMode::Admin
                    };
                    mode_blocked = Some(crate::note_permissions::Advice::blocked_mode(
                        mode, required, key,
                    ));
                }
                turbodbn_services::database::notebook_query::validate_script_target(
                    saved.profile.kind,
                    &sql,
                    &database,
                )?;
                if selected
                    && saved.profile.kind == turbodbn_services::DatabaseKind::MySql
                    && turbodbn_services::database::workbench::script_context::control(
                        &turbodbn_services::database::workbench::script_context::expand(
                            &sql, None,
                        )?,
                    )?
                    .is_some()
                {
                    return Err("script.error.context_required".into());
                }
                sequence.push(
                    saved.profile.kind,
                    &sql,
                    turbodbn_services::database::notebook_query::script_requires_confirmation(
                        saved.profile.kind,
                        &sql,
                        false,
                    )?,
                )?;
                if let Some(block) =
                    sql_script::permission_block(&mut reader, saved.profile.kind, number, &allowed)?
                    && !blocked
                        .iter()
                        .any(|prior: &sql_script::PermissionBlock| prior.command == block.command)
                {
                    blocked.push(block);
                }
            }
            if !blocked.is_empty() {
                return Err(PreflightError::Permissions(blocked));
            }
            if let Some(advice) = mode_blocked {
                return Err(PreflightError::NoteAccess(advice));
            }
            sequence.finish()?;
            let mut endpoint = saved.endpoint(database)?;
            if endpoint.profile.kind == turbodbn_services::DatabaseKind::MySql {
                endpoint.profile.charset = "utf8mb4".into();
            }
            Ok::<_, PreflightError>((reader, endpoint))
        });
        self.task = Some(cx.spawn_in(window, async move |owner, cx| {
            let (reader, endpoint) = match work.await {
                Ok(value) => value,
                Err(error) => {
                    let _ = owner.update_in(cx, |this, window, cx| {
                        if this.run_id == run {
                            match error {
                                PreflightError::Permissions(blocks) => {
                                    this.permission_dialog(blocks, window, cx)
                                }
                                PreflightError::NoteAccess(advice)=>{this.fail("notes.access.blocked".into(),cx);this.status=advice.message(cx);cx.emit(ScriptEvent::PermissionBlocked(advice));}
                                PreflightError::Other(error) => {
                                    let mismatch = error == "script.error.database_mismatch";
                                    this.fail(error, cx);
                                    if mismatch {
                                        cx.emit(ScriptEvent::SelectTarget);
                                    }
                                }
                            }
                        }
                    });
                    return;
                }
            };
            let (session, events) = Session::start(endpoint.profile, endpoint.password, false);
            if !owner
                .update(cx, |this, cx| {
                    if !this.current(run, cx) {
                        if this.active && this.run_id == run {
                            this.fail("notes.preview_changed".into(), cx);
                        }
                        return false;
                    }
                    this.frozen = Some(Arc::new(Mutex::new(reader)));
                    this.session = Some(session);
                    true
                })
                .unwrap_or(false)
            {
                return;
            }
            while let Ok(event) = events.recv().await {
                let keep = owner
                    .update_in(cx, |this, window, cx| {
                        if !this.current(run, cx) {
                            if this.active && this.run_id == run {
                                this.fail("notes.preview_changed".into(), cx);
                            }
                            return false;
                        }
                        match event {
                            Event::Connected(info) => {
                                this.server_legacy = this.script.dialect
                                    == turbodbn_core::notebook::SqlDialect::MySql
                                    && !turbodbn_services::mysql::modern_version(&info.version);
                                this.authorize_next(window, cx);
                            }
                            Event::Operation { id, .. } if id == this.operation && this.pending_request.is_none_or(|(_,hash)|hash!=turbodbn_services::database::notebook_query::script_source_fingerprint(&this.current_sql)) => this.fail("script.error.request_mismatch".into(),cx),
                            Event::Operation { id, result } if id == this.operation => match result
                            {
                                Ok(OperationResult::Workbench(reply)) => match *reply {
                                    Reply::NotePrepared(plan) => {
                                        if this.pending_request.take()!=Some((true,turbodbn_services::database::notebook_query::script_source_fingerprint(&plan.sql))) { this.fail("script.error.request_mismatch".into(),cx); }
                                        else { this.prepared(plan,window,cx); }
                                    },
                                    Reply::NoteResult(result) => {
                                        if this.pending_request.take()!=Some((false,turbodbn_services::database::notebook_query::script_source_fingerprint(&this.current_sql))) { this.fail("script.error.request_mismatch".into(),cx);return this.active; }

                                        this.completed += 1;
                                        let line =
                                            this.current_location.map_or(0, |(_, line)| line);
                                        let refresh =
                                            this.last_progress.elapsed().as_millis() >= 250;
                                        this.results.update(cx, |results, cx| {
                                            results.push(result, this.next, line, refresh, cx)
                                        });
                                        this.executing = false;
                                        if this.last_progress.elapsed().as_millis() >= 250 {
                                            this.log(
                                                crate::operation_log::Phase::Progress,
                                                None,
                                                cx,
                                            );
                                            this.last_progress = Instant::now();
                                        }
                                        if let Some(plan) =
                                            this.write_plan.take().filter(|p| !p.guards.is_empty())
                                        {
                                            this.refreshing_guards = true;
                                            this.send(Action::ScriptPrepare { sql: plan.sql }, cx);
                                        } else {
                                            this.advance(window, cx);
                                        }
                                    }
                                    _ => this.fail("script.error.execution".into(), cx),
                                },
                                Err(error) => this.fail(error, cx),
                                _ => this.fail("script.error.execution".into(), cx),
                            },
                            Event::Failed(error) => this.fail(error, cx),
                            Event::Disconnected => this.fail("script.error.execution".into(), cx),
                            _ => {}
                        }
                        cx.notify();
                        this.active
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        }));
        cx.notify();
        true
    }
    fn permission_dialog(
        &mut self,
        blocks: Vec<sql_script::PermissionBlock>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(first) = blocks.first() else {
            return;
        };
        let number = first.number;
        self.status = t(cx, "script.permission.stopped");
        self.log(
            crate::operation_log::Phase::Failed,
            Some("settings.sql_policy_blocked"),
            cx,
        );
        self.finish(false, cx);
        let lines = blocks
            .iter()
            .map(|block| {
                tr(
                    cx,
                    "script.permission.item",
                    &[
                        ("number", &(block.number + 1).to_string()),
                        ("line", &block.line.to_string()),
                        ("command", &block.command),
                    ],
                )
            })
            .collect::<Vec<_>>();
        let owner = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let locate = owner.clone();
            dialog
                .title(t(cx, "script.permission.title"))
                .w(px(580.))
                .overlay_closable(false)
                .child(
                    div()
                        .v_flex()
                        .gap_3()
                        .child(t(cx, "script.permission.stopped"))
                        .children(lines.iter().cloned().map(|line| {
                            div()
                                .h_flex()
                                .gap_2()
                                .child(gpui_component::Icon::new(IconName::TriangleAlert))
                                .child(line)
                        }))
                        .child(t(cx, "script.permission.hint"))
                        .child(
                            feedback::actions()
                                .child(feedback::close_button(cx))
                                .child(
                                    Button::new("script-permission-locate")
                                        .icon(IconName::Search)
                                        .label(t(cx, "script.permission.locate"))
                                        .on_click(move |_, window, cx| {
                                            window.close_dialog(cx);
                                            let _ = locate.update(cx, |this, cx| {
                                                this.issues_only = false;
                                                this.selected = number;
                                                this.load_page(
                                                    number / sql_script::PAGE_SIZE
                                                        * sql_script::PAGE_SIZE,
                                                    cx,
                                                );
                                                this.select(number, cx);
                                            });
                                        }),
                                )
                                .child(
                                    Button::new("script-permission-settings")
                                        .primary()
                                        .icon(IconName::Settings)
                                        .label(t(cx, "script.permission.settings"))
                                        .on_click(|_, window, cx| {
                                            window.close_dialog(cx);
                                            let hub = cx.global::<feedback::Hub>().0.clone();
                                            cx.defer(move |cx| {
                                                hub.update(cx, |_, cx| {
                                                    cx.emit(feedback::OpenSqlSettings)
                                                })
                                            });
                                        }),
                                ),
                        ),
                )
        });
        cx.notify();
    }
    fn authorize_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(reader) = self.frozen.clone() else {
            return;
        };
        let start = self.authorize_next;
        let end = self.end;
        let run = self.run_id;
        let cancel = self.cancel.clone();
        let kind = sql_script::kind(self.script.dialect);
        let legacy = self.server_legacy;
        let summarize = self.authorization_summary.is_none();
        self.status = t(cx, "script.authorizing");
        let work = cx.background_executor().spawn(async move {
            let mut reader = reader.lock().map_err(|_| "script.error.execution")?;
            let summary = if summarize {
                Some(reader.authorization_summary(start, end, legacy, &cancel)?)
            } else {
                None
            };
            for number in start..end {
                if cancel.load(Ordering::Relaxed) {
                    return Err("script.cancelled".to_string());
                }
                let sql = reader.statement(number)?;
                if turbodbn_services::database::notebook_query::script_requires_confirmation(
                    kind, &sql, legacy,
                )? {
                    let line = reader.statement_line(number)?;
                    return Ok((Some((number, line, sql)), summary));
                }
            }
            Ok::<_, String>((None, summary))
        });
        cx.spawn_in(window, async move |owner, cx| {
            let result = work.await;
            let _ = owner.update_in(cx, |this, window, cx| {
                if !this.current(run, cx) {
                    return;
                }
                let (number, line, sql) = match result {
                    Ok((Some(value), summary)) => {
                        if let Some(summary) = summary {
                            this.authorization_summary = Some(summary);
                        }
                        value
                    }
                    Ok((None, summary)) => {
                        if let Some(summary) = summary {
                            this.authorization_summary = Some(summary);
                        }
                        this.batch_authorize(window, cx);
                        return;
                    }
                    Err(error) => {
                        this.fail(error, cx);
                        return;
                    }
                };
                this.current_location = Some((number, line));
                this.authorize_next = number;
                this.preparing_authorization = true;
                this.current_sql = sql.clone();
                this.send(Action::ScriptPrepare { sql }, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn authorize_plan(&mut self, plan: Plan, window: &mut Window, cx: &mut Context<Self>) {
        let this = self;
        let number = this.authorize_next;
        for guard in &plan.guards {
            let key = (guard.object.schema.clone(), guard.object.name.clone());
            if key.0.len() + key.1.len() > 1024
                || this.expected_guards.len() >= 4096 && !this.expected_guards.contains_key(&key)
            {
                this.fail("script.error.guard_limit".into(), cx);
                return;
            }
            let hash = turbodbn_services::database::notebook_query::guard_hash(guard);
            if this
                .expected_guards
                .get(&key)
                .is_some_and(|old| old != &hash)
            {
                this.fail("script.error.schema_changed".into(), cx);
                return;
            }
            this.expected_guards.insert(key, hash);
        }
        this.authorize_next = number + 1;
        cx.defer_in(window, |this, window, cx| this.authorize_next(window, cx));
        cx.notify();
    }
    fn batch_authorize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(reader) = self.frozen.clone() else {
            return;
        };
        let Some(summary) = self.authorization_summary.clone() else {
            return;
        };
        let Some((saved, database)) = &self.binding else {
            return;
        };
        let target = format!(
            "{} / {} · {}",
            saved.profile.name, database, saved.profile.environment
        );
        let weak = cx.entity().downgrade();
        let run = self.run_id;
        let outcome = crate::script_authorize::open(
            reader,
            summary,
            target,
            self.next,
            self.end,
            self.server_legacy,
            window,
            cx,
            move |window, cx| {
                weak.update(cx, |this, cx| {
                    if !this.current(run, cx) {
                        feedback::error(t(cx, "notes.preview_changed"), cx);
                        return false;
                    }
                    this.authorized = true;
                    cx.defer_in(window, |this, window, cx| this.load_next(window, cx));
                    true
                })
                .unwrap_or(false)
            },
        );
        self.risk_task = Some(cx.spawn(async move |owner, cx| {
            if !outcome.recv().await.unwrap_or(false) {
                let _ = owner.update(cx, |this, cx| {
                    if this.run_id == run && !this.authorized {
                        this.stop(cx);
                    }
                });
            }
        }));
        cx.notify();
    }
    fn load_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.current_location = None;
        self.preparing_authorization = false;
        self.status = tr(
            cx,
            "script.progress",
            &[
                ("done", &self.completed.to_string()),
                (
                    "total",
                    &(self.end - self.next + self.completed).to_string(),
                ),
            ],
        );
        let Some(reader) = self.frozen.clone() else {
            self.fail("script.error.execution".into(), cx);
            return;
        };
        let number = self.next;
        let run = self.run_id;
        let work = cx.background_executor().spawn(async move {
            let mut reader = reader.lock().map_err(|_| "script.error.execution")?;
            let line = reader.statement_line(number)?;
            reader.statement(number).map(|sql| (line, sql))
        });
        cx.spawn_in(window, async move |owner, cx| {
            let result = work.await;
            let _ = owner.update_in(cx, |this, _, cx| {
                if !this.current(run, cx) {
                    if this.active && this.run_id == run {
                        this.fail("notes.preview_changed".into(), cx);
                    }
                    return;
                }
                match result {
                    Ok((line, sql)) => {
                        this.current_location = Some((number, line));
                        this.current_sql = sql.clone();
                        this.send(Action::ScriptPrepare { sql }, cx);
                    }
                    Err(error) => this.fail(error, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn permitted(&self, plan: &Plan, cx: &App) -> bool {
        self.binding.as_ref().is_some_and(|(saved, _)| {
            turbodbn_services::database::notebook_query::script_command_required(
                saved.profile.kind,
                &plan.sql,
            )
            .is_some_and(|key| {
                cx.global::<crate::preferences::Preferences>()
                    .database
                    .command_enabled(key)
                    && self.access_mode.allows_note(key)
            })
        })
    }
    fn prepared(&mut self, plan: Plan, window: &mut Window, cx: &mut Context<Self>) {
        if self.refreshing_guards {
            self.refreshing_guards = false;
            for guard in &plan.guards {
                self.expected_guards.insert(
                    (guard.object.schema.clone(), guard.object.name.clone()),
                    turbodbn_services::database::notebook_query::guard_hash(guard),
                );
            }
            self.advance(window, cx);
            return;
        }
        if let Some(advice) = self.binding.as_ref().and_then(|(saved, _)| {
            crate::note_permissions::Advice::for_sql(
                self.access_mode,
                &saved.profile,
                &plan.sql,
                true,
                cx,
            )
        }) {
            self.fail("notes.access.blocked".into(), cx);
            self.status = advice.message(cx);
            cx.emit(ScriptEvent::PermissionBlocked(advice));
            return;
        }
        if !self.permitted(&plan, cx) {
            self.fail("settings.sql_policy_blocked".into(), cx);
            return;
        }
        if plan.sql != self.current_sql.trim() {
            self.fail("notes.preview_changed".into(), cx);
            return;
        }
        if self.preparing_authorization {
            self.preparing_authorization = false;
            self.authorize_plan(plan, window, cx);
            return;
        }
        for guard in &plan.guards {
            let key = (guard.object.schema.clone(), guard.object.name.clone());
            if self.expected_guards.get(&key)
                != Some(&turbodbn_services::database::notebook_query::guard_hash(
                    guard,
                ))
            {
                self.fail("script.error.schema_changed".into(), cx);
                return;
            }
        }
        if !plan.read_only
            && self.binding.as_ref().is_some_and(|(saved, _)| {
                saved.profile.kind == turbodbn_services::DatabaseKind::Sqlite
                    && saved.profile.sqlite_read_only
            })
        {
            self.fail("notes.read_only".into(), cx);
            return;
        }
        if !self.authorized {
            self.fail("notes.preview_changed".into(), cx);
            return;
        }
        let confirmed = plan.confirm;
        self.results
            .update(cx, |results, _| results.begin_statement(self.next, &plan));
        self.write_plan = (!plan.read_only).then(|| plan.clone());
        self.executing = true;
        self.send(Action::ScriptExecute { plan, confirmed }, cx);
    }

    fn advance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.next += 1;
        if self.next >= self.end {
            self.finish(true, cx);
        } else {
            self.load_next(window, cx);
        }
    }
    fn send(&mut self, action: Action, cx: &mut Context<Self>) {
        let Some((_, database)) = &self.binding else {
            return;
        };
        self.pending_request = match &action {
            Action::ScriptPrepare { sql } => Some((
                true,
                turbodbn_services::database::notebook_query::script_source_fingerprint(sql),
            )),
            Action::ScriptExecute { plan, .. } => {
                if turbodbn_services::database::notebook_query::script_source_fingerprint(&plan.sql)
                    != turbodbn_services::database::notebook_query::script_source_fingerprint(
                        &self.current_sql,
                    )
                {
                    self.fail("script.error.request_mismatch".into(), cx);
                    return;
                }
                Some((
                    false,
                    turbodbn_services::database::notebook_query::script_source_fingerprint(
                        &plan.sql,
                    ),
                ))
            }
            _ => None,
        };
        self.operation += 1;
        if self.session.as_ref().is_none_or(|session| {
            session
                .request(Operation {
                    id: self.operation,
                    kind: OperationKind::Workbench(Box::new(Request {
                        database: database.clone(),
                        action,
                    })),
                })
                .is_err()
        }) {
            self.fail("script.error.execution".into(), cx);
        }
    }
    fn fail(&mut self, error: String, cx: &mut Context<Self>) {
        let reason = crate::locale::message(cx, &error);
        self.failed_location = self.current_location;
        if let Some((number, line)) = self.failed_location {
            let stage = if self.refreshing_guards {
                "script.report.verify_failed"
            } else if self.executing {
                "script.report.failed"
            } else {
                "script.report.prepare_failed"
            };
            self.results
                .update(cx, |results, cx| results.fail(number, line, stage, cx));
        }
        self.status = if let Some((number, line)) = self.failed_location {
            let stage = t(
                cx,
                if self.refreshing_guards {
                    "script.failure.after_write"
                } else if self.executing {
                    "script.failure.execute"
                } else {
                    "script.failure.prepare"
                },
            );
            tr(
                cx,
                "script.failure.location",
                &[
                    ("number", &(number + 1).to_string()),
                    ("line", &line.to_string()),
                    ("stage", &stage),
                    ("done", &self.completed.to_string()),
                    ("reason", &reason),
                ],
            )
        } else {
            reason
        };
        self.log(crate::operation_log::Phase::Failed, Some(&error), cx);
        if error != "notes.access.blocked" {
            if matches!(
                error.as_str(),
                "mysql.error.command_denied"
                    | "mysql.error.privilege_1227"
                    | "mysql.error.trigger_privilege_1419"
            ) {
                let owner = cx.entity().downgrade();
                let run = self.run_id;
                feedback::action_error(
                    self.status.clone(),
                    t(cx, "script.diagnostics.run"),
                    move |window, cx| {
                        let _ = owner.update(cx, |this, cx| {
                            if this.run_id != run {
                                feedback::error(t(cx, "permissions.expired"), cx);
                                return;
                            }
                            this.diagnose(window, cx);
                        });
                    },
                    cx,
                );
            } else {
                feedback::error(self.status.clone(), cx);
            }
        }
        self.finish(false, cx);
    }
    fn finish(&mut self, success: bool, cx: &mut Context<Self>) {
        if !self.active {
            return;
        }
        self.active = false;
        self.results.update(cx, |results, cx| results.finish(cx));
        self.cancel.store(true, Ordering::Relaxed);
        self.session = None;
        self.frozen = None;
        self.expected_guards = std::collections::HashMap::new();
        self.write_plan = None;
        if success {
            self.status = tr(
                cx,
                "script.completed",
                &[("count", &self.completed.to_string())],
            );
            self.log(crate::operation_log::Phase::Succeeded, None, cx);
            feedback::success(self.status.clone(), cx);
        }
        let owner = cx.entity().downgrade();
        let run = self.run_id;
        cx.defer(move |cx| {
            let _ = owner.update(cx, |_, cx| cx.emit(ScriptEvent::Finished { run, success }));
        });
        cx.notify();
    }
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        if self.indexing {
            self.cancel.store(true, Ordering::Relaxed);
            self.page_task = None;
            self.indexing = false;
            self.index_error = true;
            self.status = t(cx, "script.index_cancelled");
        }
        if self.active {
            self.status = t(
                cx,
                if self.executing {
                    "script.cancelled_write"
                } else {
                    "script.cancelled"
                },
            );
            self.log(crate::operation_log::Phase::Cancelled, None, cx);
            self.finish(false, cx);
        }
        self.run_id += 1;
        self.risk_task = None;
        cx.notify();
    }
}
impl Drop for ScriptView {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
impl Render for ScriptView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active;
        let disabled = active || self.locked || self.indexing;
        let run_tip = if active {
            t(cx, "script.disabled.running")
        } else if self.locked {
            t(cx, "script.disabled.batch")
        } else if self.indexing {
            t(cx, "script.reindexing")
        } else if self.script.unsupported > 0 {
            tr(
                cx,
                "script.disabled.unsupported",
                &[("count", &self.script.unsupported.to_string())],
            )
        } else {
            t(cx, "script.run_hint")
        };
        let supported = self
            .rows
            .iter()
            .find(|entry| entry.number == self.selected)
            .is_some_and(|entry| entry.supported);
        let mut body = div()
            .v_flex()
            .gap_2()
            .min_w_0()
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(
                        div()
                            .id("script-run-all-tooltip")
                            .tooltip(move |window, cx| {
                                gpui_component::tooltip::Tooltip::new(run_tip.clone())
                                    .build(window, cx)
                            })
                            .child(
                                Button::new("script-run-all")
                                    .icon(IconName::ArrowRight)
                                    .label(t(cx, "script.run_all"))
                                    .disabled(disabled || self.script.unsupported > 0)
                                    .on_click(
                                        cx.listener(|_, _, _, cx| cx.emit(ScriptEvent::Run(false))),
                                    ),
                            ),
                    )
                    .child(
                        Button::new("script-run-selected")
                            .icon(IconName::ArrowRight)
                            .label(t(cx, "script.run_selected"))
                            .disabled(disabled || !supported)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(ScriptEvent::Run(true)))),
                    )
                    .child(
                        Button::new("script-stop")
                            .icon(IconName::Close)
                            .label(t(cx, "script.stop"))
                            .disabled(!active)
                            .on_click(cx.listener(|this, _, _, cx| this.stop(cx))),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(
                                Button::new("script-issue-filter")
                                    .small()
                                    .ghost()
                                    .icon(IconName::TriangleAlert)
                                    .label(tr(
                                        cx,
                                        "script.issue_count",
                                        &[("count", &self.script.unsupported.to_string())],
                                    ))
                                    .selected(self.issues_only)
                                    .disabled(self.indexing || self.script.unsupported == 0)
                                    .tooltip(t(cx, "script.issue_filter_hint"))
                                    .on_click(cx.listener(|this, _, _, cx| this.toggle_issues(cx))),
                            )
                            .when(self.issues_only, |el| {
                                el.child(
                                    Button::new("script-clear-issues")
                                        .small()
                                        .ghost()
                                        .icon(IconName::Close)
                                        .tooltip(t(cx, "script.clear_filter"))
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.toggle_issues(cx)),
                                        ),
                                )
                            })
                            .when(self.issues_only, |el| {
                                el.child(
                                    Button::new("script-prev-issue")
                                        .small()
                                        .ghost()
                                        .icon(IconName::ChevronLeft)
                                        .tooltip(t(cx, "script.previous_issue"))
                                        .on_click(
                                            cx.listener(|this, _, _, cx| {
                                                this.step_issue(false, cx)
                                            }),
                                        ),
                                )
                                .child(
                                    Button::new("script-next-issue")
                                        .small()
                                        .ghost()
                                        .icon(IconName::ChevronRight)
                                        .tooltip(t(cx, "script.next_issue"))
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.step_issue(true, cx)),
                                        ),
                                )
                            }),
                    ),
            )
            .child(tr(
                cx,
                "script.summary",
                &[
                    ("count", &self.script.statements.to_string()),
                    ("writes", &self.script.writes.to_string()),
                    (
                        "reads",
                        &(self.script.statements - self.script.writes - self.script.unsupported)
                            .to_string(),
                    ),
                    ("unsupported", &self.script.unsupported.to_string()),
                    (
                        "size",
                        &format!("{:.1}", self.script.bytes as f64 / 1048576.),
                    ),
                ],
            ));
        if self.index_error {
            body = body.child(
                Button::new("script-retry-index")
                    .icon(gpui_component::Icon::default().path("icons/refresh.svg"))
                    .label(t(cx, "script.retry_index"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.cancel = Arc::new(AtomicBool::new(false));
                        this.refresh_index(cx);
                    })),
            );
        }
        body = body.child(
            div()
                .relative()
                .h(px(180.))
                .w_full()
                .child(
                    uniform_list(
                        "script-rows",
                        self.rows.len(),
                        cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                            range
                                .map(|index| {
                                    let row = &this.rows[index];
                                    let number = row.number;
                                    div()
                                        .id(SharedString::from(format!("script-row-{number}")))
                                        .h_flex()
                                        .h(px(30.))
                                        .gap_2()
                                        .px_2()
                                        .items_center()
                                        .bg(if this.selected == number {
                                            cx.theme().muted
                                        } else {
                                            transparent_black()
                                        })
                                        .child(div().w(px(48.)).child((number + 1).to_string()))
                                        .child(
                                            div()
                                                .w(px(64.))
                                                .text_color(cx.theme().muted_foreground)
                                                .child(format!("L{}", row.line)),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .truncate()
                                                .child(row.preview.clone()),
                                        )
                                        .when(row.has_comments, |el| {
                                            el.child(
                                                Button::new(SharedString::from(format!(
                                                    "script-comments-{number}"
                                                )))
                                                .ghost()
                                                .small()
                                                .icon(IconName::Info)
                                                .tooltip(t(cx, "script.comments_hint"))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.select(number, cx);
                                                    this.show_comments = true;
                                                })),
                                            )
                                        })
                                        .when(!row.supported, |el| {
                                            let reason = crate::locale::message(cx, &row.reason);
                                            el.child(
                                                div()
                                                    .id(SharedString::from(format!(
                                                        "script-error-{number}"
                                                    )))
                                                    .tooltip(move |window, cx| {
                                                        gpui_component::tooltip::Tooltip::new(
                                                            reason.clone(),
                                                        )
                                                        .build(window, cx)
                                                    })
                                                    .child(gpui_component::Icon::new(
                                                        IconName::TriangleAlert,
                                                    )),
                                            )
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.select(number, cx)
                                        }))
                                })
                                .collect()
                        }),
                    )
                    .track_scroll(self.list_scroll.clone())
                    .h_full()
                    .w_full(),
                )
                .child(
                    div()
                        .absolute()
                        .right_0()
                        .top_0()
                        .bottom_0()
                        .w(px(14.))
                        .child(
                            Scrollbar::vertical(&self.list_scroll)
                                .scrollbar_show(ScrollbarShow::Always),
                        ),
                )
                .w_full(),
        );
        body = body.child(
            div()
                .h_flex()
                .gap_3()
                .justify_end()
                .child(
                    Button::new("script-page-prev")
                        .ghost()
                        .icon(IconName::ChevronLeft)
                        .tooltip(t(cx, "script.previous"))
                        .disabled(self.page == 0)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.load_page(this.page.saturating_sub(sql_script::PAGE_SIZE), cx)
                        })),
                )
                .child(format!(
                    "{}–{} / {}",
                    self.page + 1,
                    (self.page + sql_script::PAGE_SIZE).min(if self.issues_only {
                        self.script.unsupported
                    } else {
                        self.script.statements
                    }),
                    if self.issues_only {
                        self.script.unsupported
                    } else {
                        self.script.statements
                    }
                ))
                .child(
                    Button::new("script-page-next")
                        .ghost()
                        .icon(IconName::ChevronRight)
                        .tooltip(t(cx, "script.next"))
                        .disabled(
                            self.page + sql_script::PAGE_SIZE
                                >= if self.issues_only {
                                    self.script.unsupported
                                } else {
                                    self.script.statements
                                },
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.load_page(this.page + sql_script::PAGE_SIZE, cx)
                        })),
                ),
        );
        if !self.preview_markdown.is_empty() {
            body = body.child(
                div()
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.preview_range.clone()),
                    )
                    .when(!self.preview_comments.is_empty(), |el| {
                        el.child(
                            Button::new("script-show-comments")
                                .ghost()
                                .small()
                                .icon(IconName::Info)
                                .label(t(cx, "script.comments"))
                                .selected(self.show_comments)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.show_comments = !this.show_comments;
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        Button::new("script-copy-sql")
                            .ghost()
                            .small()
                            .icon(IconName::Copy)
                            .label(t(cx, "script.copy_sql"))
                            .tooltip(t(cx, "script.copy_sql_hint"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(
                                    this.preview_sql.to_string(),
                                ));
                                feedback::success(t(cx, "feedback.copied"), cx);
                            })),
                    ),
            );
            if self.show_comments {
                body = body.child(
                    div()
                        .id("script-leading-comments")
                        .max_h(px(140.))
                        .overflow_y_scroll()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .whitespace_normal()
                        .child(self.preview_comments.clone()),
                );
            }

            body = body.child(
                div()
                    .id("script-preview")
                    .relative()
                    .max_h(px(220.))
                    .overflow_y_scroll()
                    .child(
                        TextView::markdown(
                            "script-current-source",
                            self.preview_markdown.clone(),
                            window,
                            cx,
                        )
                        .selectable(true),
                    )
                    .child(
                        div().absolute().right(px(6.)).top(px(6.)).child(
                            Button::new("script-copy-preview")
                                .ghost()
                                .icon(IconName::Copy)
                                .tooltip(t(cx, "script.copy_source"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        this.preview.to_string(),
                                    ));
                                    feedback::success(t(cx, "feedback.copied"), cx);
                                })),
                        ),
                    ),
            );
        }
        body = body.child(self.results.clone());
        if self.failed_location.is_some()
            && self.binding.as_ref().is_some_and(|(saved, _)| {
                saved.profile.kind == turbodbn_services::DatabaseKind::MySql
            })
        {
            body = body.child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(
                        Button::new("script-diagnostics")
                            .ghost()
                            .icon(IconName::Search)
                            .label(t(
                                cx,
                                if self.diagnostic_running {
                                    "script.diagnostics.running"
                                } else {
                                    "script.diagnostics.run"
                                },
                            ))
                            .tooltip(t(cx, "script.diagnostics.tip"))
                            .disabled(self.active || self.diagnostic_running)
                            .on_click(cx.listener(|this, _, window, cx| this.diagnose(window, cx))),
                    )
                    .when(self.diagnostic_running, |el| {
                        el.child(
                            Button::new("cancel-script-diagnostics")
                                .ghost()
                                .icon(IconName::Close)
                                .tooltip(t(cx, "script.diagnostics.cancel"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.stop_diagnostics(cx);
                                    cx.notify();
                                })),
                        )
                    }),
            );
        }
        if let Some((number, line)) = self.failed_location {
            body = body.child(
                Button::new("script-locate-failure")
                    .ghost()
                    .icon(IconName::TriangleAlert)
                    .label(tr(
                        cx,
                        "script.failure.locate",
                        &[
                            ("number", &(number + 1).to_string()),
                            ("line", &line.to_string()),
                        ],
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.issues_only = false;
                        this.selected = number;
                        this.preview_epoch += 1;
                        this.preview_task = None;
                        this.load_page(number / sql_script::PAGE_SIZE * sql_script::PAGE_SIZE, cx);
                        this.set_preview(this.current_sql.clone(), cx);
                        cx.notify();
                    })),
            );
        }
        body.child(
            div()
                .text_color(cx.theme().muted_foreground)
                .child(self.status.clone()),
        )
    }
}

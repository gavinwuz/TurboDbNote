use crate::{
    database::{DatabaseChoice, DatabaseView, SavedConnection},
    feedback,
    locale::t,
    query_grid::QueryGrid,
};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Disableable, IconName, RopeExt, Selectable, Sizable, StyledExt, WindowExt,
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
    menu::{DropdownMenu, PopupMenuItem},
    scroll::{Scrollbar, ScrollbarShow},
    select::{SearchableVec, Select, SelectEvent, SelectState},
    table::{Table, TableState},
    text::TextView,
};
use turbodbn_core::{Cell, CellContent, QueryState, QueryStatus};
use turbodbn_services::{
    DatabaseKind,
    database::{
        Operation, OperationKind, OperationResult,
        notebook_query::{ExecutionSummary, Plan},
        workbench::{Action, Reply, Request, Rows},
    },
    mysql::{Event, Session},
};
use uuid::Uuid;

#[path = "cell_assist.rs"]
mod assist;
#[path = "markdown_tools.rs"]
pub(super) mod markdown_tools;
#[path = "cell_sql_range.rs"]
mod sql_range;

/// Immutable text storage is captured on the UI thread and materialized by the save worker.
pub(crate) struct CellSnapshot {
    cell: Cell,
    source: Option<gpui_component::Rope>,
}
impl CellSnapshot {
    pub(crate) fn finish(mut self) -> Cell {
        if let Some(text) = self.source {
            match &mut self.cell.content {
                CellContent::Sql { source, .. }
                | CellContent::SqlScriptText { source, .. }
                | CellContent::Markdown { source } => *source = text.to_string(),
                _ => {}
            }
        }
        self.cell
    }
}

pub enum CellEvent {
    Changed,
    ReplaceSqlFile,
    SqlAvailabilityChanged,
    PermissionBlocked(crate::note_permissions::Advice),
    SelectTarget,
    BatchFinished { token: Uuid, success: bool },
    Remove,
    Move(i32),
    Edit,
    Insert { after: bool, copy: bool },
}
pub struct BatchScope {
    revision: u64,
    connection: SavedConnection,
}
pub struct CellView {
    query_only: bool,
    note_path: std::path::PathBuf,
    batch_view: Option<Entity<crate::script_cell::ScriptView>>,
    batch_preparing: Option<Task<()>>,
    batch_subscription: Option<Subscription>,
    cell: Cell,
    markdown_last: Option<markdown_tools::FormatMemo>,
    task_list: Option<Entity<crate::task_list::TaskList>>,
    editor: Entity<InputState>,
    editor_height_limit: f32,
    editor_content_height: f32,
    toolbar_scroll: ScrollHandle,
    toolbar_compact: bool,
    split_view: bool,
    completion: std::rc::Rc<assist::SqlCompletion>,
    assistance_paused: bool,
    title: Entity<InputState>,
    database: Entity<InputState>,
    connections: Entity<DatabaseView>,
    inherited_connection: Option<Uuid>,
    inherited_database: String,
    access_mode: turbodbn_services::access_mode::AccessMode,
    editing: bool,
    note_active: bool,
    formatting: Option<Task<()>>,
    column_loading: Option<Task<()>>,
    ddl_epoch: std::cell::Cell<u64>,
    assist_scope: Option<(Option<DatabaseKind>, Option<Uuid>, String)>,
    collapsed: bool,
    batch_locked: bool,
    batch_token: Option<Uuid>,
    awaiting_preview: bool,
    preview_task: Option<Task<()>>,
    focus: FocusHandle,
    position: (usize, usize),
    database_picker: Entity<SelectState<SearchableVec<DatabaseChoice>>>,
    database_names: Vec<String>,
    picker_context: Option<(Vec<String>, String)>,
    reading: Option<(SharedString, SharedString)>,
    discovery: Option<Task<()>>,
    discovery_generation: u64,
    discovery_snapshot: Option<SavedConnection>,
    wrap: bool,
    revision: u64,
    query: QueryState,
    session: Option<Session>,
    task: Option<Task<()>>,
    snapshot: Option<SavedConnection>,
    last_revision: u64,
    executed_sql: Option<String>,
    last_offset: usize,
    more: bool,
    elapsed: u64,
    operation: Option<(Uuid, std::time::Instant)>,
    notice: String,
    note_name: String,
    policy_help: bool,
    grid: Entity<TableState<QueryGrid>>,
    document: Option<Entity<crate::result_document::ResultDocument>>,
    json_mode: bool,
    execution_summary: Option<ExecutionSummary>,
    execution_details: bool,
    execution_grid: Entity<TableState<QueryGrid>>,
    executing_write: Option<String>,
    results: Option<Entity<crate::table_view::TableView>>,
    _results_observer: Option<Subscription>,
    _results_clear: Option<Subscription>,
    _results_permission: Option<Subscription>,
    page_size: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
    _connections: Subscription,
    script: Option<Entity<crate::script_cell::ScriptView>>,
    script_running: bool,
    script_run_id: u64,
    _script_subscription: Option<Subscription>,
}
impl EventEmitter<CellEvent> for CellView {}
impl Focusable for CellView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl CellView {
    pub(crate) fn note_scroll_handles(&self, cx: &App) -> Vec<ScrollHandle> {
        let mut handles = vec![self.toolbar_scroll.clone()];
        if let Some(batch) = &self.batch_view {
            handles.extend(batch.read(cx).note_scroll_handles(cx));
        }
        for table in [&self.grid, &self.execution_grid] {
            let table = table.read(cx);
            handles.extend([
                table.vertical_scroll_handle.0.borrow().base_handle.clone(),
                table.horizontal_scroll_handle.base_handle().clone(),
            ]);
        }
        if let Some(results) = &self.results {
            handles.extend(results.read(cx).note_scroll_handles(cx));
        }
        if let Some(document) = &self.document {
            handles.extend(document.read(cx).note_scroll_handles());
        }
        handles
    }
    pub(crate) fn reading_focused(&self, window: &Window, cx: &App) -> bool {
        !self.editing && self.focus.contains_focused(window, cx)
    }
    pub(crate) fn open_table_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.cell.content.is_editable_sql() {
            return;
        }
        self.query_only = true;
        self.set_editing(true, window, cx);
        self.editor.update(cx, |input, cx| input.focus(window, cx));
        let Some(kind) = self.assist_context(cx).0 else {
            return;
        };
        let sql = self.editor.read(cx).value();
        let safe = turbodbn_services::database::notebook_query::automatic_table_read(kind, &sql);
        if safe {
            self.run(0, window, cx);
        } else {
            feedback::error(t(cx, "query_note.manual_run"), cx);
        }
    }
    pub(crate) fn set_query_only(&mut self, value: bool) {
        self.query_only = value;
    }

    pub(crate) fn replace_sql(&mut self, sql: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.cell.content.is_editable_sql() {
            self.editor
                .update(cx, |editor, cx| editor.set_value(sql, window, cx));
            cx.notify();
        }
    }

    pub fn new(
        mut cell: Cell,
        connections: Entity<DatabaseView>,
        note_path: std::path::PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let source = match &mut cell.content {
            CellContent::Sql { source, .. }
            | CellContent::SqlScriptText { source, .. }
            | CellContent::Markdown { source } => std::mem::take(source),
            CellContent::Task { description, .. } => std::mem::take(description),
            CellContent::SqlScript { .. } => String::new(),
        };
        let sql = cell.content.is_editable_sql();
        let wrap = cx
            .global::<crate::preferences::Preferences>()
            .database
            .sql_wrap;
        let provider = std::rc::Rc::new(assist::SqlCompletion::new(
            cx.entity().downgrade(),
            window.window_handle(),
        ));
        let completion = provider.clone();
        let editor = cx.new(|cx| {
            let input = InputState::new(window, cx)
                .multi_line(true)
                .default_value(source);
            if sql {
                let mut input = input
                    .scroll_beyond_last_line(false)
                    .code_editor("sql")
                    .deferred_highlighting(crate::SQL_LIGHT_LANGUAGE)
                    .soft_wrap(wrap);
                input.lsp.completion_provider = Some(provider);
                input
            } else {
                input
            }
        });
        let title = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t(cx, "notes.cell_title"))
                .default_value(cell.title.clone())
        });
        let database = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t(cx, "notes.database"))
                .default_value(match &cell.content {
                    CellContent::Sql { database, .. }
                    | CellContent::SqlScriptText { database, .. }
                    | CellContent::SqlScript { database, .. } => {
                        database.clone().unwrap_or_default()
                    }
                    _ => String::new(),
                })
        });
        let page_size = cx.new(|cx| InputState::new(window, cx).default_value("20"));
        let mut subscriptions: Vec<Subscription> =
            [editor.clone(), title.clone(), database.clone()]
                .iter()
                .map(|input| {
                    let query_input = input.entity_id() != title.entity_id();
                    let scope_input = input.entity_id() == database.entity_id();
                    let editor_input = query_input && !scope_input;
                    let mut previous = input.read(cx).text().clone();
                    cx.subscribe(input, move |this, input, event, cx| {
                        if matches!(event, InputEvent::Change) {
                            let current = input.read(cx).text().clone();
                            if current == previous {
                                return;
                            }
                            let max_lines = this.editor_max_lines();
                            let (shape_changed, blank_changed) = if sql && editor_input {
                                let current = editor_shape(&current, max_lines);
                                let previous = editor_shape(&previous, max_lines);
                                (current != previous, current.1 != previous.1)
                            } else {
                                (false, false)
                            };
                            let empty_changed =
                                editor_input && (current.len() == 0) != (previous.len() == 0);
                            let refresh = empty_changed
                                || !sql
                                || !editor_input
                                || !this.editing
                                || this.last_revision == this.revision
                                || shape_changed;
                            previous = current;
                            if scope_input {
                                if let Some(script) = &this.script {
                                    script.update(cx, |script, cx| script.scope_changed(cx));
                                }
                                this.auto_connect(cx);
                                this.publish_ddl_context(false, cx);
                            }
                            if query_input {
                                if this.last_revision == this.revision
                                    && let Some(results) = &this.results
                                {
                                    results.update(cx, |results, cx| results.stale_result(cx));
                                }
                                this.revision += 1;
                            }
                            cx.emit(CellEvent::Changed);
                            if blank_changed {
                                cx.emit(CellEvent::SqlAvailabilityChanged);
                            }
                            if refresh {
                                cx.notify();
                            }
                        }
                    })
                })
                .collect();
        subscriptions.push(cx.subscribe_in(
            &connections,
            window,
            |this, _, event: &crate::database::NoteColumnsReady, window, cx| {
                if this.connection_id() == Some(event.0)
                    && event.1 != Some(cx.entity_id())
                    && this.column_loading.is_none()
                    && this.editing
                    && this.completion.can_refresh()
                    && this.editor.read(cx).focus_handle(cx).is_focused(window)
                {
                    cx.defer_in(window, |this, window, cx| {
                        this.trigger_cached_completion(window, cx)
                    });
                }
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &connections,
            window,
            |this, _, event: &crate::database::NoteCatalogReady, window, cx| {
                this.catalog_ready(event, window, cx);
            },
        ));
        let database_picker = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(Vec::<DatabaseChoice>::new()),
                None,
                window,
                cx,
            )
            .searchable(true)
        });
        subscriptions.push(cx.subscribe_in(
            &database_picker,
            window,
            |this, _, event: &SelectEvent<SearchableVec<DatabaseChoice>>, window, cx| {
                let SelectEvent::Confirm(value) = event;
                if !this.busy() {
                    this.database.update(cx, |input, cx| {
                        input.set_value(value.clone().unwrap_or_default(), window, cx)
                    });
                }
            },
        ));
        let observed = cx.observe_in(&connections, window, |this, db, _window, cx| {
            this.auto_connect(cx);
            if this.discovery.is_none()
                && this
                    .discovery_snapshot
                    .as_ref()
                    .is_some_and(|s| !db.read(cx).connection_snapshot_current(s))
            {
                this.database_names.clear();
                this.discovery_snapshot = None;
                this.picker_context = None;
            }
            if this
                .snapshot
                .as_ref()
                .is_some_and(|s| !db.read(cx).connection_snapshot_current(s))
            {
                this.stop(cx);
                if let Some(results) = &this.results {
                    results.update(cx, |results, cx| results.stale_result(cx));
                }
            }
            cx.notify();
        });
        let grid = cx.new(|cx| {
            TableState::new(QueryGrid::new(Rows::default(), 0), window, cx)
                .row_selectable(false)
                .col_selectable(false)
                .col_movable(false)
        });
        let task_list = if let CellContent::Task {
            items,
            status,
            due_at,
            remind_at,
            ..
        } = &cell.content
        {
            let description = editor.read(cx).value().to_string();
            let task = cx.new(|cx| {
                crate::task_list::TaskList::new(
                    items.clone(),
                    description,
                    (*status, *due_at, *remind_at),
                    cx,
                )
            });
            subscriptions.push(cx.subscribe(
                &task,
                |this, _, _: &crate::task_list::Changed, cx| {
                    this.revision += 1;
                    cx.emit(CellEvent::Changed);
                    cx.notify();
                },
            ));
            Some(task)
        } else {
            None
        };
        let script = if let CellContent::SqlScript { script, .. } = &cell.content {
            Some(cx.new(|cx| {
                crate::script_cell::ScriptView::new(
                    note_path.clone(),
                    script.clone(),
                    connections.clone(),
                    window,
                    cx,
                )
            }))
        } else {
            None
        };
        let script_subscription = script.as_ref().map(|script| {
            cx.subscribe_in(script, window, |this, _, event, window, cx| match event {
                crate::script_cell::ScriptEvent::Reindexed(script) => {
                    if let CellContent::SqlScript {
                        script: current, ..
                    } = &mut this.cell.content
                    {
                        *current = script.clone();
                        this.revision += 1;
                        cx.emit(CellEvent::Changed);
                        cx.notify();
                    }
                }
                crate::script_cell::ScriptEvent::PermissionBlocked(advice) => {
                    cx.emit(CellEvent::PermissionBlocked(advice.clone()))
                }
                crate::script_cell::ScriptEvent::SelectTarget => {
                    cx.emit(CellEvent::SelectTarget);
                }
                crate::script_cell::ScriptEvent::Run(selected) => {
                    let selected = *selected;
                    cx.defer_in(window, move |this, window, cx| {
                        this.start_script(selected, window, cx);
                    });
                }
                crate::script_cell::ScriptEvent::Finished { run, success } => {
                    if this.script_run_id == *run {
                        this.script_running = false;
                        this.finish_batch(*success, cx);
                        cx.notify();
                    }
                }
            })
        });
        Self {
            query_only: false,
            note_path,
            batch_view: None,
            batch_preparing: None,
            batch_subscription: None,
            editor_height_limit: 280.,
            editor_content_height: 0.,
            toolbar_scroll: ScrollHandle::new(),
            toolbar_compact: false,
            split_view: false,
            script,
            script_running: false,
            script_run_id: 0,
            _script_subscription: script_subscription,
            query: QueryState::new(cell.id),
            cell,
            markdown_last: None,
            task_list,
            editor,
            completion,
            assistance_paused: false,
            title,
            database,
            connections,
            inherited_connection: None,
            inherited_database: String::new(),
            access_mode: Default::default(),
            editing: false,
            note_active: true,
            formatting: None,
            column_loading: None,
            ddl_epoch: Default::default(),
            assist_scope: None,
            collapsed: false,
            batch_locked: false,
            batch_token: None,
            awaiting_preview: false,
            preview_task: None,
            focus: cx.focus_handle(),
            position: (0, 1),
            database_picker,
            database_names: vec![],
            picker_context: None,
            reading: None,
            discovery: None,
            discovery_generation: 0,
            discovery_snapshot: None,
            wrap,
            revision: 0,
            session: None,
            task: None,
            snapshot: None,
            last_revision: 0,
            executed_sql: None,
            last_offset: 0,
            more: false,
            elapsed: 0,
            operation: None,
            notice: String::new(),
            note_name: String::new(),
            policy_help: false,
            grid,
            document: None,
            json_mode: false,
            execution_summary: None,
            execution_details: false,
            execution_grid: cx
                .new(|cx| TableState::new(QueryGrid::new(Rows::default(), 0), window, cx)),
            executing_write: None,
            results: None,
            _results_observer: None,
            _results_clear: None,
            _results_permission: None,
            page_size,
            _subscriptions: subscriptions,
            _connections: observed,
        }
    }
    fn inherits_connection(&self) -> bool {
        matches!(
            self.cell.content,
            CellContent::Sql {
                connection_id: None,
                ..
            } | CellContent::SqlScriptText {
                connection_id: None,
                ..
            } | CellContent::SqlScript {
                connection_id: None,
                ..
            }
        )
    }
    pub fn permission_connection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.working(cx) || self.result_pending(cx) {
            feedback::error(t(cx, "permissions.busy"), cx);
            return;
        }
        let required_kind = match &self.cell.content {
            CellContent::SqlScript { script, .. } => {
                Some(turbodbn_services::sql_script::kind(script.dialect))
            }
            _ => None,
        };
        let profiles = self
            .connections
            .read(cx)
            .note_profiles()
            .into_iter()
            .filter(|p| p.supports_mutation() && required_kind.is_none_or(|kind| p.kind == kind))
            .collect::<Vec<_>>();
        let cell = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            dialog
                .title(t(cx, "permissions.choose_connection"))
                .w(px(480.))
                .child(
                    div()
                        .v_flex()
                        .gap_3()
                        .child(t(
                            cx,
                            if profiles.is_empty() {
                                "permissions.no_writable"
                            } else {
                                "permissions.connection"
                            },
                        ))
                        .child(
                            Button::new("permission-connection-picker")
                                .disabled(profiles.is_empty())
                                .label(t(cx, "permissions.choose_connection"))
                                .dropdown_caret(true)
                                .dropdown_menu({
                                    let profiles = profiles.clone();
                                    let cell = cell.clone();
                                    move |mut menu, _, _| {
                                        for profile in &profiles {
                                            let profile = profile.clone();
                                            let cell = cell.clone();
                                            menu = menu.item(
                                                crate::connection_choice::item(&profile).on_click(
                                                    move |_, window, cx| {
                                                        window.close_dialog(cx);
                                                        let _ = cell.update(cx, |this, cx| {
                                                            if this.working(cx)
                                                                || this.result_pending(cx)
                                                            {
                                                                feedback::error(
                                                                    t(cx, "permissions.busy"),
                                                                    cx,
                                                                );
                                                                return;
                                                            }
                                                            match &mut this.cell.content {
                                                                CellContent::Sql {
                                                                    connection_id,
                                                                    ..
                                                                }
                                                                | CellContent::SqlScriptText {
                                                                    connection_id,
                                                                    ..
                                                                }
                                                                | CellContent::SqlScript {
                                                                    connection_id,
                                                                    ..
                                                                } => {
                                                                    *connection_id =
                                                                        Some(profile.id)
                                                                }
                                                                _ => return,
                                                            }
                                                            this.database.update(
                                                                cx,
                                                                |field, cx| {
                                                                    field.set_value(
                                                                        profile.database.clone(),
                                                                        window,
                                                                        cx,
                                                                    )
                                                                },
                                                            );
                                                            this.revision += 1;
                                                            this.database_names.clear();
                                                            this.picker_context = None;
                                                            if let Some(script) = &this.script {
                                                                script.update(cx, |script, cx| {
                                                                    script.scope_changed(cx)
                                                                });
                                                            }
                                                            if let Some(results) = &this.results {
                                                                results.update(
                                                                    cx,
                                                                    |results, cx| {
                                                                        results.stale_result(cx)
                                                                    },
                                                                );
                                                            }
                                                            cx.emit(CellEvent::Changed);
                                                            cx.notify();
                                                        });
                                                    },
                                                ),
                                            );
                                        }
                                        menu.scrollable(true).min_w(px(360.)).max_h(px(280.))
                                    }
                                }),
                        )
                        .child(feedback::actions().child(feedback::close_button(cx))),
                )
        });
    }
    pub fn set_access_mode(
        &mut self,
        mode: turbodbn_services::access_mode::AccessMode,
        cx: &mut Context<Self>,
    ) {
        self.access_mode = mode;
        if let Some(batch) = &self.batch_view {
            batch.update(cx, |batch, cx| batch.set_access_mode(mode, cx));
        }
        if let Some(script) = &self.script {
            script.update(cx, |script, cx| script.set_access_mode(mode, cx));
        }
        if let Some(results) = &self.results {
            results.update(cx, |results, cx| results.set_note_access_mode(mode, cx));
        }
        cx.notify();
    }
    pub fn set_defaults(
        &mut self,
        connection: Option<Uuid>,
        database: String,
        cx: &mut Context<Self>,
    ) {
        let affected = self.inherits_connection()
            && (self.inherited_connection != connection
                || self.database.read(cx).value().trim().is_empty()
                    && self.inherited_database != database);
        self.inherited_connection = connection;
        self.inherited_database = database;
        self.auto_connect(cx);
        if affected {
            if self.editing {
                self.publish_ddl_context(false, cx);
            }
            if let Some(script) = &self.script {
                script.update(cx, |script, cx| script.scope_changed(cx));
                self.script_running = false;
                self.revision += 1;
            } else {
                self.stop(cx);
            }
            self.database_names.clear();
            self.picker_context = None;
            if let Some(results) = &self.results {
                results.update(cx, |results, cx| results.stale_result(cx));
            }
            cx.notify();
        }
    }
    pub fn set_note_path(&mut self, path: std::path::PathBuf, cx: &mut Context<Self>) {
        if let Some(script) = &self.script {
            script.update(cx, |script, _| script.path = path.clone());
        }
        self.note_path = path;
    }
    fn start_script(
        &mut self,
        selected: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.script_running || self.batch_locked && self.batch_token.is_none() {
            return false;
        }
        let Some(script) = self.script.clone().or_else(|| self.batch_view.clone()) else {
            return false;
        };
        let (saved, database) = match self.binding(cx) {
            Ok(binding) => binding,
            Err(error) => {
                feedback::error(error, cx);
                return false;
            }
        };
        let (active, run) = script.update(cx, |script, cx| {
            let active = script.run(saved, database, selected, window, cx);
            (active, script.generation())
        });
        self.script_running = active;
        self.script_run_id = run;
        cx.notify();
        active
    }
    fn render_script(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let name = if let CellContent::SqlScript { script, .. } = &self.cell.content {
            script.name.clone()
        } else {
            String::new()
        };
        div()
            .id(SharedString::from(format!("note-script-{}", self.cell.id)))
            .v_flex()
            .gap_2()
            .p_2()
            .min_w_0()
            .border_1()
            .border_color(cx.theme().border)
            .track_focus(&self.focus)
            .relative()
            .child(self.toolbar_measure(cx))
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .items_center()
                    .child(self.collapse_button(cx))
                    .child(gpui_component::Icon::new(IconName::File))
                    .child(div().text_xs().child(t(cx, "notes.quick.sql_file")))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(name.clone())
                            .id("note-script-name")
                            .tooltip(move |window, cx| {
                                gpui_component::tooltip::Tooltip::new(name.clone())
                                    .build(window, cx)
                            }),
                    )
                    .child(self.scope_controls(window, cx))
                    .child(
                        crate::assets::note_tool("sql-file-more")
                            .icon(IconName::Ellipsis)
                            .tooltip(t(cx, "notes.cell_more"))
                            .dropdown_menu({
                                let owner = cx.entity().downgrade();
                                move |mut menu, _, cx| {
                                    for (key, action) in [
                                        ("notes.file.replace", 0),
                                        ("script.copy_path", 1),
                                        ("notes.cell_copy", 2),
                                        ("notes.remove_cell", 3),
                                    ] {
                                        let target = owner.clone();
                                        let locked = owner
                                            .read_with(cx, |v, cx| v.working(cx) || v.batch_locked)
                                            .unwrap_or(true);
                                        menu = menu.item(
                                            PopupMenuItem::new(t(cx, key))
                                                .disabled(locked && action != 1)
                                                .on_click(move |_, _, cx| {
                                                    let target = target.clone();
                                                    cx.defer(move |cx| {
                                                        let _ = target.update(cx, |this, cx| {
                                                            match action {
                                                                0 => cx.emit(
                                                                    CellEvent::ReplaceSqlFile,
                                                                ),
                                                                1 => {
                                                                    if let Some(script) =
                                                                        &this.script
                                                                    {
                                                                        script.update(
                                                                            cx,
                                                                            |script, cx| {
                                                                                script.copy_path(cx)
                                                                            },
                                                                        );
                                                                    }
                                                                }
                                                                2 => cx.emit(CellEvent::Insert {
                                                                    after: true,
                                                                    copy: true,
                                                                }),
                                                                _ => cx.emit(CellEvent::Remove),
                                                            }
                                                        });
                                                    });
                                                }),
                                        );
                                    }
                                    menu
                                }
                            }),
                    ),
            )
            .when(!self.collapsed, |el| {
                el.child(self.script.as_ref().unwrap().clone())
            })
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key.eq_ignore_ascii_case("f5") && !this.batch_locked {
                    this.start_script(false, window, cx);
                    cx.stop_propagation();
                }
            }))
            .into_any_element()
    }
    pub fn has_sql(&self, cx: &App) -> bool {
        self.script.is_some()
            || self.cell.content.is_editable_sql()
                && !self.editor.read(cx).value().trim().is_empty()
    }
    pub fn batch_scope(&self, cx: &App) -> Result<Option<BatchScope>, String> {
        if self.script.is_none()
            && (!self.cell.content.is_editable_sql()
                || self.editor.read(cx).value().trim().is_empty())
        {
            return Ok(None);
        }
        if self.working(cx) || self.result_pending(cx) {
            return Err(t(cx, "grid.pending").to_string());
        }
        let (connection, _) = self.binding(cx)?;
        if let CellContent::SqlScript { script, .. } = &self.cell.content {
            if script.unsupported != 0 {
                return Err(t(cx, "script.error.unsupported").to_string());
            }
            if turbodbn_services::sql_script::dialect(connection.profile.kind)
                != Some(script.dialect)
            {
                return Err(t(cx, "script.error.dialect").to_string());
            }
        }
        Ok(Some(BatchScope {
            revision: self.revision,
            connection,
        }))
    }
    pub fn start_batch(
        &mut self,
        token: Uuid,
        scope: BatchScope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.revision != scope.revision
            || !self
                .connections
                .read(cx)
                .connection_snapshot_current(&scope.connection)
        {
            return false;
        }
        self.batch_token = Some(token);
        self.run(0, window, cx);
        if !self.busy() {
            self.finish_batch(false, cx);
            return false;
        }
        true
    }
    fn finish_batch(&mut self, success: bool, cx: &mut Context<Self>) {
        if let Some(token) = self.batch_token.take() {
            cx.emit(CellEvent::BatchFinished { token, success });
        }
    }
    pub fn set_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        self.collapsed = collapsed;
        if collapsed {
            self.editing = false;
            self.completion.cancel();
        }
        cx.notify();
    }
    pub(crate) fn set_editor_height_limit(&mut self, height: f32, cx: &mut Context<Self>) {
        if (self.editor_height_limit - height).abs() >= 1. {
            self.editor_height_limit = height;
            if self.editing && self.cell.content.is_editable_sql() {
                cx.notify();
            }
        }
    }
    fn editor_max_lines(&self) -> usize {
        ((self.editor_height_limit - 16.) / 22.).ceil().max(3.) as usize
    }
    fn collapse_button(&self, cx: &Context<Self>) -> Button {
        crate::assets::note_tool("toggle-note-cell-collapse")
            .icon(gpui_component::Icon::default().path(if self.collapsed {
                "icons/note-expand.svg"
            } else {
                "icons/note-collapse.svg"
            }))
            .tooltip(t(
                cx,
                if self.collapsed {
                    "notes.expand_cell"
                } else {
                    "notes.collapse_cell"
                },
            ))
            .on_click(cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.set_collapsed(!this.collapsed, cx);
            }))
    }
    fn clear_sql_button(&self, cx: &Context<Self>) -> Button {
        let markdown = matches!(self.cell.content, CellContent::Markdown { .. });
        crate::assets::note_tool("clear-note-sql")
            .icon(
                gpui_component::Icon::default()
                    .path("icons/note-clear.svg")
                    .text_color(crate::assets::note_color(4, cx)),
            )
            .tooltip(t(
                cx,
                if markdown {
                    "notes.clear_md"
                } else {
                    "notes.clear_sql"
                },
            ))
            .disabled(
                self.working(cx) || self.batch_locked || self.editor.read(cx).text().len() == 0,
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                if this.working(cx) || this.batch_locked || this.editor.read(cx).text().len() == 0 {
                    return;
                }
                let revision = this.revision;
                let owner = cx.entity().downgrade();
                window.defer(cx, move |window, cx| {
                    feedback::confirm(
                        t(
                            cx,
                            if markdown {
                                "notes.clear_md"
                            } else {
                                "notes.clear_sql"
                            },
                        ),
                        t(
                            cx,
                            if markdown {
                                "notes.clear_md_body"
                            } else {
                                "notes.clear_sql_body"
                            },
                        ),
                        window,
                        cx,
                        move |window, cx| {
                            let _ = owner.update(cx, |this, cx| {
                                if this.revision != revision
                                    || !this.editing
                                    || !this.note_active
                                    || this.working(cx)
                                    || this.batch_locked
                                {
                                    feedback::error(
                                        t(
                                            cx,
                                            if markdown {
                                                "notes.clear_md_changed"
                                            } else {
                                                "notes.clear_sql_changed"
                                            },
                                        ),
                                        cx,
                                    );
                                    return;
                                }
                                this.completion.cancel();
                                this.editor.update(cx, |input, cx| {
                                    let length = input.value().encode_utf16().count();
                                    input.replace_text_in_range(Some(0..length), "", window, cx);
                                    input.focus(window, cx);
                                });
                                feedback::success(
                                    t(
                                        cx,
                                        if markdown {
                                            "notes.clear_md_done"
                                        } else {
                                            "notes.clear_sql_done"
                                        },
                                    ),
                                    cx,
                                );
                            });
                        },
                    );
                });
            }))
    }
    pub fn set_batch_locked(&mut self, locked: bool, cx: &mut Context<Self>) {
        if let Some(script) = &self.script {
            script.update(cx, |script, cx| script.set_locked(locked, cx));
        }
        if self.batch_locked != locked {
            self.batch_locked = locked;
            cx.notify();
        }
    }
    fn ai_button(&self, cx: &Context<Self>) -> impl IntoElement {
        crate::assets::note_tool("note-send-ai")
            .icon(
                gpui_component::Icon::default()
                    .path("icons/note-ai.svg")
                    .text_color(crate::assets::note_color(2, cx)),
            )
            .tooltip(t(cx, "notes.ai_draft"))
            .disabled(self.editor.read(cx).value().trim().is_empty())
            .on_click(cx.listener(|this, _, _, cx| {
                this.ask_ai(cx);
            }))
    }
    fn ask_ai(&self, cx: &mut Context<Self>) {
        let (kind, connection, database) = self.assist_context(cx);
        let name = connection
            .and_then(|id| {
                self.connections
                    .read(cx)
                    .note_profile(id)
                    .map(|p| p.name.clone())
            })
            .unwrap_or_default();
        let request = crate::feedback::SqlDraft {
            sql: self.editor.read(cx).value().to_string(),
            scope: crate::feedback::SqlScope {
                connection,
                name,
                database,
                kind: kind
                    .map(|kind| kind.label().to_string())
                    .unwrap_or_default(),
            },
        };
        let hub = cx.global::<crate::feedback::Hub>().0.clone();
        cx.defer(move |cx| hub.update(cx, |_, cx| cx.emit(request)));
    }
    fn mode_button(&self, cx: &Context<Self>) -> impl IntoElement {
        let editing = self.editing;
        crate::assets::note_tool("cell-mode")
            .selected(editing)
            .icon(
                gpui_component::Icon::default()
                    .path(if editing {
                        "icons/note-read-mode.svg"
                    } else {
                        "icons/note-edit-mode.svg"
                    })
                    .text_color(crate::assets::note_color(if editing { 1 } else { 0 }, cx)),
            )
            .tooltip(t(
                cx,
                if editing {
                    "notes.read_mode"
                } else {
                    "notes.edit_mode"
                },
            ))
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                if editing {
                    this.set_editing(false, window, cx);
                } else {
                    cx.emit(CellEvent::Edit);
                }
            }))
    }
    fn document_controls(&self, cx: &Context<Self>) -> impl IntoElement {
        let owner = cx.entity().downgrade();
        crate::result_document::switches(
            self.json_mode,
            self.busy(),
            cx,
            move |json, window, cx| {
                let _ = owner.update(cx, |this, cx| {
                    this.json_mode = json;
                    if json {
                        let rows = this
                            .execution_summary
                            .as_ref()
                            .map(ExecutionSummary::json_rows)
                            .unwrap_or_else(|| this.grid.read(cx).delegate().rows.clone());
                        this.document = Some(cx.new(|cx| {
                            crate::result_document::ResultDocument::new(rows, window, cx)
                        }));
                    } else {
                        this.document = None;
                    }
                    cx.notify();
                });
            },
        )
    }
    fn copy_source_button(&self, cx: &Context<Self>) -> impl IntoElement {
        crate::assets::note_tool("copy-note-source")
            .icon(IconName::Copy)
            .tooltip(t(cx, "notes.copy_content"))
            .on_click(cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                cx.write_to_clipboard(ClipboardItem::new_string(
                    this.task_list
                        .as_ref()
                        .map(|task| task.read(cx).text())
                        .unwrap_or_else(|| this.editor.read(cx).value().to_string()),
                ));
                feedback::success(t(cx, "feedback.copied"), cx);
            }))
    }
    pub(crate) fn set_note_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if !active {
            self.completion.cancel();
        }
        if !active && self.note_active && self.editing {
            feedback::ddl_context(
                feedback::DdlContext {
                    note: None,
                    candidates: vec![],
                    origin: Some(self.cell.id),
                    scope: None,
                    object: None,
                    open: false,
                },
                cx,
            );
        }
        self.note_active = active;
        if active && self.editing {
            self.publish_ddl_context(false, cx);
        }
    }
    pub fn set_editing(&mut self, editing: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing == editing {
            return;
        }
        self.editing = editing;
        if !editing {
            self.completion.cancel();
        }
        self.publish_ddl_context(false, cx);
        if editing {
            self.auto_connect(cx);
            self.collapsed = false;
        }
        if editing {
            self.editor.update(cx, |input, cx| input.focus(window, cx));
        } else {
            self.focus.focus(window);
        }
        cx.notify();
    }
    pub fn set_position(&mut self, index: usize, count: usize, cx: &mut Context<Self>) {
        if self.position != (index, count) {
            self.position = (index, count);
            cx.notify();
        }
    }
    fn refresh_database_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut names = self
            .connection_id()
            .map(|id| self.connections.read(cx).note_databases(id))
            .unwrap_or_default();
        names.extend(self.database_names.clone());
        let value = self.database.read(cx).value().to_string();
        if !value.is_empty() {
            names.push(value.clone());
        }
        if let Some(id) = self.connection_id() {
            names.retain(|name| self.connections.read(cx).note_database_visible(id, name));
        }
        names.sort();
        names.dedup();
        names.truncate(2000);
        if self.picker_context.as_ref() == Some(&(names.clone(), value.clone())) {
            return;
        }
        self.picker_context = Some((names.clone(), value.clone()));
        self.database_picker.update(cx, |picker, cx| {
            picker.set_items(
                SearchableVec::new(names.into_iter().map(DatabaseChoice).collect::<Vec<_>>()),
                window,
                cx,
            );
            picker.set_selected_value(&value, window, cx);
        });
    }
    fn detect_databases(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.discovery.is_some() || self.busy() {
            return;
        }
        let (_, connection, database) = self.assist_context(cx);
        let Some(connection) = connection else {
            feedback::error(t(cx, "notes.choose_connection"), cx);
            return;
        };
        let saved = match self.connections.read(cx).saved_connection(connection, cx) {
            Ok(saved) => saved,
            Err(error) => {
                feedback::error(error, cx);
                return;
            }
        };
        if saved.profile.kind == DatabaseKind::Sqlite {
            return;
        }
        self.discovery_generation += 1;
        let generation = self.discovery_generation;
        self.discovery_snapshot = Some(saved.clone());
        let id = Uuid::new_v4();
        let started = std::time::Instant::now();
        self.log(id, crate::operation_log::Phase::Started, 0, 0, None, cx);
        let endpoint = cx.background_executor().spawn(async move {
            let mut endpoint = saved.endpoint(database)?;
            endpoint.profile.list_all_databases = true;
            Ok::<_, String>(endpoint)
        });
        self.discovery = Some(cx.spawn_in(window, async move |view, cx| {
            let result = match endpoint.await {
                Err(error) => Err(error),
                Ok(endpoint) => {
                    let (_session, events) =
                        Session::start(endpoint.profile, endpoint.password, true);
                    match events.recv().await {
                        Ok(Event::Tested(info)) => Ok(info),
                        Ok(Event::Failed(error)) => Err(error),
                        _ => Err("数据库列表读取失败".into()),
                    }
                }
            };
            let _ = cx.update(|window, cx| {
                view.update(cx, |this, cx| {
                    if generation != this.discovery_generation {
                        return;
                    }
                    this.discovery = None;
                    let current = this.discovery_snapshot.as_ref().is_some_and(|s| {
                        this.connection_id() == Some(s.profile.id)
                            && this.connections.read(cx).connection_snapshot_current(s)
                    });
                    if !current {
                        this.log(
                            id,
                            crate::operation_log::Phase::Cancelled,
                            started.elapsed().as_millis() as u64,
                            0,
                            None,
                            cx,
                        );
                        cx.notify();
                        return;
                    }
                    match result {
                        Ok(info) => {
                            this.database_names = info.databases.into_iter().take(2000).collect();
                            this.refresh_database_picker(window, cx);
                            if let Some(error) = info.database_list_error {
                                feedback::error(error, cx);
                            }
                            this.log(
                                id,
                                crate::operation_log::Phase::Succeeded,
                                started.elapsed().as_millis() as u64,
                                this.database_names.len() as u64,
                                None,
                                cx,
                            );
                        }
                        Err(error) => {
                            this.log(
                                id,
                                crate::operation_log::Phase::Failed,
                                started.elapsed().as_millis() as u64,
                                0,
                                Some(&error),
                                cx,
                            );
                            feedback::error(error, cx);
                        }
                    }
                    cx.notify();
                })
            });
        }));
        cx.notify();
    }
    pub fn snapshot(&self, cx: &App) -> Cell {
        self.snapshot_deferred(cx).finish()
    }
    pub(crate) fn snapshot_deferred(&self, cx: &App) -> CellSnapshot {
        let mut cell = self.cell.clone();
        let mut text = None;
        cell.title = self.title.read(cx).value().to_string();
        match &mut cell.content {
            CellContent::SqlScript { database, .. } => {
                let value = self.database.read(cx).value().trim().to_owned();
                *database = (!value.is_empty()).then_some(value);
            }
            CellContent::Sql {
                source, database, ..
            }
            | CellContent::SqlScriptText {
                source, database, ..
            } => {
                *source = String::new();
                text = Some(self.editor.read(cx).text().clone());
                let value = self.database.read(cx).value().trim().to_owned();
                *database = (!value.is_empty()).then_some(value);
            }
            CellContent::Markdown { source } => {
                *source = String::new();
                text = Some(self.editor.read(cx).text().clone());
            }
            CellContent::Task {
                description,
                items,
                status,
                ..
            } => {
                if let Some(task) = &self.task_list {
                    let snapshot = task.read(cx).snapshot();
                    *items = snapshot.0;
                    *description = snapshot.1;
                    *status = snapshot.2;
                } else {
                    *description = self.editor.read(cx).value().to_string();
                }
            }
        }
        CellSnapshot { cell, source: text }
    }
    pub fn working(&self, cx: &App) -> bool {
        self.busy()
            || self
                .script
                .as_ref()
                .is_some_and(|s| s.read(cx).is_indexing())
            || self.awaiting_preview
            || self
                .results
                .as_ref()
                .is_some_and(|r| r.read(cx).result_busy())
    }
    pub fn indexing(&self, cx: &App) -> bool {
        self.script
            .as_ref()
            .is_some_and(|s| s.read(cx).is_indexing())
    }
    pub fn result_pending(&self, cx: &App) -> bool {
        self.results
            .as_ref()
            .is_some_and(|r| r.read(cx).result_pending())
    }
    pub fn discard_result_changes(&mut self, cx: &mut Context<Self>) {
        if let Some(results) = &self.results {
            results.update(cx, |results, cx| results.cancel_result(true, cx));
        }
    }
    pub fn busy(&self) -> bool {
        self.batch_preparing.is_some()
            || self.script_running
            || matches!(self.query.status, QueryStatus::Running)
    }
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        self.batch_preparing = None;
        if let Some(view) = &self.batch_view {
            view.update(cx, |v, cx| v.stop(cx));
        }
        self.script_running = false;
        if let Some(script) = &self.script {
            script.update(cx, |script, cx| script.stop(cx));
            self.script_running = false;
        }
        self.revision += 1;
        self.awaiting_preview = false;
        self.preview_task = None;
        self.finish_batch(false, cx);
        if let Some(results) = &self.results {
            results.update(cx, |results, cx| results.cancel_result(false, cx));
        }
        if let Some((id, started)) = self.operation.take() {
            if let Some(operation) = self.executing_write.take() {
                self.set_execution_summary(
                    Some(ExecutionSummary {
                        operation,
                        status: "cancelled".into(),
                        affected_rows: None,
                        matched_rows: None,
                        changed_rows: None,
                        elapsed_ms: started.elapsed().as_millis() as u64,
                        count_source: None,
                        ..Default::default()
                    }),
                    cx,
                );
            }
            self.log(
                id,
                crate::operation_log::Phase::Cancelled,
                started.elapsed().as_millis() as u64,
                0,
                None,
                cx,
            );
        }
        if !self.busy() {
            cx.notify();
            return;
        }
        self.query.cancel();
        self.session = None;
        self.task = None;
        self.notice = t(cx, "notes.stopped").to_string();
        cx.notify();
    }
    fn connection_id(&self) -> Option<Uuid> {
        match self.cell.content {
            CellContent::Sql { connection_id, .. }
            | CellContent::SqlScriptText { connection_id, .. }
            | CellContent::SqlScript { connection_id, .. } => {
                connection_id.or(self.inherited_connection)
            }
            _ => None,
        }
    }
    fn binding(&self, cx: &App) -> Result<(SavedConnection, String), String> {
        let id = self
            .connection_id()
            .ok_or_else(|| t(cx, "notes.choose_connection").to_string())?;
        let saved = self.connections.read(cx).saved_connection(id, cx)?;
        if !matches!(
            saved.profile.kind,
            DatabaseKind::MySql | DatabaseKind::PostgreSql | DatabaseKind::Sqlite
        ) {
            return Err(t(cx, "notes.sql_driver").to_string());
        }
        let db = self.database.read(cx).value().trim().to_string();
        let db = if saved.profile.kind == DatabaseKind::Sqlite {
            saved.database_label()
        } else if !db.is_empty() {
            db
        } else if self.inherits_connection() && !self.inherited_database.is_empty() {
            self.inherited_database.clone()
        } else {
            saved.database_label()
        };
        Ok((saved, db))
    }
    fn run(&mut self, offset: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.script.is_some() {
            self.start_script(false, window, cx);
            return;
        }
        self.policy_help = false;
        if self.result_pending(cx) {
            feedback::error(t(cx, "grid.pending"), cx);
            return;
        }
        if self.working(cx) {
            return;
        }
        if offset > 0
            && (self.last_revision != self.revision
                || self
                    .snapshot
                    .as_ref()
                    .is_none_or(|s| !self.connections.read(cx).connection_snapshot_current(s)))
        {
            feedback::error(t(cx, "notes.results_stale"), cx);
            return;
        }
        let size = self
            .page_size
            .read(cx)
            .value()
            .parse::<usize>()
            .unwrap_or(0);
        if !(1..=1000).contains(&size) {
            feedback::error(t(cx, "notes.page_size_invalid"), cx);
            return;
        }
        let (saved, db) = match self.binding(cx) {
            Ok(v) => v,
            Err(e) => {
                feedback::error(e, cx);
                return;
            }
        };
        let sql = if offset > 0 {
            self.executed_sql.clone()
        } else {
            self.execution_sql(false, window, cx)
        };
        let Some(sql) = sql else {
            return;
        };
        if offset == 0 {
            self.batch_view = None;
            self.batch_subscription = None;
            self.executed_sql = Some(sql.clone());
        }
        self.query.page_size = size;
        self.request(saved, db, Action::NotePrepare { sql }, offset, window, cx);
    }
    fn explain(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.working(cx) || self.batch_locked {
            return;
        }
        if self.result_pending(cx) {
            feedback::error(t(cx, "grid.pending"), cx);
            return;
        }
        let Some(sql) = self.execution_sql(true, window, cx) else {
            return;
        };
        match self.binding(cx) {
            Ok((saved, db)) => {
                self.request(saved, db, Action::NoteExplain { sql }, 0, window, cx);
            }
            Err(error) => feedback::error(error, cx),
        }
    }
    fn request(
        &mut self,
        saved: SavedConnection,
        database: String,
        action: Action,
        offset: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .connections
            .read(cx)
            .connection_snapshot_current(&saved)
        {
            feedback::error(t(cx, "transfer.connection_changed"), cx);
            self.finish_batch(false, cx);
            return;
        }
        if matches!(action, Action::NoteExplain { .. })
            && !cx
                .global::<crate::preferences::Preferences>()
                .database
                .command_enabled("explain")
        {
            feedback::error(t(cx, "settings.sql_policy_blocked"), cx);
            self.finish_batch(false, cx);
            return;
        }
        let sql = match &action {
            Action::NotePrepare { sql } => Some(sql.as_str()),
            Action::NoteExecute { plan, .. } => Some(plan.sql.as_str()),
            _ => None,
        };
        if let Some(advice) = sql.and_then(|sql| {
            crate::note_permissions::Advice::for_sql(
                self.access_mode,
                &saved.profile,
                sql,
                false,
                cx,
            )
        }) {
            self.finish_batch(false, cx);
            cx.emit(CellEvent::PermissionBlocked(advice));
            return;
        }
        if self.execution_summary.take().is_some() {
            self.document = None;
            self.json_mode = false;
        }
        self.executing_write = match &action {
            Action::NoteExecute { plan, .. } if !plan.read_only => Some(
                turbodbn_services::database::notebook_query::command_required(
                    saved.profile.kind,
                    &plan.sql,
                )
                .unwrap_or("command")
                .to_ascii_uppercase(),
            ),
            _ => None,
        };
        let token = self.query.begin();
        self.operation = Some((token.request_id, std::time::Instant::now()));
        self.elapsed = 0;
        let revision = self.revision;
        let size = self.query.page_size;
        self.snapshot = Some(saved.clone());
        self.notice = t(cx, "notes.running").to_string();
        self.more = false;
        let prepare = matches!(action, Action::NotePrepare { .. });
        let execution = match &action {
            Action::NoteExecute {
                plan, confirmed, ..
            } => Some((plan.clone(), *confirmed)),
            _ => None,
        };
        self.log(
            token.request_id,
            crate::operation_log::Phase::Started,
            0,
            0,
            None,
            cx,
        );
        let endpoint = {
            let saved = saved.clone();
            let database = database.clone();
            cx.background_executor()
                .spawn(async move { saved.endpoint(database) })
        };
        self.task = Some(cx.spawn_in(window, async move |view, cx| {
            let endpoint = match endpoint.await {
                Ok(v) => v,
                Err(error) => {
                    let _ = view.update(cx, |this, cx| this.failed(token, error, cx));
                    return;
                }
            };
            let (session, events) = Session::start(endpoint.profile, endpoint.password, false);
            let accepted = view
                .update(cx, |this, _| {
                    if !this.query.accepts(token) {
                        return false;
                    }
                    this.session = Some(session);
                    true
                })
                .unwrap_or(false);
            if !accepted {
                return;
            }
            while let Ok(event) = events.recv().await {
                let done = matches!(
                    event,
                    Event::Operation { .. } | Event::Failed(_) | Event::Disconnected
                );
                let ok = cx.update(|window, cx| {
                    view.update(cx, |this, cx| {
                        if !this.query.accepts(token) {
                            return;
                        }
                        if this.revision != revision {
                            this.failed(token, t(cx, "notes.request_changed").to_string(), cx);
                            return;
                        }
                        if !this
                            .connections
                            .read(cx)
                            .connection_snapshot_current(&saved)
                        {
                            this.stop(cx);
                            return;
                        }
                        match event {
                            Event::Connected(_) => {
                                let result = this.session.as_ref().unwrap().request(Operation {
                                    id: 1,
                                    kind: OperationKind::Workbench(Box::new(Request {
                                        database: database.clone(),
                                        action: action.clone(),
                                    })),
                                });
                                if let Err(error) = result {
                                    this.failed(token, error, cx);
                                }
                            }
                            Event::Operation { result, .. } => {
                                this.session = None;
                                match result {
                                    Ok(OperationResult::Workbench(reply)) => match *reply {
                                        Reply::NotePrepared(plan) if prepare => {
                                            this.query.complete(token, Ok(()));
                                            if let Some((id, started)) = this.operation.take() {
                                                this.log(
                                                    id,
                                                    crate::operation_log::Phase::Succeeded,
                                                    started.elapsed().as_millis() as u64,
                                                    0,
                                                    None,
                                                    cx,
                                                );
                                            }
                                            this.notice = t(cx, "notes.prepared").to_string();
                                            let saved = saved.clone();
                                            let db = database.clone();
                                            cx.defer_in(window, move |this, window, cx| {
                                                this.prepared(
                                                    (saved, db),
                                                    plan,
                                                    offset,
                                                    revision,
                                                    window,
                                                    cx,
                                                )
                                            });
                                        }
                                        Reply::NoteResult(result) => {
                                            this.query.complete(token, Ok(()));
                                            this.last_revision = revision;
                                            this.last_offset = result.offset;
                                            this.more = result.more;
                                            this.elapsed = result.elapsed_ms;
                                            this.operation = None;
                                            let rows = result.rows.values.len() as u64;
                                            let read_only = result.read_only;
                                            let affected = result.affected_rows;
                                            this.executing_write = None;
                                            this.set_execution_summary(result.execution.as_deref().cloned(), cx);
                                            let elapsed_ms = result.elapsed_ms;
                                            if let Some(meta) = &result.grid
                                                && let Some(object) = &meta.object
                                            {
                                                this.connections.update(cx, |db, _| {
                                                    db.remember_note_columns(
                                                        saved.profile.id,
                                                        &database,
                                                        object,
                                                        &meta.schema.columns,
                                                    )
                                                });
                                            }
                                            if result.read_only
                                                && result.grid.is_some()
                                                && let Some((plan, confirmed)) = &execution
                                            {
                                                let view = cx.new(|cx| {
                                                    crate::table_view::TableView::from_note(
                                                        this.connections.downgrade(),
                                                        (saved.clone(), database.clone()),
                                                        (plan.clone(), *confirmed, result),
                                                        window,
                                                        cx,
                                                    )
                                                });
                                                this._results_permission=Some(cx.subscribe(&view,|_,_,advice:&crate::note_permissions::Advice,cx|cx.emit(CellEvent::PermissionBlocked(advice.clone()))));
                                                this._results_clear = Some(cx.subscribe(&view, |_, _, _: &crate::table_view::ClearNoteResults, cx| {
                                                    let owner = cx.entity().downgrade();
                                                    cx.defer(move |cx| { let _ = owner.update(cx, |this, cx| this.clear_results(cx)); });
                                                }));
                                                let mut result_state = {
                                                    let result = view.read(cx);
                                                    (result.result_busy(), result.result_pending())
                                                };
                                                this._results_observer = Some(cx.observe(&view, move |_, view, cx| {
                                                    let result = view.read(cx);
                                                    let current = (result.result_busy(), result.result_pending());
                                                    if current != result_state {
                                                        result_state = current;
                                                        cx.notify();
                                                    }
                                                }));
                                                this.grid.update(cx, |grid, cx| {
                                                    *grid.delegate_mut() =
                                                        QueryGrid::new(Rows::default(), 0);
                                                    grid.refresh(cx);
                                                });
                                                this.document = None;
                                                this.json_mode = false;
                                                view.update(cx,|view,cx|view.set_note_access_mode(this.access_mode,cx));
                                                this.results = Some(view);
                                            } else {
                                                this.results = None;
                                                this._results_observer = None;
                                                this._results_clear = None;
                                                this.json_mode = result.read_only && crate::result_document::single_json(&result.rows);
                                                this.document = if this.json_mode { Some(cx.new(|cx| crate::result_document::ResultDocument::new(result.rows.clone(), window, cx))) } else { None };
                                                this.grid.update(cx, |grid, cx| {
                                                    *grid.delegate_mut() =
                                                        QueryGrid::new(result.rows, result.offset);
                                                    grid.refresh(cx);
                                                });
                                            }
                                            this.notice = if read_only {
                                                crate::locale::tr(
                                                    cx,
                                                    "notes.query_rows",
                                                    &[("count", &rows.to_string())],
                                                )
                                                .to_string()
                                            } else {
                                                crate::locale::tr(cx, "notes.command_result", &[("count", &affected.map(|n| n.to_string()).unwrap_or_else(|| t(cx, "output.operation.unavailable").to_string())), ("elapsed", &crate::duration::elapsed(elapsed_ms))]).to_string()
                                            };
                                            if !read_only {
                                                feedback::success(this.notice.clone(), cx);
                                            }
                                            if let Some(log) = this.connections.read(cx).output.clone() {
                                                let mut record = crate::operation_log::Record::new(token.request_id, crate::operation_log::Kind::Query, crate::operation_log::Phase::Succeeded);
                                                record.source = Some(saved.profile.id);
                                                record.context = this.log_context(cx);
                                                record.elapsed_ms = elapsed_ms;
                                                record.rows = rows;
                                                record.affected_rows = affected;
                                                record.command = !read_only;
                                                log.update(cx, |log, cx| log.operation(record, cx));
                                            }
                                            this.finish_batch(true, cx);
                                        }
                                        _ => this.failed(
                                            token,
                                            t(cx, "notes.invalid_response").to_string(),
                                            cx,
                                        ),
                                    },
                                    Err(error) => this.failed(token, error, cx),
                                    _ => this.failed(
                                        token,
                                        t(cx, "notes.invalid_response").to_string(),
                                        cx,
                                    ),
                                }
                            }
                            Event::Failed(error) => this.failed(token, error, cx),
                            Event::Disconnected => {
                                if this.busy() {
                                    this.failed(token, t(cx, "notes.disconnected").to_string(), cx);
                                }
                            }
                            _ => {}
                        }
                        this.query.page_size = size;
                        cx.notify();
                    })
                });
                if ok.is_err() || done {
                    break;
                }
            }
        }));
        cx.notify();
    }
    fn failed(&mut self, token: turbodbn_core::QueryToken, error: String, cx: &mut Context<Self>) {
        self.policy_help = error == "notes.sql_scope_unsupported";
        let error = crate::locale::message(cx, &error).to_string();
        if !self.query.complete(token, Err(error.clone())) {
            return;
        }
        self.session = None;
        let elapsed = self
            .operation
            .take()
            .map(|(_, started)| started.elapsed().as_millis() as u64)
            .unwrap_or(0);
        if let Some(operation) = self.executing_write.take() {
            self.set_execution_summary(
                Some(ExecutionSummary {
                    operation,
                    status: "outcome_unknown".into(),
                    affected_rows: None,
                    matched_rows: None,
                    changed_rows: None,
                    elapsed_ms: elapsed,
                    count_source: None,
                    ..Default::default()
                }),
                cx,
            );
        }
        self.notice = error.clone();
        self.log(
            token.request_id,
            crate::operation_log::Phase::Failed,
            elapsed,
            0,
            Some(&error),
            cx,
        );
        feedback::error(error, cx);
        self.finish_batch(false, cx);
        cx.notify();
    }
    fn prepared(
        &mut self,
        binding: (SavedConnection, String),
        plan: Plan,
        offset: usize,
        revision: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (saved, database) = binding;
        if revision != self.revision {
            self.finish_batch(false, cx);
            return;
        }

        if let Some(advice) = crate::note_permissions::Advice::for_sql(
            self.access_mode,
            &saved.profile,
            &plan.sql,
            false,
            cx,
        ) {
            self.finish_batch(false, cx);
            cx.emit(CellEvent::PermissionBlocked(advice));
            return;
        }
        if !plan.read_only
            && saved.profile.sqlite_read_only
            && saved.profile.kind == DatabaseKind::Sqlite
        {
            feedback::error(t(cx, "notes.read_only"), cx);
            self.finish_batch(false, cx);
            return;
        }
        if plan.confirm {
            let view = cx.entity().downgrade();
            let target = format!(
                "{} / {} · {}",
                saved.profile.name, database, saved.profile.environment
            );
            self.awaiting_preview = true;
            let outcome = feedback::sql_preview_observed(
                plan.sql.clone(),
                target,
                window,
                cx,
                move |sql, window, cx| {
                    view.update(cx, |this, cx| {
                        if sql != plan.sql || this.revision != revision {
                            feedback::error(t(cx, "notes.preview_changed"), cx);
                            return false;
                        }
                        this.awaiting_preview = false;
                        this.request(
                            saved.clone(),
                            database.clone(),
                            Action::NoteExecute {
                                plan: plan.clone(),
                                confirmed: true,
                                offset,
                                page_size: this.query.page_size,
                            },
                            offset,
                            window,
                            cx,
                        );
                        this.busy()
                    })
                    .unwrap_or(false)
                },
            );
            self.preview_task = Some(cx.spawn(async move |view, cx| {
                let accepted = outcome.recv().await.unwrap_or(false);
                let _ = view.update(cx, |this, cx| {
                    this.awaiting_preview = false;
                    if !accepted {
                        this.finish_batch(false, cx);
                    }
                    cx.notify();
                });
            }));
        } else {
            self.request(
                saved,
                database,
                Action::NoteExecute {
                    plan,
                    confirmed: false,
                    offset,
                    page_size: self.query.page_size,
                },
                offset,
                window,
                cx,
            );
        }
    }
    pub(crate) fn set_note_name(&mut self, name: &str, cx: &mut Context<Self>) {
        self.note_name = name.into();
        if let Some(task) = &self.task_list {
            task.update(cx, |task, _| task.note_name = name.into());
        }
    }
    fn log_context(&self, cx: &App) -> String {
        let connection = self
            .connection_id()
            .and_then(|id| self.connections.read(cx).note_profile(id))
            .map(|p| p.name.clone())
            .unwrap_or_default();
        let title = self.cell.title.trim();
        format!(
            "{} · {} · #{}{}",
            connection,
            self.note_name,
            self.position.0 + 1,
            if title.is_empty() {
                String::new()
            } else {
                format!(" · {title}")
            }
        )
    }
    fn log(
        &self,
        id: Uuid,
        phase: crate::operation_log::Phase,
        elapsed: u64,
        rows: u64,
        error: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        if let Some(log) = self.connections.read(cx).output.clone() {
            let mut record =
                crate::operation_log::Record::new(id, crate::operation_log::Kind::Query, phase);
            record.source = self.connection_id();
            record.context = self.log_context(cx);
            record.elapsed_ms = elapsed;
            record.rows = rows;
            if let Some(error) = error {
                record.failure = crate::operation_log::failure_class(error);
            }
            log.update(cx, |log, cx| log.operation(record, cx));
        }
    }
    fn set_execution_summary(&mut self, summary: Option<ExecutionSummary>, cx: &mut Context<Self>) {
        if let Some(summary) = &summary {
            self.results = None;
            self._results_observer = None;
            self._results_clear = None;
            self.grid.update(cx, |grid, cx| {
                *grid.delegate_mut() = QueryGrid::new(Rows::default(), 0);
                grid.refresh(cx);
            });
            let unavailable = t(cx, "output.operation.unavailable").to_string();
            let mut rows = Rows {
                columns: vec![
                    t(cx, "result.execution.operation").to_string(),
                    t(cx, "result.execution.status").to_string(),
                    t(cx, "result.execution.affected").to_string(),
                    t(cx, "result.execution.elapsed").to_string(),
                ],
                values: vec![vec![
                    Some(summary.operation.clone()),
                    Some(
                        t(
                            cx,
                            if summary.status == "succeeded" {
                                "output.operation.succeeded"
                            } else if summary.status == "cancelled" {
                                "result.execution.cancelled"
                            } else {
                                "result.execution.unknown"
                            },
                        )
                        .to_string(),
                    ),
                    Some(
                        summary
                            .affected_rows
                            .map(|n| n.to_string())
                            .unwrap_or_else(|| unavailable.clone()),
                    ),
                    Some(crate::duration::elapsed(summary.elapsed_ms)),
                ]],
            };
            for (key, count) in [
                ("result.execution.matched", summary.matched_rows),
                ("result.execution.changed", summary.changed_rows),
            ] {
                if let Some(count) = count {
                    rows.columns.push(t(cx, key).to_string());
                    rows.values[0].push(Some(count.to_string()));
                }
            }
            self.execution_grid.update(cx, |grid, cx| {
                *grid.delegate_mut() = QueryGrid::new(rows, 0);
                grid.refresh(cx);
            });
            self.document = None;
            self.json_mode = false;
        }
        self.execution_summary = summary;
    }
    fn copy(&self, cx: &mut Context<Self>) {
        if let Some(value) = self.grid.read(cx).delegate().selected_value() {
            cx.write_to_clipboard(ClipboardItem::new_string(value));
            feedback::success(t(cx, "feedback.copied"), cx);
        }
    }
}
impl CellView {
    fn clear_results(&mut self, cx: &mut Context<Self>) {
        self.execution_details = false;
        if self.working(cx) || self.batch_locked {
            return;
        }
        if self.result_pending(cx) {
            feedback::error(t(cx, "grid.pending"), cx);
            return;
        }
        self.execution_summary = None;
        self.execution_grid.update(cx, |grid, cx| {
            *grid.delegate_mut() = QueryGrid::new(Rows::default(), 0);
            grid.refresh(cx);
        });
        self.results = None;
        self._results_observer = None;
        self._results_clear = None;
        self.grid.update(cx, |grid, cx| {
            *grid.delegate_mut() = QueryGrid::new(Rows::default(), 0);
            grid.refresh(cx);
        });
        self.document = None;
        self.json_mode = false;
        self.notice.clear();
        self.more = false;
        cx.notify();
        self._results_permission = None;
    }
    fn clear_results_button(&self, cx: &Context<Self>) -> Button {
        Button::new("clear-note-results")
            .small()
            .ghost()
            .icon(IconName::Close)
            .tooltip(t(cx, "notes.clear_results_help"))
            .disabled(self.working(cx) || self.batch_locked)
            .on_click(cx.listener(|this, _, _, cx| this.clear_results(cx)))
    }
    fn run_button(&self, cx: &Context<Self>) -> Button {
        let busy = self.working(cx);
        crate::assets::note_tool("run-note-sql")
            .icon(
                gpui_component::Icon::default()
                    .path(if busy {
                        "icons/stop.svg"
                    } else {
                        "icons/note-run.svg"
                    })
                    .text_color(if busy {
                        cx.theme().danger
                    } else {
                        crate::assets::note_color(1, cx)
                    }),
            )
            .tooltip(t(
                cx,
                if busy {
                    "notes.stop"
                } else if self.editing && !self.editor.read(cx).selection_byte_range().is_empty() {
                    "notes.query.run_selection"
                } else {
                    "notes.query.run_all"
                },
            ))
            .disabled(!busy && (self.batch_locked || !self.assist_execution_supported(cx)))
            .on_click(cx.listener(|this, _, window, cx| {
                if this.working(cx) {
                    this.stop(cx)
                } else {
                    this.run(0, window, cx)
                }
            }))
    }
    fn plan_button(&self, cx: &Context<Self>) -> Button {
        let locked = self.working(cx) || self.batch_locked;
        crate::assets::note_tool("explain-note-sql")
            .icon(
                gpui_component::Icon::default()
                    .path("icons/note-plan.svg")
                    .text_color(crate::assets::note_color(0, cx)),
            )
            .tooltip(t(cx, "notes.explain"))
            .disabled(locked || !self.assist_execution_supported(cx))
            .on_click(cx.listener(|this, _, window, cx| this.explain(window, cx)))
    }
    fn scope_controls(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let locked = self.working(cx) || self.batch_locked;
        let profiles = self.connections.read(cx).note_profiles();
        let selected = profiles.iter().find(|p| Some(p.id) == self.connection_id());
        let sqlite = selected.is_some_and(|p| p.kind == DatabaseKind::Sqlite);
        let label = selected
            .map(|p| {
                if self.inherits_connection() {
                    crate::locale::tr(cx, "notes.inherited_connection", &[("name", &p.name)])
                        .to_string()
                } else {
                    p.name.clone()
                }
            })
            .unwrap_or_else(|| t(cx, "notes.choose_connection").to_string());
        let owner = cx.entity();
        let mut left = div().h_flex().gap_1().min_w_0();
        self.refresh_database_picker(window, cx);
        left = left.child(
            Button::new("note-connection")
                .w(px(if self.toolbar_compact { 108. } else { 150. }))
                .min_w(px(76.))
                .small()
                .ghost()
                .child(
                    selected
                        .map(|profile| {
                            crate::connection_choice::identity(profile)
                                .max_w(px(if self.toolbar_compact { 72. } else { 104. }))
                        })
                        .unwrap_or_else(|| div().truncate().child(label.clone()))
                        .max_w(px(if self.toolbar_compact { 72. } else { 104. })),
                )
                .tooltip(label)
                .dropdown_caret(true)
                .disabled(locked || self.discovery.is_some())
                .dropdown_menu(move |mut menu, _, cx| {
                    let inherited = owner.clone();
                    menu = menu.item(PopupMenuItem::new(t(cx, "notes.use_default")).on_click(
                        move |_, window, cx| {
                            inherited.update(cx, |this, cx| {
                                if this.working(cx) || this.batch_locked {
                                    return;
                                }
                                if let CellContent::Sql { connection_id, .. }
                                | CellContent::SqlScriptText { connection_id, .. }
                                | CellContent::SqlScript { connection_id, .. } =
                                    &mut this.cell.content
                                {
                                    *connection_id = None;
                                }
                                this.database
                                    .update(cx, |input, cx| input.set_value("", window, cx));
                                if let Some(script) = &this.script {
                                    script.update(cx, |script, cx| script.scope_changed(cx));
                                }
                                this.revision += 1;
                                this.database_names.clear();
                                this.picker_context = None;
                                if let Some(results) = &this.results {
                                    results.update(cx, |results, cx| results.stale_result(cx));
                                }
                                cx.emit(CellEvent::Changed);
                                cx.notify();
                            })
                        },
                    ));
                    for profile in &profiles {
                        let owner = owner.clone();
                        let profile = profile.clone();
                        menu = menu.item(crate::connection_choice::item(&profile).on_click(
                            move |_, window, cx| {
                                owner.update(cx, |this, cx| {
                                    if this.working(cx) || this.batch_locked {
                                        return;
                                    }
                                    if let CellContent::Sql { connection_id, .. }
                                    | CellContent::SqlScriptText { connection_id, .. }
                                    | CellContent::SqlScript { connection_id, .. } =
                                        &mut this.cell.content
                                    {
                                        *connection_id = Some(profile.id);
                                    }
                                    if let Some(results) = &this.results {
                                        results.update(cx, |results, cx| results.stale_result(cx));
                                    }
                                    this.database_names.clear();
                                    this.picker_context = None;
                                    this.database.update(cx, |input, cx| {
                                        input.set_value(profile.database.clone(), window, cx)
                                    });
                                    if let Some(script) = &this.script {
                                        script.update(cx, |script, cx| script.scope_changed(cx));
                                    }
                                    this.revision += 1;
                                    this.auto_connect(cx);
                                    this.publish_ddl_context(false, cx);
                                    cx.emit(CellEvent::Changed);
                                    cx.notify();
                                })
                            },
                        ));
                    }
                    menu.scrollable(true).min_w(px(360.)).max_h(px(300.))
                }),
        );
        if sqlite {
            let label = self
                .binding(cx)
                .map(|(s, _)| s.database_label())
                .unwrap_or_default();
            left = left.child(
                div()
                    .text_sm()
                    .ml_1()
                    .min_w_0()
                    .max_w(px(180.))
                    .truncate()
                    .text_color(cx.theme().muted_foreground)
                    .child(label),
            );
        } else {
            left = left
                .child(
                    Input::new(&self.database)
                        .small()
                        .cleanable(!locked && self.connection_id().is_some())
                        .w(px(if self.toolbar_compact { 136. } else { 180. }))
                        .min_w(px(90.))
                        .disabled(locked || self.connection_id().is_none())
                        .suffix(
                            Select::new(&self.database_picker)
                                .small()
                                .appearance(false)
                                .menu_width(px(360.))
                                .w(px(28.))
                                .disabled(locked || self.connection_id().is_none()),
                        ),
                )
                .child(
                    Button::new("note-detect-databases")
                        .small()
                        .ghost()
                        .icon(crate::assets::refresh_icon())
                        .tooltip(t(cx, "notes.database_detect"))
                        .disabled(
                            locked || self.discovery.is_some() || self.connection_id().is_none(),
                        )
                        .on_click(
                            cx.listener(|this, _, window, cx| this.detect_databases(window, cx)),
                        ),
                );
        }
        left.into_any_element()
    }
    pub fn set_split_view(&mut self, split: bool, cx: &mut Context<Self>) {
        if self.split_view != split {
            self.split_view = split;
            if split {
                self.toolbar_compact = true;
            }
            cx.notify();
        }
    }
    fn toolbar_measure(&self, cx: &Context<Self>) -> AnyElement {
        let owner = cx.entity().downgrade();
        canvas(
            move |bounds, _, cx| {
                let compact = owner.read_with(cx, |v, _| v.split_view).unwrap_or(false)
                    || f32::from(bounds.size.width) < 820.;
                if owner
                    .read_with(cx, |v, _| v.toolbar_compact != compact)
                    .unwrap_or(false)
                {
                    let owner = owner.clone();
                    cx.defer(move |cx| {
                        let _ = owner.update(cx, |v, cx| {
                            v.toolbar_compact = compact;
                            cx.notify();
                        });
                    });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full()
        .into_any_element()
    }
    fn toolbar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let sql = self.cell.content.is_editable_sql();
        let busy = self.working(cx);
        let locked = busy || self.batch_locked;
        let mut left = div()
            .h_flex()
            .gap_1()
            .flex_1()
            .min_w_0()
            .child(self.mode_button(cx))
            .when(
                sql || matches!(self.cell.content, CellContent::Markdown { .. }),
                |bar| bar.child(self.clear_sql_button(cx)),
            )
            .when(!self.toolbar_compact || !sql, |bar| {
                bar.child(
                    Input::new(&self.title)
                        .cleanable(!locked)
                        .disabled(locked)
                        .small()
                        .w(px(100.))
                        .min_w(px(64.)),
                )
            });
        if sql {
            left = left.child(self.scope_controls(window, cx));
        }
        let mut right = div().h_flex().gap_1().flex_shrink_0();
        if sql {
            right =
                right.child(
                    crate::assets::note_tool("note-tables")
                        .icon(
                            gpui_component::Icon::default()
                                .path("icons/note-tables.svg")
                                .text_color(crate::assets::note_color(0, cx)),
                        )
                        .tooltip(t(cx, "sql_tables.caret"))
                        .disabled(
                            locked
                                || self.connection_id().is_none()
                                || matches!(
                                    self.assist_context(cx).0,
                                    None | Some(DatabaseKind::Redis | DatabaseKind::External)
                                ),
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.trigger_table_completion(window, cx)
                        })),
                );
            right = right.child(
                crate::assets::note_tool("load-note-columns")
                    .icon(
                        gpui_component::Icon::default()
                            .path("icons/note-columns.svg")
                            .text_color(crate::assets::note_color(0, cx)),
                    )
                    .tooltip(t(cx, "sql_columns.open"))
                    .disabled(locked || self.connection_id().is_none())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.trigger_column_completion(window, cx)
                    })),
            );
        }

        if sql && !self.toolbar_compact {
            right = right.child(
                crate::assets::note_tool("format-note-sql")
                    .icon(
                        gpui_component::Icon::default()
                            .path("icons/note-format.svg")
                            .text_color(crate::assets::note_color(2, cx)),
                    )
                    .tooltip(t(cx, "sql_assist.format"))
                    .disabled(
                        locked
                            || self.formatting.is_some()
                            || self.assist_context(cx).0 == Some(DatabaseKind::Redis),
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.format_sql(window, cx))),
            );
        }
        if sql {
            right = right.child(self.run_button(cx));
        }
        if sql && !self.toolbar_compact {
            right = right.child(self.plan_button(cx));
        }
        if sql && !self.toolbar_compact {
            right = right
                .child(
                    crate::assets::note_tool("note-ddl")
                        .icon(
                            gpui_component::Icon::default()
                                .path("icons/note-ddl.svg")
                                .text_color(crate::assets::note_color(0, cx)),
                        )
                        .tooltip(t(cx, "ddl.open"))
                        .on_click(cx.listener(|this, _, _, cx| this.publish_ddl_context(true, cx))),
                )
                .child(self.ai_button(cx));
        }
        if !self.query_only {
            right = right.child(
                crate::assets::note_tool("note-cell-more")
                    .icon(IconName::Ellipsis)
                    .tooltip(t(cx, "notes.cell_more"))
                    .dropdown_menu({
                        let owner = cx.entity().downgrade();
                        move |mut menu, _, cx| {
                            let Some(view) = owner.upgrade() else {
                                return menu;
                            };
                            if view.read(cx).toolbar_compact
                                && view.read(cx).cell.content.is_editable_sql()
                            {
                                let title = view.read(cx).title.clone();
                                menu = menu.item(
                                    PopupMenuItem::new(t(cx, "notes.cell_title"))
                                        .disabled(
                                            view.read(cx).working(cx) || view.read(cx).batch_locked,
                                        )
                                        .on_click(move |_, window, cx| {
                                            let title = title.clone();
                                            window.defer(cx, move |window, cx| {
                                                window.open_dialog(cx, move |dialog, _, cx| {
                                                    dialog
                                                        .title(t(cx, "notes.cell_title"))
                                                        .child(Input::new(&title).cleanable(true))
                                                        .child(
                                                            feedback::actions()
                                                                .child(feedback::close_button(cx)),
                                                        )
                                                });
                                            });
                                        }),
                                );

                                for (key, action) in [
                                    ("sql_assist.format", 0),
                                    ("notes.explain", 1),
                                    ("ddl.open", 2),
                                    ("notes.ai_draft", 3),
                                ] {
                                    let target = view.downgrade();
                                    menu =
                                        menu.item(
                                            PopupMenuItem::new(t(cx, key))
                                                .disabled(
                                                    view.read(cx).working(cx)
                                                        || view.read(cx).batch_locked,
                                                )
                                                .on_click(move |_, window, cx| {
                                                    let target = target.clone();
                                                    window.defer(cx, move |window, cx| {
                                                        let _ = target.update(cx, |this, cx| {
                                                            match action {
                                                                0 => this.format_sql(window, cx),
                                                                1 => this.explain(window, cx),
                                                                2 => this
                                                                    .publish_ddl_context(true, cx),
                                                                _ => this.ask_ai(cx),
                                                            }
                                                        });
                                                    });
                                                }),
                                        );
                                }
                            }
                            if view.read(cx).cell.content.is_editable_sql() {
                                let owner = view.downgrade();
                                let paused = view.read(cx).assistance_paused;
                                menu = menu.item(
                                    PopupMenuItem::new(t(
                                        cx,
                                        if paused {
                                            "sql_assist.resume"
                                        } else {
                                            "sql_assist.pause"
                                        },
                                    ))
                                    .on_click(
                                        move |_, window, cx| {
                                            let owner = owner.clone();
                                            window.defer(cx, move |window, cx| {
                                                let _ = owner.update(cx, |this, cx| {
                                                    this.assistance_paused =
                                                        !this.assistance_paused;
                                                    let enabled = !this.assistance_paused;
                                                    this.completion.set_automatic(enabled);
                                                    this.editor.update(cx, |input, cx| {
                                                        input.handle_action_for_context_menu(
                                                            Box::new(gpui_component::input::Escape),
                                                            window,
                                                            cx,
                                                        );
                                                        input.set_deferred_highlighting_enabled(
                                                            enabled, cx,
                                                        );
                                                    });
                                                    cx.notify();
                                                });
                                            });
                                        },
                                    ),
                                );
                            }
                            for (icon, key, action) in [
                                (IconName::Copy, "notes.cell_copy", 0),
                                (IconName::Plus, "notes.cell_before", 3),
                                (IconName::Plus, "notes.cell_after", 4),
                                (IconName::ArrowUp, "notes.cell_up", 1),
                                (IconName::ArrowDown, "notes.cell_down", 2),
                                (IconName::Delete, "notes.cell_remove", 5),
                            ] {
                                let owner = owner.clone();
                                menu = menu.item(
                                    PopupMenuItem::new(t(cx, key))
                                        .icon(icon)
                                        .disabled(view.read(cx).cell_action_disabled(action, cx))
                                        .on_click(move |_, _, cx| {
                                            let owner = owner.clone();
                                            cx.defer(move |cx| {
                                                let _ = owner.update(cx, |this, cx| {
                                                    this.cell_action(action, cx)
                                                });
                                            });
                                        }),
                                );
                            }
                            menu
                        }
                    }),
            );
        }
        div()
            .id("note-cell-toolbar")
            .w_full()
            .min_w_0()
            .h(px(32.))
            .relative()
            .overflow_x_scroll()
            .track_scroll(&self.toolbar_scroll)
            .map(|mut bar| {
                bar.style().restrict_scroll_to_axis = Some(true);
                bar
            })
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .h(px(24.))
                    .w_full()
                    .min_w(px(if sql {
                        if self.toolbar_compact { 400. } else { 650. }
                    } else {
                        240.
                    }))
                    .gap_2()
                    .child(left)
                    .child(right),
            )
            .child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .h(px(8.))
                    .child(
                        Scrollbar::horizontal(&self.toolbar_scroll)
                            .scrollbar_show(ScrollbarShow::Always),
                    ),
            )
            .child(self.toolbar_measure(cx))
            .into_any_element()
    }
    fn cell_action_disabled(&self, action: usize, cx: &App) -> bool {
        self.batch_locked
            || (self.working(cx) && action == 5)
            || (action == 1 && self.position.0 == 0)
            || (action == 2 && self.position.0 + 1 >= self.position.1)
            || (matches!(action, 0 | 3 | 4)
                && self.position.1 >= turbodbn_services::note_store::MAX_CELLS)
    }
    fn cell_action(&mut self, action: usize, cx: &mut Context<Self>) {
        if self.cell_action_disabled(action, cx) {
            return;
        }
        cx.emit(match action {
            0 => CellEvent::Insert {
                after: true,
                copy: true,
            },
            1 => CellEvent::Move(-1),
            2 => CellEvent::Move(1),
            3 => CellEvent::Insert {
                after: false,
                copy: false,
            },
            4 => CellEvent::Insert {
                after: true,
                copy: false,
            },
            _ => CellEvent::Remove,
        });
    }
}
impl Render for CellView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.script.is_some() {
            return self.render_script(window, cx);
        }
        let sql = self.cell.content.is_editable_sql();
        if sql {
            let context = self.assist_context(cx);
            if self.assist_scope.as_ref() != Some(&context) {
                let language = if context.0 == Some(DatabaseKind::MySql) {
                    "mysql"
                } else {
                    "sql"
                };
                self.assist_scope = Some(context);
                self.reading = None;
                self.editor.update(cx, |input, cx| {
                    input.set_highlighter(language, cx);
                    input.handle_action_for_context_menu(
                        Box::new(gpui_component::input::Escape),
                        window,
                        cx,
                    );
                });
            }
        }
        let md = matches!(self.cell.content, CellContent::Markdown { .. });
        let busy = self.working(cx);
        let wrap = cx
            .global::<crate::preferences::Preferences>()
            .database
            .sql_wrap;
        if sql && wrap != self.wrap {
            self.wrap = wrap;
            self.editor
                .update(cx, |input, cx| input.set_soft_wrap(wrap, window, cx));
        }
        let editing = self.editing;
        let marker = if sql {
            cx.theme().info
        } else if md {
            cx.theme().success
        } else {
            cx.theme().warning
        };
        let mut body = div()
            .id(SharedString::from(format!("note-cell-{}", self.cell.id)))
            .relative()
            .v_flex()
            .gap_2()
            .p_2()
            .w_full()
            .min_w_0()
            .when(self.query_only, |body| {
                body.size_full().min_h_0().overflow_hidden()
            })
            .border_1()
            .border_color(if editing { marker } else { cx.theme().border })
            .track_focus(&self.focus)
            .capture_action(cx.listener(
                |this, _: &crate::cell::markdown_tools::Bold, window, cx| {
                    if this.editing && this.editor.read(cx).focus_handle(cx).is_focused(window) {
                        this.markdown_format(crate::cell::markdown_tools::Format::Bold, window, cx);
                        cx.stop_propagation();
                    } else {
                        cx.propagate();
                    }
                },
            ))
            .capture_action(cx.listener(
                |this, _: &crate::cell::markdown_tools::Italic, window, cx| {
                    if this.editing && this.editor.read(cx).focus_handle(cx).is_focused(window) {
                        this.markdown_format(
                            crate::cell::markdown_tools::Format::Italic,
                            window,
                            cx,
                        );
                        cx.stop_propagation();
                    } else {
                        cx.propagate();
                    }
                },
            ))
            .capture_action(cx.listener(
                |this, _: &crate::cell::markdown_tools::Link, window, cx| {
                    if this.editing && this.editor.read(cx).focus_handle(cx).is_focused(window) {
                        this.markdown_format(crate::cell::markdown_tools::Format::Link, window, cx);
                        cx.stop_propagation();
                    } else {
                        cx.propagate();
                    }
                },
            ))
            .tab_index(0)
            .when(editing, |el| {
                el.child(
                    div()
                        .absolute()
                        .left(px(-12.))
                        .top_0()
                        .bottom_0()
                        .w(px(4.))
                        .rounded_sm()
                        .bg(marker),
                )
            })
            .capture_action(cx.listener(
                |this, action: &gpui_component::input::Enter, window, cx| {
                    this.assist_enter(action, window, cx)
                },
            ))
            .capture_action(
                cx.listener(|this, _: &gpui_component::input::Escape, window, cx| {
                    this.assist_escape(window, cx)
                }),
            )
            .capture_action(cx.listener(
                |this, _: &gpui_component::input::IndentInline, window, cx| {
                    if this.assist_accept(true, window, cx) {
                        cx.stop_propagation();
                    } else {
                        cx.propagate();
                    }
                },
            ))
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key.eq_ignore_ascii_case("f5")
                    && this.cell.content.is_editable_sql()
                    && this.focus.contains_focused(window, cx)
                {
                    this.run(0, window, cx);
                    cx.stop_propagation();
                    return;
                }
                if this.assist_key(event, window, cx) {
                    return;
                }
                if event.keystroke.key == "f2"
                    && !this
                        .results
                        .as_ref()
                        .is_some_and(|r| r.read(cx).focus_handle(cx).contains_focused(window, cx))
                {
                    cx.emit(CellEvent::Edit);
                    cx.stop_propagation();
                }
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.modifiers.control
                    && event.keystroke.key.eq_ignore_ascii_case("c")
                    && this.grid.read(cx).focus_handle(cx).is_focused(window)
                {
                    this.copy(cx);
                    cx.stop_propagation();
                }
            }));
        if self.collapsed {
            let label = self.title.read(cx).value();
            let label = if label.trim().is_empty() {
                self.editor
                    .read(cx)
                    .value()
                    .lines()
                    .next()
                    .unwrap_or("")
                    .chars()
                    .take(160)
                    .collect::<String>()
            } else {
                label.to_string()
            };
            return body
                .child(
                    div()
                        .h_flex()
                        .gap_2()
                        .child(
                            Button::new("expand-note-cell")
                                .ghost()
                                .small()
                                .justify_start()
                                .icon(gpui_component::Icon::default().path("icons/note-expand.svg"))
                                .flex_1()
                                .min_w_0()
                                .label(if label.is_empty() {
                                    t(cx, "notes.empty_cell")
                                } else {
                                    label.into()
                                })
                                .tooltip(t(cx, "notes.expand_cell"))
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.set_collapsed(false, cx)),
                                ),
                        )
                        .when(sql, |bar| bar.child(self.ai_button(cx)))
                        .child(self.copy_source_button(cx)),
                )
                .into_any_element();
        }
        if let Some(task) = &self.task_list {
            let task = task.clone();
            if editing {
                body = body.child(self.toolbar(window, cx));
            } else {
                body = body.child(
                    div()
                        .h_flex()
                        .gap_2()
                        .child(self.mode_button(cx))
                        .child(self.collapse_button(cx))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_ellipsis()
                                .overflow_hidden()
                                .child(self.title.read(cx).value()),
                        )
                        .child(self.copy_source_button(cx)),
                );
            }
            return body.child(task).into_any_element();
        }

        if editing {
            body = body.child(self.toolbar(window, cx));
            if sql
                && matches!(
                    self.assist_context(cx).0,
                    Some(DatabaseKind::ClickHouse | DatabaseKind::Redis)
                )
            {
                body = body.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(t(cx, "sql_assist.edit_only")),
                );
            }
        } else if !sql || !self.title.read(cx).value().is_empty() {
            body = body.child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(self.title.read(cx).value()),
                    )
                    .when(!sql, |row| row.child(self.copy_source_button(cx))),
            );
        }
        if editing {
            let max_lines = if sql { self.editor_max_lines() } else { 12 };
            let lines = editor_shape(self.editor.read(cx).text(), max_lines).0;
            if md {
                body = body.child(self.markdown_toolbar(cx));
            }
            body = body.child(
                div().when(md, |el| el.key_context("NoteMarkdown")).child(
                    Input::new(&self.editor)
                        .disabled(busy || self.batch_locked)
                        .appearance(false)
                        .bordered(false)
                        .focus_bordered(false)
                        .line_height(px(22.))
                        .h(px(if sql {
                            self.editor_content_height
                                .max(3. * 22.)
                                .min(self.editor_height_limit - 16.)
                                + 16.
                        } else {
                            lines as f32 * 22. + 16.
                        })),
                ),
            );
            if sql {
                let owner = cx.entity().downgrade();
                let editor = self.editor.clone();
                body = body.child(
                    canvas(
                        move |_, _, cx| {
                            if let Some(height) = editor.read(cx).content_height() {
                                let height = f32::from(height);
                                if let Some(cell) = owner.upgrade() {
                                    let cell = cell.read(cx);
                                    let limit = cell.editor_height_limit - 16.;
                                    let displayed = |height: f32| height.max(66.).min(limit);
                                    if (displayed(height) - displayed(cell.editor_content_height))
                                        .abs()
                                        < 1.
                                    {
                                        return;
                                    }
                                }
                                let owner = owner.clone();
                                cx.defer(move |cx| {
                                    let _ = owner.update(cx, |this, cx| {
                                        if (this.editor_content_height - height).abs() >= 1. {
                                            this.editor_content_height = height;
                                            cx.notify();
                                        }
                                    });
                                });
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .h_0()
                    .w_0(),
                );
            }
        } else {
            let source = self.editor.read(cx).value();
            let empty = source.is_empty();
            let content: AnyElement = if empty {
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(t(cx, "notes.empty_cell"))
                    .into_any_element()
            } else if sql || md {
                if self.reading.as_ref().is_none_or(|(old, _)| old != &source) {
                    let rendered = if sql {
                        sql_reading(
                            &source,
                            self.assist_context(cx).0.unwrap_or(DatabaseKind::Sqlite),
                        )
                        .into()
                    } else {
                        source.clone()
                    };
                    self.reading = Some((source.clone(), rendered));
                }
                TextView::markdown(
                    SharedString::from(format!(
                        "note-reading-{}-{}",
                        self.cell.id,
                        cx.theme().mode.is_dark()
                    )),
                    self.reading.as_ref().unwrap().1.clone(),
                    window,
                    cx,
                )
                .selectable(true)
                .when(sql, |view| {
                    view.style(
                        gpui_component::text::TextViewStyle::default()
                            .code_block(StyleRefinement::default().pr(px(144.)).pl(px(76.))),
                    )
                })
                .into_any_element()
            } else {
                div().whitespace_normal().child(source).into_any_element()
            };
            let reading = div()
                .id("note-reading-content")
                .min_h(px(36.))
                .w_full()
                .capture_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    if event.button != MouseButton::Left {
                        return;
                    }
                    if event.click_count == 2 {
                        cx.emit(CellEvent::Edit);
                        cx.stop_propagation();
                    } else {
                        this.focus.focus(window);
                    }
                }))
                .when(sql && empty, |el| el.pr(px(144.)))
                .when(!sql || empty, |el| el.pl(px(76.)))
                .child(content);
            body = body.child(
                div()
                    .relative()
                    .child(reading)
                    .child(
                        div()
                            .id("reading-mode-toggle")
                            .occlude()
                            .absolute()
                            .top_2()
                            .left_1()
                            .h_flex()
                            .gap_1()
                            .child(self.mode_button(cx))
                            .child(self.collapse_button(cx)),
                    )
                    .when(sql, |el| {
                        el.child(
                            div()
                                .id("note-reading-copy")
                                .occlude()
                                .absolute()
                                .top_2()
                                .right_2()
                                .h_flex()
                                .gap_1()
                                .child(self.run_button(cx))
                                .child(self.plan_button(cx))
                                .child(self.ai_button(cx))
                                .child(self.copy_source_button(cx)),
                        )
                    }),
            );
        }
        if let Some(results) = &self.results {
            body = body.child(
                div()
                    .min_w_0()
                    .when(self.query_only, |b| b.flex_1().min_h_0())
                    .when(!self.query_only, |b| b.h(px(380.)))
                    .child(
                        AnyView::from(results.clone())
                            .cached(StyleRefinement::default().size_full()),
                    ),
            );
        }
        if self.execution_summary.is_some() {
            body = body.child(
                div()
                    .id("single-command-status")
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .child(gpui_component::Icon::new(IconName::Check))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(self.notice.clone()),
                    )
                    .child(
                        Button::new("single-command-details")
                            .ghost()
                            .small()
                            .icon(if self.execution_details {
                                IconName::ChevronUp
                            } else {
                                IconName::ChevronDown
                            })
                            .label(t(cx, "notes.results.details"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.execution_details = !this.execution_details;
                                cx.notify();
                            })),
                    )
                    .child(self.clear_results_button(cx)),
            );
            if self.execution_details {
                body = body.child(
                    div()
                        .h_flex()
                        .gap_2()
                        .child(self.document_controls(cx))
                        .child(self.clear_results_button(cx))
                        .child(
                            Button::new("copy-execution-summary")
                                .small()
                                .ghost()
                                .icon(IconName::Copy)
                                .label(t(cx, "result.execution.copy"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(summary) = &this.execution_summary {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            serde_json::to_string_pretty(summary)
                                                .unwrap_or_default(),
                                        ));
                                        feedback::success(t(cx, "feedback.copied"), cx);
                                    }
                                })),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(t(cx, "result.execution.count_help")),
                        ),
                );
                if self.json_mode
                    && let Some(document) = &self.document
                {
                    body = body.child(div().h(px(260.)).w_full().child(document.clone()));
                } else {
                    body = body.child(
                        div()
                            .h(px(112.))
                            .w_full()
                            .child(Table::new(&self.execution_grid)),
                    );
                }
            }
        }
        if let Some(batch) = &self.batch_view {
            body = body.child(batch.clone());
        }
        if self.batch_preparing.is_some() {
            body = body.child(t(cx, "notes.multi.preparing"));
        }
        if self.batch_view.is_none()
            && self.results.is_none()
            && sql
            && (self.execution_summary.is_none()
                || !self.grid.read(cx).delegate().rows.columns.is_empty())
            && (!self.notice.is_empty()
                || busy
                || !self.grid.read(cx).delegate().rows.columns.is_empty())
        {
            if self.json_mode
                && self.execution_summary.is_none()
                && let Some(document) = &self.document
            {
                body = body.child(div().h(px(300.)).w_full().child(document.clone()));
            } else if !self.grid.read(cx).delegate().rows.columns.is_empty() {
                let vertical = self.grid.read(cx).vertical_scroll_handle.clone();
                let horizontal = self.grid.read(cx).horizontal_scroll_handle.clone();
                body = body.child(
                    div()
                        .relative()
                        .h(px(240.))
                        .w_full()
                        .child(Table::new(&self.grid).scrollbar_visible(false, false))
                        .child(
                            div()
                                .absolute()
                                .top(px(32.))
                                .right_0()
                                .bottom_0()
                                .w(px(14.))
                                .child(
                                    Scrollbar::vertical(&vertical)
                                        .scrollbar_show(ScrollbarShow::Always),
                                ),
                        )
                        .child(
                            div()
                                .absolute()
                                .left(px(52.))
                                .right_0()
                                .bottom_0()
                                .h(px(14.))
                                .child(
                                    Scrollbar::horizontal(&horizontal)
                                        .scrollbar_show(ScrollbarShow::Always),
                                ),
                        ),
                );
            }
            if self.execution_summary.is_none() {
                let stale = self.last_revision != self.revision;
                let previous = self.last_offset.saturating_sub(self.query.page_size);
                let next = self.last_offset + self.query.page_size;
                body = body.child(
                    div()
                        .h_flex()
                        .gap_2()
                        .child(self.document_controls(cx))
                        .when(busy && !editing, |el| {
                            el.child(
                                Button::new("read-stop-query")
                                    .small()
                                    .ghost()
                                    .icon(gpui_component::Icon::default().path("icons/stop.svg"))
                                    .tooltip(t(cx, "notes.stop"))
                                    .on_click(cx.listener(|this, _, _, cx| this.stop(cx))),
                            )
                        })
                        .child(
                            Button::new("note-previous")
                                .ghost()
                                .icon(IconName::ChevronLeft)
                                .disabled(busy || stale || self.last_offset == 0)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.run(previous, window, cx)
                                })),
                        )
                        .child((self.last_offset / self.query.page_size + 1).to_string())
                        .child(
                            Button::new("note-next")
                                .ghost()
                                .icon(IconName::ChevronRight)
                                .disabled(busy || stale || !self.more)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.run(next, window, cx)
                                })),
                        )
                        .child(
                            Input::new(&self.page_size)
                                .cleanable(!busy)
                                .w(px(64.))
                                .disabled(busy),
                        )
                        .child(
                            Button::new("note-copy-value")
                                .ghost()
                                .icon(IconName::Copy)
                                .tooltip(t(cx, "grid.copy.cell"))
                                .on_click(cx.listener(|this, _, _, cx| this.copy(cx))),
                        )
                        .child(self.clear_results_button(cx))
                        .child(
                            div()
                                .flex_1()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(if self.notice.is_empty() {
                                    String::new()
                                } else {
                                    format!(
                                        "{} · {}",
                                        self.notice,
                                        crate::duration::elapsed(self.elapsed)
                                    )
                                }),
                        ),
                );
            }
        }
        body.when(self.policy_help, |body| {
            body.child(
                Button::new("sql-policy-settings")
                    .icon(IconName::Settings2)
                    .label(t(cx, "settings.sql_policy_open"))
                    .on_click(|_, _, cx| {
                        let hub = cx.global::<feedback::Hub>().0.clone();
                        cx.defer(move |cx| {
                            hub.update(cx, |_, cx| cx.emit(feedback::OpenSqlSettings))
                        });
                    }),
            )
        })
        .into_any_element()
    }
}

// Only these text-derived properties affect the SQL cell's surrounding UI.
// Keep ordinary character edits inside InputState rather than rebuilding the result area.
fn editor_shape(text: &gpui_component::Rope, max_lines: usize) -> (usize, bool) {
    (
        text.iter_lines()
            .take(max_lines)
            .count()
            .clamp(3, max_lines),
        text.chars().all(char::is_whitespace),
    )
}

// Thousands of styled runs in one shaped line can block input even with background completion.
// Limit highlighting only; the editor, completion, copy and execution retain the full source.
fn long_sql_line(bytes: impl Iterator<Item = u8>) -> bool {
    let mut width = 0;
    for byte in bytes {
        if byte == b'\n' || byte == b'\r' {
            width = 0;
        } else {
            width += 1;
            if width > 8192 {
                return true;
            }
        }
    }
    false
}

/// Shared registered SQL highlighting for Note reading and script previews.
pub(crate) fn sql_reading(source: &str, kind: DatabaseKind) -> String {
    let count = source
        .split(|c| c != '~')
        .map(str::len)
        .max()
        .unwrap_or(0)
        .max(2)
        + 1;
    let fence = "~".repeat(count);
    let language = if long_sql_line(source.bytes()) {
        crate::SQL_LIGHT_LANGUAGE
    } else if kind == DatabaseKind::MySql {
        "mysql"
    } else {
        "sql"
    };
    format!("{fence}{language}\n{source}\n{fence}")
}

#[cfg(test)]
mod deferred_snapshot_tests {
    use super::CellSnapshot;
    use gpui_component::RopeExt;
    use turbodbn_core::{Cell, CellContent};
    use uuid::Uuid;
    #[test]
    fn save_worker_owns_frozen_text_and_preserves_cell_scope() {
        let connection = Uuid::new_v4();
        for content in [
            CellContent::Sql {
                source: String::new(),
                connection_id: Some(connection),
                database: Some("fixture".into()),
            },
            CellContent::SqlScriptText {
                source: String::new(),
                connection_id: Some(connection),
                database: Some("fixture".into()),
            },
            CellContent::Markdown {
                source: String::new(),
            },
        ] {
            let id = Uuid::new_v4();
            let mut text = gpui_component::Rope::from("SELECT '中文';");
            let snapshot = CellSnapshot {
                cell: Cell {
                    id,
                    title: "Frozen draft".into(),
                    content: content.clone(),
                },
                source: Some(text.clone()),
            };
            text.replace(0..6, "UPDATE");
            let restored = std::thread::spawn(move || snapshot.finish())
                .join()
                .unwrap();
            assert_eq!(restored.id, id);
            assert_eq!(restored.title, "Frozen draft");
            match restored.content {
                CellContent::Sql {
                    source,
                    connection_id,
                    database,
                }
                | CellContent::SqlScriptText {
                    source,
                    connection_id,
                    database,
                } => {
                    assert_eq!(source, "SELECT '中文';");
                    assert_eq!(connection_id, Some(connection));
                    assert_eq!(database.as_deref(), Some("fixture"));
                }
                CellContent::Markdown { source } => assert_eq!(source, "SELECT '中文';"),
                _ => panic!("Snapshot changed cell kind"),
            }
        }
    }
}

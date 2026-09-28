use crate::{
    database::{DatabaseChoice, DatabaseView, SavedConnection},
    feedback,
    locale::t,
    query_grid::QueryGrid,
};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Disableable, IconName, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
    menu::{DropdownMenu, PopupMenuItem},
    scroll::{Scrollbar, ScrollbarShow},
    select::{SearchableVec, Select, SelectEvent, SelectState},
    table::{Table, TableState},
    text::TextView,
};
use turbodbn_core::{Cell, CellContent, QueryState, QueryStatus, TaskStatus};
use turbodbn_services::{
    DatabaseKind,
    database::{
        Operation, OperationKind, OperationResult,
        notebook_query::Plan,
        workbench::{Action, Reply, Request, Rows},
    },
    mysql::{Event, Session},
};
use uuid::Uuid;

pub enum CellEvent {
    Changed,
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
    cell: Cell,
    editor: Entity<InputState>,
    title: Entity<InputState>,
    database: Entity<InputState>,
    connections: Entity<DatabaseView>,
    inherited_connection: Option<Uuid>,
    inherited_database: String,
    editing: bool,
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
    last_offset: usize,
    more: bool,
    elapsed: u64,
    operation: Option<(Uuid, std::time::Instant)>,
    notice: String,
    grid: Entity<TableState<QueryGrid>>,
    results: Option<Entity<crate::table_view::TableView>>,
    _results_observer: Option<Subscription>,
    page_size: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
    _connections: Subscription,
}
impl EventEmitter<CellEvent> for CellView {}
impl CellView {
    pub fn new(
        mut cell: Cell,
        connections: Entity<DatabaseView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let source = match &mut cell.content {
            CellContent::Sql { source, .. } | CellContent::Markdown { source } => {
                std::mem::take(source)
            }
            CellContent::Task { description, .. } => std::mem::take(description),
        };
        let sql = matches!(cell.content, CellContent::Sql { .. });
        let wrap = cx
            .global::<crate::preferences::Preferences>()
            .database
            .sql_wrap;
        let editor = cx.new(|cx| {
            let input = InputState::new(window, cx)
                .multi_line(true)
                .default_value(source);
            if sql {
                input.code_editor("sql").soft_wrap(wrap)
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
                    CellContent::Sql { database, .. } => database.clone().unwrap_or_default(),
                    _ => String::new(),
                })
        });
        let page_size = cx.new(|cx| InputState::new(window, cx).default_value("20"));
        let mut subscriptions: Vec<Subscription> =
            [editor.clone(), title.clone(), database.clone()]
                .iter()
                .map(|input| {
                    let query_input = input.entity_id() != title.entity_id();
                    cx.subscribe(input, move |this, _, event, cx| {
                        if matches!(event, InputEvent::Change) {
                            if query_input {
                                this.revision += 1;
                                if let Some(results) = &this.results {
                                    results.update(cx, |results, cx| results.stale_result(cx));
                                }
                            }
                            cx.emit(CellEvent::Changed);
                            cx.notify();
                        }
                    })
                })
                .collect();
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
        let observed = cx.observe(&connections, |this, db, cx| {
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
        Self {
            query: QueryState::new(cell.id),
            cell,
            editor,
            title,
            database,
            connections,
            inherited_connection: None,
            inherited_database: String::new(),
            editing: false,
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
            last_offset: 0,
            more: false,
            elapsed: 0,
            operation: None,
            notice: String::new(),
            grid,
            results: None,
            _results_observer: None,
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
            }
        )
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
        if affected {
            self.stop(cx);
            self.database_names.clear();
            self.picker_context = None;
            if let Some(results) = &self.results {
                results.update(cx, |results, cx| results.stale_result(cx));
            }
            cx.notify();
        }
    }
    pub fn has_sql(&self, cx: &App) -> bool {
        matches!(self.cell.content, CellContent::Sql { .. })
            && !self.editor.read(cx).value().trim().is_empty()
    }
    pub fn batch_scope(&self, cx: &App) -> Result<Option<BatchScope>, String> {
        if !matches!(self.cell.content, CellContent::Sql { .. })
            || self.editor.read(cx).value().trim().is_empty()
        {
            return Ok(None);
        }
        if self.working(cx) || self.result_pending(cx) {
            return Err(t(cx, "grid.pending").to_string());
        }
        let (connection, _) = self.binding(cx)?;
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
        }
        cx.notify();
    }
    pub fn set_batch_locked(&mut self, locked: bool, cx: &mut Context<Self>) {
        if self.batch_locked != locked {
            self.batch_locked = locked;
            cx.notify();
        }
    }
    pub fn set_editing(&mut self, editing: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing == editing {
            return;
        }
        self.editing = editing;
        if editing {
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
        let (saved, database) = match self.binding(cx) {
            Ok(v) => v,
            Err(e) => {
                feedback::error(e, cx);
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
        let mut cell = self.cell.clone();
        cell.title = self.title.read(cx).value().to_string();
        match &mut cell.content {
            CellContent::Sql {
                source, database, ..
            } => {
                *source = self.editor.read(cx).value().to_string();
                let value = self.database.read(cx).value().trim().to_owned();
                *database = (!value.is_empty()).then_some(value);
            }
            CellContent::Markdown { source } => *source = self.editor.read(cx).value().to_string(),
            CellContent::Task { description, .. } => {
                *description = self.editor.read(cx).value().to_string()
            }
        }
        cell
    }
    pub fn working(&self, cx: &App) -> bool {
        self.busy()
            || self.awaiting_preview
            || self
                .results
                .as_ref()
                .is_some_and(|r| r.read(cx).result_busy())
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
        matches!(self.query.status, QueryStatus::Running)
    }
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        self.revision += 1;
        self.awaiting_preview = false;
        self.preview_task = None;
        self.finish_batch(false, cx);
        if let Some(results) = &self.results {
            results.update(cx, |results, cx| results.cancel_result(false, cx));
        }
        if let Some((id, started)) = self.operation.take() {
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
            CellContent::Sql { connection_id, .. } => connection_id.or(self.inherited_connection),
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
        self.query.page_size = size;
        self.request(
            saved,
            db,
            Action::NotePrepare {
                sql: self.editor.read(cx).value().to_string(),
            },
            offset,
            window,
            cx,
        );
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
                                            if result.read_only
                                                && let Some((plan, confirmed)) = &execution
                                            {
                                                let view = cx.new(|cx| {
                                                    crate::table_view::TableView::from_note(
                                                        this.connections.downgrade(),
                                                        (saved.clone(), database.clone()),
                                                        (plan.clone(), *confirmed, result.clone()),
                                                        window,
                                                        cx,
                                                    )
                                                });
                                                this._results_observer =
                                                    Some(cx.observe(&view, |_, _, cx| cx.notify()));
                                                this.results = Some(view);
                                            } else {
                                                this.results = None;
                                                this._results_observer = None;
                                                this.grid.update(cx, |grid, cx| {
                                                    *grid.delegate_mut() =
                                                        QueryGrid::new(result.rows, result.offset);
                                                    grid.refresh(cx);
                                                });
                                            }
                                            this.notice = if result.read_only {
                                                crate::locale::tr(
                                                    cx,
                                                    "notes.query_rows",
                                                    &[("count", &rows.to_string())],
                                                )
                                                .to_string()
                                            } else {
                                                t(cx, "notes.command_done").to_string()
                                            };
                                            this.log(
                                                token.request_id,
                                                crate::operation_log::Phase::Succeeded,
                                                result.elapsed_ms,
                                                rows,
                                                None,
                                                cx,
                                            );
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
        if !self.query.complete(token, Err(error.clone())) {
            return;
        }
        self.session = None;
        self.operation = None;
        self.notice = error.clone();
        self.log(
            token.request_id,
            crate::operation_log::Phase::Failed,
            0,
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
            record.elapsed_ms = elapsed;
            record.rows = rows;
            if let Some(error) = error {
                record.failure = crate::operation_log::failure_class(error);
            }
            log.update(cx, |log, cx| log.operation(record, cx));
        }
    }
    fn copy(&self, cx: &mut Context<Self>) {
        if let Some(value) = self.grid.read(cx).delegate().selected_value() {
            cx.write_to_clipboard(ClipboardItem::new_string(value));
            feedback::success(t(cx, "feedback.copied"), cx);
        }
    }
}
impl CellView {
    fn toolbar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let sql = matches!(self.cell.content, CellContent::Sql { .. });
        let busy = self.working(cx);
        let locked = busy || self.batch_locked;
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
        let mut left = div()
            .h_flex()
            .flex_wrap()
            .gap_1()
            .flex_1()
            .min_w(px(220.))
            .child(
                Button::new("read-note-cell")
                    .small()
                    .ghost()
                    .icon(IconName::Eye)
                    .tooltip(t(cx, "notes.read_mode"))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.set_editing(false, window, cx)),
                    ),
            )
            .child(
                Input::new(&self.title)
                    .cleanable(!locked)
                    .disabled(locked)
                    .small()
                    .w(px(160.))
                    .flex_shrink_0(),
            );
        if sql {
            self.refresh_database_picker(window, cx);
            left = left.child(
                Button::new("note-connection")
                    .small()
                    .ghost()
                    .icon(gpui_component::Icon::default().path("icons/database.svg"))
                    .label(label.clone())
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
                                    if let CellContent::Sql { connection_id, .. } =
                                        &mut this.cell.content
                                    {
                                        *connection_id = None;
                                    }
                                    this.database
                                        .update(cx, |input, cx| input.set_value("", window, cx));
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
                            menu = menu.item(
                                PopupMenuItem::new(profile.name.clone())
                                    .disabled(!matches!(
                                        profile.kind,
                                        DatabaseKind::MySql
                                            | DatabaseKind::PostgreSql
                                            | DatabaseKind::Sqlite
                                    ))
                                    .on_click(move |_, window, cx| {
                                        owner.update(cx, |this, cx| {
                                            if this.busy() {
                                                return;
                                            }
                                            if let CellContent::Sql { connection_id, .. } =
                                                &mut this.cell.content
                                            {
                                                *connection_id = Some(profile.id);
                                            }
                                            if let Some(results) = &this.results {
                                                results.update(cx, |results, cx| {
                                                    results.stale_result(cx)
                                                });
                                            }
                                            this.database_names.clear();
                                            this.picker_context = None;
                                            this.database.update(cx, |input, cx| {
                                                input.set_value(
                                                    profile.database.clone(),
                                                    window,
                                                    cx,
                                                )
                                            });
                                            this.revision += 1;
                                            cx.emit(CellEvent::Changed);
                                            cx.notify();
                                        })
                                    }),
                            );
                        }
                        menu.scrollable(true).max_h(px(300.))
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
                        .text_color(cx.theme().muted_foreground)
                        .child(label),
                );
            } else {
                left = left
                    .child(
                        Input::new(&self.database)
                            .small()
                            .cleanable(!locked && self.connection_id().is_some())
                            .w(px(260.))
                            .flex_shrink_0()
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
                                locked
                                    || self.discovery.is_some()
                                    || self.connection_id().is_none(),
                            )
                            .on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.detect_databases(window, cx)
                                }),
                            ),
                    );
            }
        }
        let mut right = div().h_flex().gap_1().flex_shrink_0();
        if sql {
            right = right.child(
                Button::new("run-note-sql")
                    .small()
                    .ghost()
                    .icon(gpui_component::Icon::default().path(if busy {
                        "icons/stop.svg"
                    } else {
                        "icons/note-run.svg"
                    }))
                    .tooltip(t(cx, if busy { "notes.stop" } else { "notes.run" }))
                    .disabled(!busy && (self.batch_locked || self.connection_id().is_none()))
                    .on_click(cx.listener(|this, _, window, cx| {
                        if this.working(cx) {
                            this.stop(cx)
                        } else {
                            this.run(0, window, cx)
                        }
                    })),
            );
        }
        for (id, icon, key, action) in [
            ("copy-note-cell", IconName::Copy, "notes.cell_copy", 0),
            ("move-note-up", IconName::ArrowUp, "notes.cell_up", 1),
            ("move-note-down", IconName::ArrowDown, "notes.cell_down", 2),
            ("insert-note-before", IconName::Plus, "notes.cell_before", 3),
            ("insert-note-after", IconName::Plus, "notes.cell_after", 4),
            ("delete-note-cell", IconName::Delete, "notes.cell_remove", 5),
        ] {
            let at_start = self.position.0 == 0;
            let at_end = self.position.0 + 1 >= self.position.1;
            let at_limit = self.position.1 >= turbodbn_services::note_store::MAX_CELLS;
            let icon = if action == 3 {
                gpui_component::Icon::default().path("icons/note-insert-before.svg")
            } else if action == 4 {
                gpui_component::Icon::default().path("icons/note-insert-after.svg")
            } else if action == 5 {
                gpui_component::Icon::default().path("icons/note-delete.svg")
            } else {
                icon.into()
            };
            right = right.child(
                Button::new(id)
                    .small()
                    .ghost()
                    .icon(icon)
                    .tooltip(t(cx, key))
                    .disabled(
                        self.batch_locked
                            || (busy && action == 5)
                            || (action == 1 && at_start)
                            || (action == 2 && at_end)
                            || (matches!(action, 0 | 3 | 4) && at_limit),
                    )
                    .on_click(cx.listener(move |_, _, _, cx| {
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
                        })
                    })),
            );
        }
        div()
            .h_flex()
            .flex_wrap()
            .justify_between()
            .gap_2()
            .child(left)
            .child(right)
            .into_any_element()
    }
}
impl Render for CellView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sql = matches!(self.cell.content, CellContent::Sql { .. });
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
            .min_w_0()
            .border_1()
            .border_color(if editing { marker } else { cx.theme().border })
            .track_focus(&self.focus)
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
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "f2"
                    && !this
                        .results
                        .as_ref()
                        .is_some_and(|r| r.read(cx).focus_handle(cx).contains_focused(window, cx))
                {
                    cx.emit(CellEvent::Edit);
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.key == "escape" && this.editing {
                    this.set_editing(false, window, cx);
                    cx.stop_propagation();
                    return;
                }
                let enter = cx
                    .global::<crate::preferences::Preferences>()
                    .database
                    .sql_enter;
                if this.editing
                    && matches!(this.cell.content, CellContent::Sql { .. })
                    && event.keystroke.key == "enter"
                    && !event.keystroke.modifiers.shift
                    && (event.keystroke.modifiers.control || enter)
                    && this.editor.read(cx).focus_handle(cx).is_focused(window)
                {
                    cx.stop_propagation();
                    this.run(0, window, cx);
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
            return body.child(
                Button::new("expand-note-cell")
                    .ghost()
                    .small()
                    .justify_start()
                    .icon(IconName::ChevronRight)
                    .label(if label.is_empty() {
                        t(cx, "notes.empty_cell")
                    } else {
                        label.into()
                    })
                    .tooltip(t(cx, "notes.expand_cell"))
                    .on_click(cx.listener(|this, _, _, cx| this.set_collapsed(false, cx))),
            );
        }
        if editing {
            body = body.child(self.toolbar(window, cx));
        } else if !self.title.read(cx).value().trim().is_empty() {
            body = body.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.title.read(cx).value()),
            );
        }
        if editing {
            let lines = self.editor.read(cx).value().lines().count().clamp(3, 12);
            body = body.child(
                feedback::editable_multiline(&self.editor, busy || self.batch_locked, cx)
                    .appearance(false)
                    .bordered(false)
                    .focus_bordered(false)
                    .h(px(lines as f32 * 22. + 16.)),
            );
        } else {
            let source = self.editor.read(cx).value();
            let content: AnyElement = if source.is_empty() {
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(t(cx, "notes.empty_cell"))
                    .into_any_element()
            } else if sql || md {
                if self.reading.as_ref().is_none_or(|(old, _)| old != &source) {
                    let rendered = if sql {
                        let count = source
                            .split(|c| c != '~')
                            .map(str::len)
                            .max()
                            .unwrap_or(0)
                            .max(2)
                            + 1;
                        let fence = "~".repeat(count);
                        format!("{fence}sql\n{source}\n{fence}").into()
                    } else {
                        source.clone()
                    };
                    self.reading = Some((source.clone(), rendered));
                }
                TextView::markdown(
                    SharedString::from(format!("note-reading-{}", self.cell.id)),
                    self.reading.as_ref().unwrap().1.clone(),
                    window,
                    cx,
                )
                .selectable(true)
                .into_any_element()
            } else {
                div().whitespace_normal().child(source).into_any_element()
            };
            body = body.child(
                div()
                    .id("note-reading-content")
                    .min_h(px(36.))
                    .w_full()
                    .capture_any_mouse_down(cx.listener(
                        |this, event: &MouseDownEvent, window, cx| {
                            if event.button != MouseButton::Left {
                                return;
                            }
                            if event.click_count == 2 {
                                cx.emit(CellEvent::Edit);
                                cx.stop_propagation();
                            } else {
                                this.focus.focus(window);
                            }
                        },
                    ))
                    .child(content),
            );
        }
        if let CellContent::Task { status, .. } = self.cell.content {
            let key = match status {
                TaskStatus::Pending => "notebook.task.pending",
                TaskStatus::InProgress => "notebook.task.in_progress",
                TaskStatus::Completed => "notebook.task.completed",
            };
            body = body.child(if editing {
                Button::new("note-task-status")
                    .small()
                    .ghost()
                    .icon(IconName::CircleCheck)
                    .label(t(cx, key))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let CellContent::Task { status, .. } = &mut this.cell.content {
                            *status = match status {
                                TaskStatus::Pending => TaskStatus::InProgress,
                                TaskStatus::InProgress => TaskStatus::Completed,
                                TaskStatus::Completed => TaskStatus::Pending,
                            };
                        }
                        this.revision += 1;
                        cx.emit(CellEvent::Changed);
                        cx.notify();
                    }))
                    .into_any_element()
            } else {
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(t(cx, key))
                    .into_any_element()
            });
        }
        if let Some(results) = &self.results {
            body = body.child(div().h(px(380.)).min_w_0().child(results.clone()));
        }
        if self.results.is_none()
            && sql
            && (!self.notice.is_empty()
                || busy
                || !self.grid.read(cx).delegate().rows.columns.is_empty())
        {
            if !self.grid.read(cx).delegate().rows.columns.is_empty() {
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
            let stale = self.last_revision != self.revision;
            let previous = self.last_offset.saturating_sub(self.query.page_size);
            let next = self.last_offset + self.query.page_size;
            body = body.child(
                div()
                    .h_flex()
                    .gap_2()
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
                            .on_click(
                                cx.listener(move |this, _, window, cx| this.run(next, window, cx)),
                            ),
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
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(if self.notice.is_empty() {
                                String::new()
                            } else {
                                format!("{} · {} ms", self.notice, self.elapsed)
                            }),
                    ),
            );
        }
        body
    }
}

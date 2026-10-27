use crate::{
    database::{DatabaseView, OpenTable},
    feedback,
    locale::{t, tr},
    query_grid::QueryGrid,
};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Disableable, IconName, Selectable, StyledExt, WindowExt,
    button::{Button, ButtonVariants},
    input::{Input, InputState},
    menu::DropdownMenu,
    popover::Popover,
    select::{SearchableVec, Select, SelectEvent, SelectState},
    table::TableState,
};
use turbodbn_services::{
    DatabaseKind,
    database::{
        comparison,
        workbench::{Action, PageOptions, Reply, Rows, Schema},
    },
};
struct Side {
    database_picker: Entity<SelectState<SearchableVec<String>>>,
    table_picker: Option<Entity<crate::ddl_picker::Picker>>,
    table_subscription: Option<Subscription>,
    picker_open: bool,
    target: OpenTable,
    database: Entity<InputState>,
    schema: Entity<InputState>,
    table: Entity<InputState>,
}
#[path = "compare_layout.rs"]
mod layout;
type SavedComparison = (
    String,
    Option<Entity<TableState<QueryGrid>>>,
    [Option<Entity<TableState<QueryGrid>>>; 2],
    bool,
    bool,
    bool,
);
pub struct Comparison {
    field_filter: Entity<InputState>,
    detail_grid: Option<Entity<TableState<QueryGrid>>>,
    table_scroll: UniformListScrollHandle,
    table_class: usize,
    detail_class: usize,
    progress: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    field_position: usize,
    metadata_view: bool,
    expanded_fields: std::collections::HashSet<String>,
    saved_views: std::collections::HashMap<String, SavedComparison>,
    source: WeakEntity<DatabaseView>,
    sides: [Side; 2],
    structure: bool,
    busy: bool,
    compare_control: Option<std::sync::Arc<turbodbn_services::database::transfer::Control>>,
    whole_database: bool,
    include_data: bool,
    ignore_comments: bool,
    ignore_column_order: bool,
    reasons_view: bool,
    table_reasons: std::collections::BTreeMap<String, Vec<comparison::Difference>>,
    only_differences: bool,
    snapshots: Option<[comparison::DatabaseSnapshot; 2]>,
    selected_table: Option<String>,
    sync_tables: std::collections::BTreeSet<String>,
    table_status: std::collections::BTreeMap<String, bool>,
    table_filter: Entity<InputState>,
    revision: u64,
    locked: std::rc::Rc<std::cell::Cell<bool>>,
    scroll_subscriptions: Vec<Subscription>,
    notice: String,
    grids: [Option<Entity<TableState<QueryGrid>>>; 2],
    ddl: [String; 2],
    table_mode: bool,
    schemas: Option<[Schema; 2]>,
    compared_targets: Option<[OpenTable; 2]>,
    _subscriptions: Vec<Subscription>,
}
impl EventEmitter<crate::table_view::TableDraftChanged> for Comparison {}
impl Comparison {
    fn start_database(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !cx
            .global::<crate::preferences::Preferences>()
            .database
            .command_enabled("select")
        {
            feedback::error(t(cx, "compare.select_disabled"), cx);
            return;
        }

        let Some(source) = self.source.upgrade() else {
            return;
        };
        let mut endpoints = vec![];
        let mut targets = self.snapshot(cx);
        for target in &mut targets {
            let db = source.read(cx);
            if db.note_connection_state(target.connection) != Some(true) {
                feedback::error(t(cx, "compare.connect_first"), cx);
                return;
            }
            let saved = match db.saved_connection(target.connection, cx) {
                Ok(s) => s,
                Err(e) => {
                    feedback::error(e, cx);
                    return;
                }
            };
            target.kind = saved.profile.kind;
            target.generation = db.note_generation(target.connection);
            match saved.endpoint(target.database.clone()) {
                Ok(e) => endpoints.push(e),
                Err(e) => {
                    feedback::error(e, cx);
                    return;
                }
            }
        }
        self.invalidate();
        self.busy = true;
        let revision = self.revision;
        let include_data = self.include_data;
        let ignore_comments = self.ignore_comments;
        let ignore_order = self.ignore_column_order;
        let kind = targets[0].kind;
        let control =
            std::sync::Arc::new(turbodbn_services::database::transfer::Control::default());
        self.compare_control = Some(control.clone());
        let output = source.read(cx).output.clone();
        let job = uuid::Uuid::new_v4();
        let started = std::time::Instant::now();
        if let Some(output) = &output {
            let record = crate::operation_log::Record::new(
                job,
                crate::operation_log::Kind::Metadata,
                crate::operation_log::Phase::Started,
            );
            output.update(cx, |output, cx| output.operation(record, cx));
        }
        self.progress.store(0, std::sync::atomic::Ordering::Relaxed);
        let progress = self.progress.clone();
        cx.spawn(async move |owner, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(150))
                    .await;
                let keep = owner
                    .update(cx, |this, cx| {
                        cx.notify();
                        this.busy
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        })
        .detach();
        let (partial_tx, partial_rx) =
            async_channel::bounded::<(usize, comparison::TableSnapshot)>(400);
        let partial_targets = targets.clone();
        cx.spawn_in(window, async move |owner, cx| {
            while let Ok((side, table)) = partial_rx.recv().await {
                let _ = owner.update_in(cx, |this, window, cx| {
                    if this.revision != revision
                        || !this.busy
                        || partial_targets.iter().any(|target| {
                            source.read(cx).note_generation(target.connection) != target.generation
                        })
                    {
                        return;
                    }
                    let snapshots = this.snapshots.get_or_insert_with(|| {
                        [
                            comparison::DatabaseSnapshot { tables: vec![] },
                            comparison::DatabaseSnapshot { tables: vec![] },
                        ]
                    });
                    snapshots[side].tables.push(table);
                    if snapshots[side].tables.len().is_multiple_of(50) {
                        cx.notify();
                    }
                    let completed_key =
                        comparison::table_key(snapshots[side].tables.last().unwrap());
                    if this.selected_table.is_none()
                        || this.selected_table.as_deref() == Some(&completed_key)
                            && this.schemas.is_none()
                    {
                        let key = comparison::table_key(snapshots[side].tables.last().unwrap());
                        this.show_database_table(&key, window, cx);
                    }
                });
            }
        })
        .detach();
        let source = self.source.upgrade().unwrap();
        let work = cx.background_executor().spawn(async move {
            let c = control;
            let a = comparison::database_snapshot_stream(
                &endpoints[0],
                include_data,
                &c,
                |done, _| {
                    progress.store(done, std::sync::atomic::Ordering::Relaxed);
                },
                |table| {
                    let _ = partial_tx.send_blocking((0, table.clone()));
                },
            )?;
            let a_count = a.tables.len();
            let b = comparison::database_snapshot_stream(
                &endpoints[1],
                include_data,
                &c,
                |done, _| {
                    progress.store(a_count + done, std::sync::atomic::Ordering::Relaxed);
                },
                |table| {
                    let _ = partial_tx.send_blocking((1, table.clone()));
                },
            )?;
            let reasons = comparison::database_pairs(&a, &b)
                .into_iter()
                .map(|key| {
                    let l = comparison::database_find(&a, &key);
                    let r = comparison::database_find(&b, &key);
                    let mut reasons = comparison::structural_differences(
                        kind,
                        l,
                        r,
                        ignore_comments,
                        ignore_order,
                    );
                    if include_data {
                        let left = l
                            .and_then(|table| table.rows.as_ref())
                            .cloned()
                            .unwrap_or_default();
                        let right = r
                            .and_then(|table| table.rows.as_ref())
                            .cloned()
                            .unwrap_or_default();
                        let changed = comparison::data(
                            &left,
                            &l.map(|table| table.schema.clone()).unwrap_or_default(),
                            &right,
                            &r.map(|table| table.schema.clone()).unwrap_or_default(),
                        )
                        .different;
                        if changed > 0 {
                            reasons.push(comparison::Difference {
                                category: "compare.reason.data",
                                reference: left.values.len().to_string(),
                                target: right.values.len().to_string(),
                            });
                        }
                    }
                    (key, reasons)
                })
                .collect::<std::collections::BTreeMap<_, _>>();
            Ok::<_, String>(([a, b], reasons))
        });
        cx.spawn_in(window, async move |owner, cx| {
            let result = work.await;
            let _ = owner.update_in(cx, |this, window, cx| {
                let cancelled = this
                    .compare_control
                    .as_ref()
                    .is_some_and(|control| control.is_cancelled());
                this.busy = false;
                this.compare_control = None;
                if let Some(output) = &output {
                    let phase = if cancelled || this.revision != revision {
                        crate::operation_log::Phase::Cancelled
                    } else if result.is_ok() {
                        crate::operation_log::Phase::Succeeded
                    } else {
                        crate::operation_log::Phase::Failed
                    };
                    let mut record = crate::operation_log::Record::new(
                        job,
                        crate::operation_log::Kind::Metadata,
                        phase,
                    );
                    record.elapsed_ms = started.elapsed().as_millis() as u64;
                    output.update(cx, |output, cx| output.operation(record, cx));
                }
                if this.revision != revision
                    || targets
                        .iter()
                        .any(|t| source.read(cx).note_generation(t.connection) != t.generation)
                {
                    cx.notify();
                    return;
                }
                match result {
                    Ok((snapshots, reasons)) => {
                        this.table_reasons = reasons;
                        this.refresh_table_status();
                        let keys = comparison::database_pairs(&snapshots[0], &snapshots[1]);
                        this.compared_targets = Some(targets);
                        this.snapshots = Some(snapshots);
                        this.notice = tr(
                            cx,
                            "compare.database_summary",
                            &[("count", &keys.len().to_string())],
                        )
                        .to_string();
                        let visible = this.visible_tables(cx);
                        let selected = this
                            .selected_table
                            .clone()
                            .filter(|key| visible.contains(key))
                            .or_else(|| visible.first().cloned());
                        this.saved_views.clear();
                        this.detail_grid = None;
                        if let Some(key) = selected {
                            this.show_database_table(&key, window, cx);
                        } else {
                            this.selected_table = None;
                            this.schemas = None;
                            this.grids = [None, None];
                        }
                    }
                    Err(e) => {
                        this.snapshots = None;
                        this.schemas = None;
                        this.detail_grid = None;
                        this.grids = [None, None];
                        this.saved_views.clear();
                        this.selected_table = None;
                        this.notice = if cancelled {
                            t(cx, "compare.cancelled").to_string()
                        } else {
                            crate::locale::message(cx, &e).to_string()
                        };
                        if cancelled {
                            feedback::notify(
                                gpui_component::notification::NotificationType::Info,
                                this.notice.clone(),
                                cx,
                            );
                        } else {
                            feedback::error(this.notice.clone(), cx);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn show_database_table(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let previous = self.selected_table.clone();
        if previous.as_deref() != Some(key)
            && let Some(previous) = previous.clone()
        {
            self.saved_views.insert(
                previous,
                (
                    self.field_filter.read(cx).value().to_string(),
                    self.detail_grid.take(),
                    self.grids.clone(),
                    self.reasons_view,
                    self.metadata_view,
                    self.table_mode,
                ),
            );
        }
        if self.saved_views.len() > 32
            && let Some(oldest) = self
                .saved_views
                .keys()
                .find(|saved| saved.as_str() != key)
                .cloned()
        {
            self.saved_views.remove(&oldest);
        }
        let Some(snapshots) = &self.snapshots else {
            return;
        };
        let l = comparison::database_find(&snapshots[0], key);
        let r = comparison::database_find(&snapshots[1], key);
        if self.busy && (l.is_none() || r.is_none()) {
            self.selected_table = Some(key.into());
            self.schemas = None;
            self.detail_grid = None;
            self.grids = [None, None];
            self.notice = t(cx, "compare.wait_pair").to_string();
            cx.notify();
            return;
        }
        let schemas = [
            l.map(|t| t.schema.clone()).unwrap_or_default(),
            r.map(|t| t.schema.clone()).unwrap_or_default(),
        ];
        let compared = if self.structure && !self.table_mode {
            comparison::ddl(&schemas[0].ddl, &schemas[1].ddl)
        } else if self.structure {
            comparison::fields_with_options(
                &schemas[0],
                &schemas[1],
                comparison::CompareOptions {
                    ignore_comments: self.ignore_comments,
                    only_differences: self.only_differences,
                },
            )
        } else {
            Ok(comparison::data(
                &l.and_then(|t| t.rows.clone()).unwrap_or_default(),
                &schemas[0],
                &r.and_then(|t| t.rows.clone()).unwrap_or_default(),
                &schemas[1],
            ))
        };
        let reasons = self.table_reasons.get(key).cloned().unwrap_or_default();
        self.reasons_view = self.structure;
        self.selected_table = Some(key.into());
        self.schemas = Some(schemas.clone());
        self.ddl = [schemas[0].ddl.clone(), schemas[1].ddl.clone()];
        if let Some(targets) = &mut self.compared_targets {
            for (i, table) in [l, r].iter().enumerate() {
                if let Some(table) = table {
                    targets[i].object = table.object.clone();
                } else {
                    targets[i].object.name = key.rsplit('.').next().unwrap_or(key).into();
                }
            }
        }
        match compared {
            Ok(c) => {
                let differences = if self.structure && !self.busy {
                    comparison::structure_details(
                        self.sides[0].target.kind,
                        &schemas[0],
                        &schemas[1],
                        comparison::CompareOptions {
                            ignore_comments: self.ignore_comments,
                            only_differences: true,
                        },
                        self.ignore_column_order,
                        "",
                    )
                    .map(|rows| rows.values.len())
                    .unwrap_or(c.different)
                } else {
                    c.different
                };
                self.set_grids(c, window, cx);
                self.notice = tr(
                    cx,
                    "compare.database_preview_summary",
                    &[
                        ("tables", &self.table_status.len().to_string()),
                        (
                            "changed",
                            &self
                                .table_status
                                .values()
                                .filter(|v| **v)
                                .count()
                                .to_string(),
                        ),
                        ("table", key),
                        ("rows", &differences.to_string()),
                        (
                            "scope",
                            &t(
                                cx,
                                if self.structure {
                                    "compare.scope_structure"
                                } else {
                                    "compare.scope_sample"
                                },
                            ),
                        ),
                    ],
                )
                .to_string();
            }
            Err(e) => feedback::error(crate::locale::message(cx, &e), cx),
        }
        let summary = reasons
            .iter()
            .filter(|reason| self.structure == (reason.category != "compare.reason.data"))
            .map(|reason| t(cx, reason.category).to_string())
            .collect::<Vec<_>>()
            .join(" / ");
        if !summary.is_empty() {
            self.notice.push_str(" · ");
            self.notice.push_str(&summary);
        }
        if previous.as_deref() != Some(key) && self.saved_views.contains_key(key) {
            let (query, detail, grids, reasons, metadata, table_mode) =
                self.saved_views.get(key).cloned().unwrap();
            self.reasons_view = reasons;
            self.metadata_view = metadata;
            self.table_mode = table_mode;
            self.field_filter
                .update(cx, |input, cx| input.set_value(query, window, cx));
            self.detail_grid = detail;
            self.grids = grids;
        } else {
            self.refresh_detail(window, cx);
        }
        cx.notify();
    }
    pub(crate) fn snapshot(&self, cx: &App) -> [OpenTable; 2] {
        std::array::from_fn(|i| {
            let mut target = self.sides[i].target.clone();
            target.database = self.sides[i].database.read(cx).value().to_string();
            target.object.schema = self.sides[i].schema.read(cx).value().to_string();
            target.object.name = self.sides[i].table.read(cx).value().to_string();
            target
        })
    }
    pub(crate) fn restore_sources(
        &mut self,
        targets: [OpenTable; 2],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for (side, target) in self.sides.iter_mut().zip(targets) {
            side.database.update(cx, |input, cx| {
                input.set_value(target.database.clone(), window, cx)
            });
            side.schema.update(cx, |input, cx| {
                input.set_value(target.object.schema.clone(), window, cx)
            });
            side.table.update(cx, |input, cx| {
                input.set_value(target.object.name.clone(), window, cx)
            });
            side.target = target;
            let mut names = self
                .source
                .upgrade()
                .map(|db| db.read(cx).note_databases(side.target.connection))
                .unwrap_or_default();
            if side.target.kind == DatabaseKind::PostgreSql {
                names.retain(|n| n == &side.target.database);
            }
            side.database_picker.update(cx, |picker, cx| {
                picker.set_items(SearchableVec::new(names), window, cx);
                picker.set_selected_index(None, window, cx);
            });
        }
        cx.notify();
    }

    pub fn new(
        source: WeakEntity<DatabaseView>,
        target: OpenTable,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let structure = target.mode == crate::table_view::Mode::StructureCompare;
        let names = source
            .upgrade()
            .map(|db| db.read(cx).note_databases(target.connection))
            .unwrap_or_default();
        let sides = std::array::from_fn(|_| Side {
            database_picker: cx.new(|cx| {
                SelectState::new(SearchableVec::new(names.clone()), None, window, cx)
                    .searchable(true)
            }),
            table_picker: None,
            table_subscription: None,
            picker_open: false,
            database: cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(target.database.clone())
                    .placeholder(t(cx, "compare.database"))
            }),
            schema: cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(target.object.schema.clone())
                    .placeholder(t(cx, "compare.schema"))
            }),
            table: cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(target.object.name.clone())
                    .placeholder(t(cx, "compare.table"))
            }),
            target: target.clone(),
        });
        let mut subscriptions = vec![];
        for (i, side) in sides.iter().enumerate() {
            for input in [&side.database, &side.schema, &side.table] {
                let mut previous = input.read(cx).value();
                subscriptions.push(cx.subscribe(input, move |this, input, event, cx| {
                    if matches!(event, gpui_component::input::InputEvent::Change) {
                        let value = input.read(cx).value();
                        if value == previous {
                            return;
                        }
                        previous = value;
                        this.invalidate();
                        cx.emit(crate::table_view::TableDraftChanged);
                        cx.notify();
                    }
                }));
            }
            subscriptions.push(cx.subscribe_in(
                &side.database_picker,
                window,
                move |this, _, event: &SelectEvent<SearchableVec<String>>, window, cx| {
                    if (i == 0 && !this.whole_database) || this.busy {
                        return;
                    }
                    if let SelectEvent::Confirm(Some(value)) = event {
                        this.sides[i]
                            .database
                            .update(cx, |input, cx| input.set_value(value.clone(), window, cx));
                        this.sides[i].table_picker = None;
                        this.sides[i].table_subscription = None;
                        this.invalidate();
                        cx.emit(crate::table_view::TableDraftChanged);
                        cx.notify();
                    }
                },
            ));
        }
        let table_filter =
            cx.new(|cx| InputState::new(window, cx).placeholder(t(cx, "compare.search_tables")));
        subscriptions.push(cx.observe(&table_filter, |_, _, cx| cx.notify()));
        let field_filter =
            cx.new(|cx| InputState::new(window, cx).placeholder(t(cx, "compare.search_fields")));
        subscriptions.push(
            cx.subscribe_in(&field_filter, window, |this, _, event, window, cx| {
                if matches!(event, gpui_component::input::InputEvent::Change) {
                    this.refresh_detail(window, cx);
                }
            }),
        );
        Self {
            field_filter,
            detail_grid: None,
            table_scroll: UniformListScrollHandle::new(),
            table_class: 4,
            detail_class: 0,
            progress: Default::default(),
            field_position: 0,
            metadata_view: false,
            expanded_fields: Default::default(),
            saved_views: Default::default(),
            source,
            sides,
            structure,
            busy: false,
            compare_control: None,
            whole_database: target.object.name.is_empty(),
            include_data: false,
            ignore_comments: false,
            ignore_column_order: true,
            reasons_view: true,
            table_reasons: Default::default(),
            only_differences: true,
            snapshots: None,
            selected_table: None,
            sync_tables: Default::default(),
            table_status: Default::default(),
            table_filter,
            revision: 0,
            locked: std::rc::Rc::new(std::cell::Cell::new(true)),
            scroll_subscriptions: vec![],
            notice: String::new(),
            grids: [None, None],
            ddl: [String::new(), String::new()],
            table_mode: true,
            schemas: None,
            compared_targets: None,
            _subscriptions: subscriptions,
        }
    }
    fn invalidate(&mut self) {
        if let Some(c) = self.compare_control.take() {
            c.cancel();
        }

        self.detail_grid = None;
        self.expanded_fields.clear();
        self.saved_views.clear();
        self.snapshots = None;
        self.selected_table = None;
        self.sync_tables.clear();
        self.table_status.clear();
        self.table_reasons.clear();
        self.reasons_view = false;
        self.revision += 1;
        self.grids = [None, None];
        self.ddl = [String::new(), String::new()];
        self.schemas = None;
        self.compared_targets = None;
        self.notice.clear();
        self.scroll_subscriptions.clear();
        for side in &mut self.sides {
            side.table_picker = None;
            side.table_subscription = None;
            side.picker_open = false;
        }
    }
    fn open_table_picker(
        &mut self,
        i: usize,
        open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if i == 0 || self.busy {
            return;
        }
        self.sides[i].picker_open = open;
        if open && let Some(source) = self.source.upgrade() {
            let context = (
                self.sides[i].target.connection,
                self.sides[i].database.read(cx).value().trim().to_string(),
                source
                    .read(cx)
                    .note_generation(self.sides[i].target.connection),
            );
            if self.sides[i]
                .table_picker
                .as_ref()
                .is_some_and(|p| !p.read(cx).scope_matches(&context))
            {
                self.sides[i].table_picker = None;
                self.sides[i].table_subscription = None;
            }
        }
        if open && self.sides[i].table_picker.is_none() {
            let db = self.sides[i].database.read(cx).value().trim().to_string();
            let id = self.sides[i].target.connection;
            let Some(source) = self.source.upgrade() else {
                return;
            };
            if db.is_empty() || source.read(cx).note_connection_state(id) != Some(true) {
                feedback::error(t(cx, "compare.connect_first"), cx);
                self.sides[i].picker_open = false;
                return;
            }
            let generation = source.read(cx).note_generation(id);
            let paged = source.read(cx).paged_catalog_supported(id);
            let picker = cx.new(|cx| {
                crate::ddl_picker::Picker::new(
                    self.source.clone(),
                    id,
                    db,
                    generation,
                    paged,
                    window,
                    cx,
                )
                .inline()
            });
            self.sides[i].table_subscription = Some(cx.subscribe_in(
                &picker,
                window,
                move |this, _, event: &crate::ddl_picker::Selected, window, cx| {
                    let side = &this.sides[i];
                    if this.busy
                        || event.scope.0 != side.target.connection
                        || event.scope.1 != side.database.read(cx).value().as_ref()
                        || this.source.upgrade().is_none_or(|db| {
                            db.read(cx).note_generation(event.scope.0) != event.scope.2
                        })
                    {
                        return;
                    }
                    this.sides[i].target.object = event.object.clone();
                    this.sides[i].table.update(cx, |input, cx| {
                        input.set_value(event.object.name.clone(), window, cx)
                    });
                    this.sides[i].schema.update(cx, |input, cx| {
                        input.set_value(event.object.schema.clone(), window, cx)
                    });
                    this.invalidate();
                    cx.emit(crate::table_view::TableDraftChanged);
                    cx.notify();
                },
            ));
            self.sides[i].table_picker = Some(picker);
        }
        if open && let Some(picker) = self.sides[i].table_picker.clone() {
            cx.defer_in(window, move |_, window, cx| {
                picker.update(cx, |picker, cx| picker.focus_search(window, cx))
            });
        }
        cx.notify();
    }
    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if self.whole_database {
            self.start_database(window, cx);
            return;
        }
        let Some(source) = self.source.upgrade() else {
            return;
        };
        let mut targets = vec![];
        for side in &self.sides {
            let mut target = side.target.clone();
            target.database = side.database.read(cx).value().trim().to_owned();
            target.object.schema = side.schema.read(cx).value().trim().to_owned();
            target.object.name = side.table.read(cx).value().trim().to_owned();
            if target.database.is_empty()
                || target.object.name.is_empty()
                || target.object.name.len() > 256
                || target.database.len() > 256
                || target.object.schema.len() > 256
            {
                feedback::error(t(cx, "compare.required"), cx);
                return;
            }
            let db = source.read(cx);
            let Some(profile) = db.note_profile(target.connection) else {
                return;
            };
            if !matches!(
                profile.kind,
                DatabaseKind::MySql
                    | DatabaseKind::PostgreSql
                    | DatabaseKind::Sqlite
                    | DatabaseKind::ClickHouse
            ) {
                feedback::error(t(cx, "compare.unsupported"), cx);
                return;
            }
            if profile.kind == DatabaseKind::PostgreSql && target.database != profile.database {
                feedback::error(t(cx, "compare.postgres_database"), cx);
                return;
            }
            if db.note_connection_state(target.connection) != Some(true) {
                feedback::error(t(cx, "compare.connect_first"), cx);
                return;
            }
            target.generation = db.note_generation(target.connection);
            target.kind = profile.kind;
            target.mysql_raw_gbk = profile.mysql_raw_gbk;
            targets.push(target);
        }
        self.busy = true;
        self.notice.clear();
        self.grids = [None, None];
        self.ddl = [String::new(), String::new()];
        self.schemas = None;
        self.compared_targets = None;
        cx.notify();
        let structure = self.structure;
        let revision = self.revision;
        cx.spawn_in(window, async move |view, cx| {
            let mut replies = vec![];
            let mut failure = None;
            for target in &targets {
                let action = if structure {
                    Action::Describe(target.object.clone())
                } else {
                    Action::GridPage {
                        object: target.object.clone(),
                        options: PageOptions {
                            page_size: 200,
                            ..Default::default()
                        },
                    }
                };
                let rx = cx.update(|_, cx| {
                    source.update(cx, |source, cx| {
                        source.mcp_request(
                            target.connection,
                            target.generation,
                            target.database.clone(),
                            action,
                            cx,
                        )
                    })
                });
                let result = match rx {
                    Ok(rx) => rx.recv().await.unwrap_or_else(|_| Err("mcp.stale".into())),
                    Err(_) => return,
                };
                match result {
                    Ok(reply) => replies.push(reply),
                    Err(error) => {
                        failure = Some(error);
                        break;
                    }
                }
            }
            let _ = cx.update(|window, cx| {
                view.update(cx, |this, cx| {
                    this.busy = false;
                    if this.revision != revision {
                        cx.notify();
                        return;
                    }
                    if targets.iter().any(|target| {
                        source.read(cx).note_generation(target.connection) != target.generation
                            || source.read(cx).note_connection_state(target.connection)
                                != Some(true)
                    }) {
                        this.notice = t(cx, "dbwork.context.changed").to_string();
                        cx.notify();
                        return;
                    }
                    if let Some(error) = failure {
                        this.notice = crate::locale::message(cx, &error).to_string();
                        feedback::error(this.notice.clone(), cx);
                        cx.notify();
                        return;
                    }
                    let mut values: Vec<(Rows, Schema, bool)> = vec![];
                    for reply in replies {
                        match reply {
                            Reply::Schema(s) => values.push((Rows::default(), s, false)),
                            Reply::GridPage {
                                rows, schema, more, ..
                            } => values.push((rows, schema, more)),
                            _ => {
                                this.notice = t(cx, "mcp.failed").to_string();
                                cx.notify();
                                return;
                            }
                        }
                    }
                    if values.len() != 2 {
                        return;
                    }
                    this.ddl = [values[0].1.ddl.clone(), values[1].1.ddl.clone()];
                    this.schemas = Some([values[0].1.clone(), values[1].1.clone()]);
                    this.compared_targets = Some([targets[0].clone(), targets[1].clone()]);
                    let result = if structure && this.table_mode {
                        comparison::fields_with_options(
                            &values[0].1,
                            &values[1].1,
                            comparison::CompareOptions {
                                ignore_comments: this.ignore_comments,
                                only_differences: this.only_differences,
                            },
                        )
                    } else if structure {
                        comparison::ddl(&this.ddl[0], &this.ddl[1])
                    } else {
                        Ok(comparison::data(
                            &values[0].0,
                            &values[0].1,
                            &values[1].0,
                            &values[1].1,
                        ))
                    };
                    match result {
                        Ok(compared) => {
                            this.notice = if structure {
                                tr(
                                    cx,
                                    "compare.ddl_summary",
                                    &[("different", &compared.different.to_string())],
                                )
                            } else {
                                tr(
                                    cx,
                                    if compared.keyed {
                                        "compare.key_summary"
                                    } else {
                                        "compare.row_summary"
                                    },
                                    &[
                                        ("left", &values[0].0.values.len().to_string()),
                                        ("right", &values[1].0.values.len().to_string()),
                                        ("different", &compared.different.to_string()),
                                    ],
                                )
                            }
                            .to_string();
                            if !structure && (values[0].2 || values[1].2) {
                                this.notice
                                    .push_str(&format!(" · {}", t(cx, "compare.more")));
                            }
                            this.set_grids(compared, window, cx);
                        }
                        Err(error) => this.notice = crate::locale::message(cx, &error).to_string(),
                    }
                    cx.notify();
                })
            });
        })
        .detach();
    }
    fn set_grids(
        &mut self,
        mut compared: comparison::Compared,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.only_differences {
            comparison::filter_differences(&mut compared);
        }
        if self.structure {
            self.notice = tr(
                cx,
                "compare.ddl_summary",
                &[("different", &compared.different.to_string())],
            )
            .to_string();
        }
        let width = compared
            .left
            .values
            .iter()
            .chain(&compared.right.values)
            .filter_map(|r| r.get(1).and_then(Option::as_ref))
            .map(|s| unicode_width::UnicodeWidthStr::width(s.as_str()))
            .max()
            .unwrap_or(80)
            .saturating_mul(8)
            .clamp(800, 32700) as f32;
        let mut side_index = 0;
        let grids = [compared.left, compared.right].map(|mut rows| {
            if self.structure && self.table_mode {
                for (name, key) in rows.columns.iter_mut().skip(1).zip([
                    "ddl.column.name",
                    "grid.structure.type",
                    "ddl.column.not_null",
                    "ddl.column.auto_increment",
                    "ddl.column.primary",
                    "ddl.column.extra",
                    "ddl.column.comment",
                ]) {
                    *name = t(cx, key).to_string();
                }
            }
            let side = side_index;
            side_index += 1;
            cx.new(|cx| {
                TableState::new(
                    if self.structure && !self.table_mode {
                        QueryGrid::new(rows, 0)
                            .with_column_widths(&[52., width])
                            .with_differences(side)
                    } else if self.structure {
                        QueryGrid::new(rows, 0)
                            .with_column_widths(&[52., 160., 160., 70., 70., 70., 220., 280.])
                            .with_differences(side)
                    } else {
                        QueryGrid::new(rows, 0).with_differences(side)
                    },
                    window,
                    cx,
                )
            })
        });
        let link = std::rc::Rc::new(crate::query_grid::ScrollLink::new(
            &grids,
            self.locked.clone(),
            cx,
        ));
        self.scroll_subscriptions.clear();
        for (i, grid) in grids.iter().enumerate() {
            grid.update(cx, |grid, _| {
                grid.delegate_mut().scroll_link = Some((link.clone(), i))
            });
            let link = link.clone();
            self.scroll_subscriptions
                .push(cx.observe(grid, move |_, _, cx| {
                    if link.synchronize(i) {
                        cx.notify();
                    }
                }));
        }
        self.grids = grids.map(Some);
    }
    fn toggle_structure(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.table_mode = !self.table_mode;
        if let Some(schemas) = &self.schemas {
            let result = if self.table_mode {
                comparison::fields(&schemas[0], &schemas[1])
            } else {
                comparison::ddl(&self.ddl[0], &self.ddl[1])
            };
            match result {
                Ok(result) => self.set_grids(result, window, cx),
                Err(error) => feedback::error(crate::locale::message(cx, &error), cx),
            }
        }
        cx.notify();
    }
    fn sync_preview(&self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(schemas), Some(targets)) = (&self.schemas, &self.compared_targets) else {
            return;
        };
        if targets[0].kind != targets[1].kind {
            feedback::error(t(cx, "compare.sync_cross_driver"), cx);
            return;
        }
        let missing_right = self.whole_database
            && self
                .snapshots
                .as_ref()
                .zip(self.selected_table.as_ref())
                .is_some_and(|(s, k)| comparison::database_find(&s[1], k).is_none());
        let missing_left = self.whole_database
            && self
                .snapshots
                .as_ref()
                .zip(self.selected_table.as_ref())
                .is_some_and(|(s, k)| comparison::database_find(&s[0], k).is_none());
        if missing_left {
            feedback::error(t(cx, "compare.no_drop_default"), cx);
            return;
        }
        let filtered = schemas.clone();
        let mut draft = if self.whole_database && !self.sync_tables.is_empty() {
            let snapshots = self.snapshots.as_ref().unwrap();
            let mut sql = String::new();
            let mut incomplete = false;
            let mut issues = vec![];
            for key in &self.sync_tables {
                let l = comparison::database_find(&snapshots[0], key);
                let r = comparison::database_find(&snapshots[1], key);
                let Some(l) = l else {
                    incomplete = true;
                    continue;
                };
                let object = r.map(|t| &t.object).unwrap_or(&l.object);
                let result = if !self.structure {
                    r.ok_or_else(|| "compare.data_sync_unavailable".to_string())
                        .and_then(|r| {
                            comparison::data_sync(
                                targets[1].kind,
                                &targets[1].database,
                                object,
                                l,
                                r,
                            )
                        })
                        .map(|sql| comparison::SyncDraft {
                            sql,
                            incomplete: l.more || r.is_some_and(|r| r.more),
                            issues: vec![],
                        })
                } else if let Some(r) = r {
                    (if self.ignore_comments {
                        comparison::sync_preserving_comments
                    } else {
                        comparison::sync_ddl
                    })(
                        targets[1].kind,
                        &targets[1].database,
                        object,
                        &r.schema,
                        &l.schema,
                    )
                } else {
                    comparison::create_table_draft(
                        targets[1].kind,
                        &targets[1].database,
                        object,
                        &l.schema,
                    )
                };
                match result {
                    Ok(d) => {
                        sql.push_str(&format!("-- Table: {}\n", key.replace(['\n', '\r'], " ")));
                        sql.push_str(&d.sql);
                        sql.push('\n');
                        incomplete |= d.incomplete;
                        issues.extend(d.issues);
                    }
                    Err(e) => {
                        feedback::error(crate::locale::message(cx, &e), cx);
                        return;
                    }
                }
                if sql.len() > 1024 * 1024 {
                    feedback::error(t(cx, "compare.ddl_limit"), cx);
                    return;
                }
            }
            comparison::SyncDraft {
                sql,
                incomplete,
                issues,
            }
        } else if missing_right {
            match comparison::create_table_draft(
                targets[1].kind,
                &targets[1].database,
                &targets[1].object,
                &filtered[0],
            ) {
                Ok(d) => d,
                Err(e) => {
                    feedback::error(crate::locale::message(cx, &e), cx);
                    return;
                }
            }
        } else if !self.structure {
            let Some(snapshots) = &self.snapshots else {
                return;
            };
            let Some(key) = &self.selected_table else {
                return;
            };
            let Some((l, r)) = comparison::database_find(&snapshots[0], key)
                .zip(comparison::database_find(&snapshots[1], key))
            else {
                return;
            };
            match comparison::data_sync(
                targets[1].kind,
                &targets[1].database,
                &targets[1].object,
                l,
                r,
            ) {
                Ok(sql) => comparison::SyncDraft {
                    sql,
                    incomplete: l.more || r.more,
                    issues: vec![],
                },
                Err(e) => {
                    feedback::error(crate::locale::message(cx, &e), cx);
                    return;
                }
            }
        } else {
            match (if self.ignore_comments {
                comparison::sync_preserving_comments
            } else {
                comparison::sync_ddl
            })(
                targets[1].kind,
                &targets[1].database,
                &targets[1].object,
                &filtered[1],
                &filtered[0],
            ) {
                Ok(draft) => draft,
                Err(error) => {
                    feedback::error(crate::locale::message(cx, &error), cx);
                    return;
                }
            }
        };
        if self.structure
            && let Err(e) = comparison::omit_column_deletions(targets[1].kind, &mut draft)
        {
            feedback::error(crate::locale::message(cx, &e), cx);
            return;
        }
        let issues = draft
            .issues
            .iter()
            .map(|item| {
                if matches!(
                    item.key,
                    "compare.issue.column" | "compare.issue.comment" | "compare.issue.parse_column"
                ) {
                    tr(cx, item.key, &[("column", item.column.as_str())])
                } else {
                    t(cx, item.key)
                }
            })
            .collect::<Vec<_>>();
        let labels = targets.each_ref().map(|target| {
            let connection = self
                .source
                .upgrade()
                .and_then(|db| {
                    db.read(cx)
                        .note_profile(target.connection)
                        .map(|p| p.name.clone())
                })
                .unwrap_or_else(|| target.connection.to_string());
            format!(
                "{} / {}",
                connection,
                if self.whole_database {
                    target.database.clone()
                } else {
                    turbodbn_services::database::workbench::qualified(
                        target.kind,
                        &target.database,
                        &target.object,
                    )
                }
            )
        });
        let direction = format!("{}  →  {}", labels[0], labels[1]);
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .code_editor("sql")
                .default_value(draft.sql.clone())
        });
        let sql = draft.sql;
        window.open_dialog(cx, move |dialog, _, cx| {
            let sql = sql.clone();
            dialog
                .title(t(cx, "compare.sync_title"))
                .w(px(820.))
                .overlay_closable(false)
                .child(
                    div()
                        .v_flex()
                        .gap_3()
                        .child(direction.clone())
                        .child(t(cx, "compare.sync_hint"))
                        .when(draft.incomplete, |el| {
                            el.child(
                                div()
                                    .text_color(cx.theme().warning)
                                    .child(t(cx, "compare.sync_incomplete")),
                            )
                        })
                        .when(!issues.is_empty(), |el| {
                            el.child(
                                div()
                                    .id("sync-manual-review")
                                    .v_flex()
                                    .gap_1()
                                    .max_h(px(120.))
                                    .overflow_y_scroll()
                                    .children(issues.iter().map(|message| {
                                        div().text_sm().whitespace_normal().child(message.clone())
                                    })),
                            )
                        })
                        .child(Input::new(&input).disabled(true).h(px(360.)))
                        .child(
                            feedback::actions().child(feedback::close_button(cx))
                            .child(Button::new("save-sync-sql").icon(IconName::FolderOpen).label(t(cx,"compare.save_sql")).on_click({let sql=sql.clone();move|_,_window,cx|{
                                let directory=dirs::document_dir().unwrap_or_else(||std::path::PathBuf::from("."));
                                let name=format!("database-diff_{}.sql",chrono::Local::now().format("%Y%m%d_%H%M%S"));
                                let pick=cx.prompt_for_new_path(&directory,Some(&name));let sql=sql.clone();
                                cx.spawn(async move|cx|{if let Ok(Ok(Some(path)))=pick.await {
                                    let result=cx.background_executor().spawn(async move{use std::io::Write;let mut temp=tempfile::NamedTempFile::new_in(path.parent().ok_or("compare.save_failed")?).map_err(|_|"compare.save_failed")?;temp.write_all(sql.as_bytes()).map_err(|_|"compare.save_failed")?;temp.persist_noclobber(path).map_err(|_|"compare.save_failed")?;Ok::<_,&str>(())}).await;
                                    let _=cx.update(|cx|match result{Ok(())=>feedback::success(t(cx,"feedback.saved"),cx),Err(_)=>feedback::error(t(cx,"compare.save_failed"),cx)});
                                }}).detach();
                            }}))
                            .child(
                                Button::new("copy-sync-ddl")
                                    .icon(IconName::Copy)
                                    .label(t(cx, "dbwork.copy.ddl"))
                                    .primary()
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            sql.clone(),
                                        ));
                                        feedback::success(t(cx, "feedback.copied"), cx);
                                    }),
                            ),
                        ),
                )
        });
    }
    fn side(&self, i: usize, cx: &Context<Self>) -> AnyElement {
        let side = &self.sides[i];
        let source = self.source.upgrade();
        let profile = source
            .as_ref()
            .and_then(|s| s.read(cx).note_profile(side.target.connection))
            .cloned();
        let profiles = source
            .as_ref()
            .map(|s| {
                s.read(cx)
                    .note_profiles()
                    .into_iter()
                    .filter(|p| {
                        s.read(cx).note_connection_state(p.id) == Some(true)
                            && matches!(
                                p.kind,
                                DatabaseKind::MySql
                                    | DatabaseKind::PostgreSql
                                    | DatabaseKind::Sqlite
                                    | DatabaseKind::ClickHouse
                            )
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let owner = cx.entity().downgrade();
        let busy = self.busy;
        let picker = Button::new(("compare-connection", i))
            .ghost()
            .justify_start()
            .w_full()
            .disabled(busy || (i == 0 && !self.whole_database))
            .dropdown_caret(true)
            .when_some(profile.clone(), |b, p| {
                b.child(crate::connection_choice::identity(&p))
            })
            .dropdown_menu(move |mut menu, _, _| {
                for profile in &profiles {
                    let profile = profile.clone();
                    let owner = owner.clone();
                    menu = menu.item(crate::connection_choice::item(&profile).on_click(
                        move |_, window, cx| {
                            let _ = owner.update(cx, |this, cx| {
                                if this.busy || (i == 0 && !this.whole_database) {
                                    return;
                                }
                                let side = &mut this.sides[i];
                                side.target.connection = profile.id;
                                side.target.kind = profile.kind;
                                side.database.update(cx, |input, cx| {
                                    input.set_value(
                                        if profile.kind == DatabaseKind::Sqlite
                                            && profile.database.is_empty()
                                        {
                                            "main".into()
                                        } else {
                                            profile.database.clone()
                                        },
                                        window,
                                        cx,
                                    )
                                });
                                side.schema.update(cx, |input, cx| {
                                    input.set_value(
                                        if profile.kind == DatabaseKind::PostgreSql {
                                            "public"
                                        } else {
                                            ""
                                        },
                                        window,
                                        cx,
                                    )
                                });
                                let mut names = this
                                    .source
                                    .upgrade()
                                    .map(|db| db.read(cx).note_databases(profile.id))
                                    .unwrap_or_default();
                                if profile.kind == DatabaseKind::PostgreSql {
                                    names.retain(|n| n == &profile.database);
                                }
                                this.sides[i].database_picker.update(cx, |picker, cx| {
                                    picker.set_items(SearchableVec::new(names), window, cx)
                                });
                                this.invalidate();
                                cx.emit(crate::table_view::TableDraftChanged);
                                cx.notify();
                            });
                        },
                    ));
                }
                menu.scrollable(true).max_h(px(320.))
            });
        let body =
            div()
                .v_flex()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .gap_2()
                .child(
                    div()
                        .h(px(24.))
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(t(
                            cx,
                            if self.whole_database && i == 0 {
                                "compare.reference_database"
                            } else if self.whole_database {
                                "compare.target_database"
                            } else if i == 0 {
                                "compare.current"
                            } else {
                                "compare.target"
                            },
                        )),
                )
                .child(picker)
                .child(
                    div()
                        .h_flex()
                        .w_full()
                        .min_w_0()
                        .gap_1()
                        .child(div().flex_1().min_w_0().child(
                            Input::new(&side.database).cleanable(true).disabled(
                                busy || (i == 0 && !self.whole_database)
                                    || profile.as_ref().is_some_and(|p| {
                                        matches!(
                                            p.kind,
                                            DatabaseKind::PostgreSql | DatabaseKind::Sqlite
                                        )
                                    }),
                            ),
                        ))
                        .child(
                            div().w(px(32.)).flex_shrink_0().child(
                                Select::new(&side.database_picker)
                                    .w_full()
                                    .menu_width(px(320.))
                                    .placeholder("")
                                    .search_placeholder(t(cx, "dbwork.database.filter"))
                                    .disabled(
                                        busy || (i == 0 && !self.whole_database)
                                            || profile.as_ref().is_some_and(|p| {
                                                matches!(
                                                    p.kind,
                                                    DatabaseKind::PostgreSql | DatabaseKind::Sqlite
                                                )
                                            }),
                                    ),
                            ),
                        ),
                )
                .when(
                    self.sides
                        .iter()
                        .any(|s| s.target.kind == DatabaseKind::PostgreSql),
                    |b| {
                        b.child(Input::new(&side.schema).cleanable(true).disabled(
                            busy || i == 0 || side.target.kind != DatabaseKind::PostgreSql,
                        ))
                    },
                )
                .when(!self.whole_database, |el| {
                    el.child(
                        div()
                            .h_flex()
                            .w_full()
                            .min_w_0()
                            .gap_1()
                            .child(
                                div().flex_1().min_w_0().child(
                                    Input::new(&side.table)
                                        .cleanable(true)
                                        .disabled(busy || (i == 0 && !self.whole_database)),
                                ),
                            )
                            .child(
                                Popover::new(("compare-table-picker", i))
                                    .open(side.picker_open)
                                    .trigger(
                                        Button::new(("compare-table-arrow", i))
                                            .ghost()
                                            .icon(IconName::ChevronDown)
                                            .tooltip(t(cx, "ddl.select_table"))
                                            .disabled(busy || (i == 0 && !self.whole_database)),
                                    )
                                    .on_open_change({
                                        let owner = cx.entity().downgrade();
                                        move |open, window, cx| {
                                            let _ = owner.update(cx, |this, cx| {
                                                this.open_table_picker(i, *open, window, cx)
                                            });
                                        }
                                    })
                                    .content({
                                        let owner = cx.entity().downgrade();
                                        move |_, _, cx| {
                                            owner
                                                .upgrade()
                                                .and_then(|owner| {
                                                    owner.read(cx).sides[i].table_picker.clone()
                                                })
                                                .map(|picker| div().child(picker))
                                                .unwrap_or_else(div)
                                        }
                                    }),
                            ),
                    )
                });
        body.into_any_element()
    }
}

impl Drop for Comparison {
    fn drop(&mut self) {
        if let Some(c) = &self.compare_control {
            c.cancel();
        }
    }
}

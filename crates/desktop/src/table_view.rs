#[path = "grid_filter.rs"]
mod filter_ui;
#[path = "data_grid.rs"]
mod grid_ui;
#[path = "mongo_workbench.rs"]
mod mongo_ui;
#[path = "note_results.rs"]
mod note_results;
#[path = "redis_editor.rs"]
mod redis_editor;
#[path = "grid_value.rs"]
mod value_ui;
#[path = "grid_width.rs"]
mod width_ui;
use crate::{
    database::{DatabaseView, OpenTable},
    locale::t,
};
use gpui::{prelude::*, *};
use gpui_component::table::{Table, TableState};
use gpui_component::{
    ActiveTheme, Disableable, IconName, Selectable, Sizable, StyledExt, WindowExt,
    button::{Button, ButtonVariants},
    color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState},
    dock::{Panel, PanelEvent},
    input::{Input, InputEvent, InputState},
    menu::{DropdownMenu, PopupMenuItem},
    scroll::{Scrollbar, ScrollbarShow},
};
pub(crate) use grid_ui::column_icon;
use std::{collections::BTreeMap, rc::Rc};
use turbodbn_services::database::workbench::{self, Action, Reply, Request, Rows, Schema};
use workbench::{Change, Filter, FilterJoin, FilterSet, PageOptions};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Grid,
    Note,
    Er,
    Compare,
    Import,
    Export,
}
pub struct TableView {
    mongo_dirty: bool,
    mongo_preview: Option<turbodbn_core::mongo::Mutation>,
    mongo: Option<std::sync::Arc<turbodbn_core::mongo::Projection>>,
    mongo_tree: Option<Entity<crate::mongo_tree::MongoTree>>,
    mongo_subscription: Option<Subscription>,
    mongo_inspect: bool,
    mongo_epoch: u64,
    widths: width_ui::Layout,
    widths_writable: bool,
    width_revision: u64,
    width_sender: Option<async_channel::Sender<width_ui::Layout>>,
    notebook: Option<note_results::NotebookContext>,
    redis_cursor: String,
    redis_kind: String,
    redis_ttl: i64,
    redis_read_at: std::time::Instant,
    redis_truncated: bool,
    redis_clock: Option<Task<()>>,
    document: Option<Entity<crate::result_document::ResultDocument>>,
    json_mode: bool,
    key: String,
    source: WeakEntity<DatabaseView>,
    target: OpenTable,
    focus: FocusHandle,
    rows: Rows,
    schema: Option<Schema>,
    offset: usize,
    more: bool,
    busy: bool,
    counting: bool,
    notice: String,
    sql: String,
    note: Entity<InputState>,
    comparison: Entity<InputState>,
    _note: Subscription,
    grid_state: Entity<TableState<grid_ui::GridDelegate>>,
    options: PageOptions,
    filter_draft: Option<filter_ui::Draft>,
    last_options: PageOptions,
    anchors: BTreeMap<usize, Option<Vec<String>>>,
    page_size_input: Entity<InputState>,
    page_number_input: Entity<InputState>,
    sync_pager_inputs: bool,
    _pager_subscriptions: Vec<Subscription>,
    elapsed_ms: u64,
    total: Option<u64>,
    keyset: bool,
    selected: Option<(usize, usize)>,
    highlight: bool,
    highlight_color: Option<Hsla>,
    color_picker: Entity<ColorPickerState>,
    _color_subscription: Subscription,
    changes: BTreeMap<usize, Change>,
    insert_count: usize,
    committing: bool,
    baseline: Option<Rows>,
}
impl TableView {
    pub(crate) fn mongo_unsaved(&self) -> bool {
        self.mongo_dirty || (self.mongo.is_some() && self.committing)
    }
    pub(crate) fn discard_mongo(&mut self, cx: &mut Context<Self>) {
        if self.committing {
            return;
        }
        self.mongo_dirty = false;
        self.mongo_preview = None;
        if let Some(tree) = &self.mongo_tree {
            tree.update(cx, |tree, cx| tree.finish(true, cx));
        }
    }
    pub(crate) fn update_blocked(&self) -> bool {
        !self.changes.is_empty()
            || self.committing
            || self.busy
            || self.counting
            || self.mongo_pending()
    }
    pub fn new(
        key: String,
        source: WeakEntity<DatabaseView>,
        target: OpenTable,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::create(key, source, target, true, window, cx)
    }
    fn create(
        key: String,
        source: WeakEntity<DatabaseView>,
        target: OpenTable,
        autoload: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.defer_in(window, |this, _, cx| this.init_widths(cx));
        let note = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .placeholder(t(cx, "dbwork.note.placeholder"))
        });
        let comparison =
            cx.new(|cx| InputState::new(window, cx).placeholder(t(cx, "dbwork.compare.target")));
        if autoload {
            cx.defer_in(window, |this, _, cx| {
                this.load(cx);
            });
        }
        let page_size = cx
            .global::<crate::preferences::Preferences>()
            .database
            .page_size
            .clamp(1, 1000);
        let page_size_input =
            cx.new(|cx| InputState::new(window, cx).default_value(page_size.to_string()));
        let page_number_input = cx.new(|cx| InputState::new(window, cx).default_value("1"));
        let pager_subscriptions = [
            (page_size_input.clone(), true),
            (page_number_input.clone(), false),
        ]
        .into_iter()
        .map(|(input, size)| {
            cx.subscribe_in(
                &input,
                window,
                move |_, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        cx.defer_in(window, move |this, _, cx| this.apply_pager_input(size, cx));
                    }
                },
            )
        })
        .collect();
        let grid_state = cx.new(|cx| {
            TableState::new(grid_ui::GridDelegate::new(), window, cx)
                .row_selectable(false)
                .col_selectable(false)
                .col_movable(false)
                .col_resizable(false)
        });
        let highlight_color = default_highlight_color(cx);
        let color_picker =
            cx.new(|cx| ColorPickerState::new(window, cx).default_value(highlight_color));
        let color_subscription =
            cx.subscribe(&color_picker, |this, _, event: &ColorPickerEvent, cx| {
                let ColorPickerEvent::Change(color) = event;
                this.highlight_color = *color;
                this.highlight = true;
                this.sync_selection(cx);
            });
        Self {
            mongo_dirty: false,
            mongo_preview: None,
            mongo: None,
            mongo_tree: None,
            mongo_subscription: None,
            mongo_inspect: false,
            mongo_epoch: 0,
            widths: Default::default(),
            widths_writable: true,
            width_revision: 0,
            width_sender: None,
            notebook: None,
            redis_cursor: "0".into(),
            redis_kind: String::new(),
            redis_ttl: -2,
            redis_read_at: std::time::Instant::now(),
            redis_truncated: false,
            redis_clock: None,
            document: None,
            json_mode: false,
            grid_state,
            options: PageOptions {
                page_size,
                ..PageOptions::default()
            },
            filter_draft: None,
            last_options: PageOptions::default(),
            anchors: BTreeMap::new(),
            page_size_input,
            page_number_input,
            sync_pager_inputs: false,
            _pager_subscriptions: pager_subscriptions,
            elapsed_ms: 0,
            total: None,
            keyset: false,
            selected: None,
            highlight: true,
            highlight_color: None,
            color_picker,
            _color_subscription: color_subscription,
            changes: BTreeMap::new(),
            insert_count: 0,
            committing: false,
            key,
            source,
            target,
            focus: cx.focus_handle(),
            rows: Rows::default(),
            schema: None,
            offset: 0,
            more: false,
            busy: false,
            counting: false,
            notice: String::new(),
            sql: String::new(),
            _note: cx.observe(&note, |_, _, cx| cx.notify()),
            note,
            comparison,
            baseline: None,
        }
    }
    fn request(&mut self, action: Action, cx: &mut Context<Self>) {
        if self.notebook.is_some() {
            self.note_request(action, cx);
            return;
        }
        if self.busy {
            return;
        }
        let paging = matches!(&action, Action::GridPage { .. });
        let counting = matches!(&action, Action::Count { .. });
        let target = cx.entity().downgrade();
        let request = Request {
            database: self.target.database.clone(),
            action,
        };
        match self.source.update(cx, |source, cx| {
            source.table_request(
                self.target.connection,
                self.target.generation,
                request,
                target,
                cx,
            )
        }) {
            Ok(Ok(())) => {
                self.busy = true;
                self.counting = counting;
                self.notice = if self.target.mode == Mode::Grid {
                    String::new()
                } else {
                    t(cx, "dbwork.loading").to_string()
                };
            }
            Ok(Err(e)) => self.notice = crate::locale::message(cx, &e).to_string(),
            Err(_) => self.notice = t(cx, "dbwork.disconnected").to_string(),
        }
        if !self.busy {
            if paging {
                self.sync_pager_inputs = true;
                self.options = self.last_options.clone();
                self.offset = self.options.offset;
            }
            crate::feedback::error(self.notice.clone(), cx);
        }
        cx.notify();
    }
    fn load(&mut self, cx: &mut Context<Self>) {
        if self.mongo_pending() {
            crate::feedback::error(t(cx, "mongo.document.pending"), cx);
            return;
        }
        if self.target.kind == turbodbn_services::DatabaseKind::Redis {
            self.request(
                Action::RedisValue {
                    key: self
                        .target
                        .object
                        .parent
                        .as_bytes()
                        .chunks(2)
                        .filter_map(|c| {
                            std::str::from_utf8(c)
                                .ok()
                                .and_then(|s| u8::from_str_radix(s, 16).ok())
                        })
                        .collect(),
                    cursor: self.redis_cursor.clone(),
                    offset: self.offset,
                    size: 100,
                },
                cx,
            );
            return;
        }
        if matches!(self.target.mode, Mode::Er | Mode::Import) {
            self.request(Action::Describe(self.target.object.clone()), cx);
        } else {
            if !self.changes.is_empty() {
                crate::feedback::error(t(cx, "grid.pending"), cx);
                return;
            }
            self.options.offset = self.offset;
            self.request(
                Action::GridPage {
                    object: self.target.object.clone(),
                    options: self.options.clone(),
                },
                cx,
            );
        }
    }

    pub fn receive(&mut self, result: Result<Reply, String>, cx: &mut Context<Self>) {
        self.busy = false;
        self.counting = false;
        match result {
            Ok(Reply::MongoPage(page)) => {
                self.receive_mongo(page, cx);
                return;
            }
            Ok(Reply::MongoWritten { inserted }) => {
                self.mongo_dirty = false;
                self.total = None;
                if let Some(tree) = &self.mongo_tree {
                    tree.update(cx, |tree, cx| tree.finish(true, cx));
                }
                self.committing = false;
                crate::feedback::success(
                    t(
                        cx,
                        if inserted {
                            "mongo.document.inserted"
                        } else {
                            "mongo.document.updated"
                        },
                    ),
                    cx,
                );
                let view = cx.entity().downgrade();
                cx.defer(move |cx| {
                    let _ = view.update(cx, |this, cx| this.load(cx));
                });
                return;
            }
            Ok(Reply::RedisValue(page)) => {
                self.redis_kind = page.kind.clone();
                self.redis_ttl = page.ttl;
                self.redis_read_at = std::time::Instant::now();
                self.redis_truncated = page.truncated;
                self.redis_clock = if page.ttl > 0 {
                    Some(cx.spawn(async move |view, cx| {
                        loop {
                            gpui::Timer::after(std::time::Duration::from_secs(1)).await;
                            let Ok(active) = view.update(cx, |this, cx| {
                                cx.notify();
                                this.redis_read_at.elapsed().as_secs()
                                    < this.redis_ttl.max(0) as u64
                            }) else {
                                break;
                            };
                            if !active {
                                break;
                            }
                        }
                    }))
                } else {
                    None
                };
                self.rows = page.rows;
                self.offset = page.offset;
                self.more = page.cursor.is_some();
                self.redis_cursor = page.cursor.unwrap_or_else(|| "0".into());
                self.sql = page.text.unwrap_or_default();
                self.notice = crate::locale::tr(
                    cx,
                    "redis.value_info",
                    &[
                        ("kind", &page.kind),
                        ("ttl", &page.ttl.to_string()),
                        (
                            "scope",
                            if page.truncated {
                                "64 KiB / 1000"
                            } else {
                                "—"
                            },
                        ),
                    ],
                )
                .to_string();
                self.sync_grid(cx);
            }
            Ok(Reply::GridPage {
                rows,
                schema,
                offset,
                more,
                sql,
                next,
                elapsed_ms,
                keyset,
            }) => {
                self.sync_pager_inputs = true;
                self.last_options = self.options.clone();
                self.rows = rows;
                self.schema = Some(schema);
                self.offset = offset;
                self.more = more;
                self.sql = sql;
                self.anchors.insert(offset, self.options.after.clone());
                if more {
                    self.anchors
                        .insert(offset + self.options.page_size, next.clone());
                }
                self.elapsed_ms = elapsed_ms;
                self.keyset = keyset;
                self.selected = None;
                self.notice = String::new();
                self.sync_grid(cx);
                self.grid_state
                    .update(cx, |table, cx| table.scroll_to_row(0, cx));
            }
            Ok(Reply::Count(count)) => {
                self.total = Some(count);
                self.notice.clear();
            }
            Err(e) => {
                if let Some(tree) = &self.mongo_tree {
                    tree.update(cx, |tree, cx| tree.finish(false, cx));
                }
                self.committing = false;
                self.mongo_epoch += 1;
                self.sync_pager_inputs = true;
                self.options = self.last_options.clone();
                self.offset = self.options.offset;
                self.baseline = None;
                self.committing = false;
                crate::feedback::error(e.clone(), cx);
                self.notice = crate::locale::message(cx, &e).to_string();
            }
            Ok(Reply::Page {
                rows,
                offset,
                more,
                sql,
            }) => {
                if let Some(baseline) = self.baseline.take() {
                    if baseline.columns != rows.columns {
                        self.notice = t(cx, "dbwork.compare.columns").to_string();
                    } else {
                        let mut counts = std::collections::HashMap::<String, i64>::new();
                        for row in &baseline.values {
                            *counts
                                .entry(serde_json::to_string(row).unwrap_or_default())
                                .or_default() += 1;
                        }
                        for row in &rows.values {
                            *counts
                                .entry(serde_json::to_string(row).unwrap_or_default())
                                .or_default() -= 1;
                        }
                        let left: i64 = counts.values().filter(|v| **v > 0).sum();
                        let right: i64 = counts.values().filter(|v| **v < 0).map(|v| -v).sum();
                        self.notice = format!(
                            "当前页比较：左侧独有 {left} 行，右侧独有 {right} 行；不是全表比较"
                        );
                    }
                } else {
                    self.rows = rows;
                    self.offset = offset;
                    self.more = more;
                    self.sql = sql;
                    self.notice = t(cx, "dbwork.page.notice").to_string();
                }
            }
            Ok(Reply::Schema(schema)) => {
                self.schema = Some(schema);
                self.notice = String::new();
            }
            Ok(Reply::Executed) => {
                self.notice = if self.committing {
                    crate::locale::tr(
                        cx,
                        "grid.commit.saved",
                        &[("count", &self.changes.len().to_string())],
                    )
                    .to_string()
                } else {
                    t(cx, "dbwork.executed").to_string()
                };
                crate::feedback::success(self.notice.clone(), cx);
                if self.committing || self.target.kind == turbodbn_services::DatabaseKind::Redis {
                    if self.target.kind == turbodbn_services::DatabaseKind::Redis {
                        self.offset = 0;
                        self.redis_cursor = "0".into();
                    }
                    self.committing = false;
                    self.changes.clear();
                    self.insert_count = 0;
                    self.total = None;
                    self.selected = None;
                    self.sync_grid(cx);
                    let view = cx.entity().downgrade();
                    cx.defer(move |cx| {
                        let _ = view.update(cx, |this, cx| this.load(cx));
                    });
                }
            }
            _ => self.notice = t(cx, "dbwork.unexpected").to_string(),
        }
        cx.notify();
    }
    fn compare(&mut self, cx: &mut Context<Self>) {
        let name = self.comparison.read(cx).value().trim().to_string();
        if name.is_empty() {
            return;
        }
        let mut object = self.target.object.clone();
        object.name = name;
        self.baseline = Some(self.rows.clone());
        self.request(
            Action::Page {
                object,
                offset: self.offset,
                page_size: self.options.page_size,
            },
            cx,
        );
        if !self.busy {
            self.baseline = None;
        }
    }
    fn export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mongo_rows = self.mongo.as_ref().map(|data| Rows {
            columns: self.rows.columns.clone(),
            values: data
                .documents
                .iter()
                .map(|doc| {
                    self.rows
                        .columns
                        .iter()
                        .map(|key| doc.get(key).map(ToString::to_string))
                        .collect()
                })
                .collect(),
        });
        match workbench::export_csv(mongo_rows.as_ref().unwrap_or(&self.rows)) {
            Ok(text) => save_text(text, "data-page.csv", window, cx),
            Err(e) => {
                self.notice = crate::locale::message(cx, &e).to_string();
                cx.notify();
            }
        }
    }
    fn import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(schema) = self.schema.clone() else {
            return;
        };
        let target = self.target.clone();
        let pick = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t(cx, "dbwork.import.csv")),
        });
        cx.spawn_in(window, async move |view, cx| {
            if let Ok(Ok(Some(paths))) = pick.await
                && let Some(path) = paths.into_iter().next()
            {
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        use std::io::Read;
                        let mut text = String::new();
                        std::fs::File::open(path)
                            .map_err(|e| e.to_string())?
                            .take(48_001)
                            .read_to_string(&mut text)
                            .map_err(|e| e.to_string())?;
                        workbench::import_csv(
                            target.kind,
                            &target.database,
                            &target.object,
                            &schema.columns,
                            &text,
                        )
                    })
                    .await;
                let _ = cx.update(|window, cx| {
                    let _ = view.update(cx, |this, cx| match result {
                        Ok(sql) => this.preview(sql, window, cx),
                        Err(e) => {
                            this.notice = e;
                            cx.notify();
                        }
                    });
                });
            }
        })
        .detach();
    }
    fn preview(&mut self, sql: String, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity().downgrade();
        let target = format!("{} / {}", self.target.database, self.target.object.name);
        let guard = self
            .schema
            .as_ref()
            .map(|s| (self.target.object.clone(), s.ddl.clone()));
        crate::feedback::sql_preview(sql, target, window, cx, move |sql, _, cx| {
            view.update(cx, |this, cx| {
                this.request(
                    Action::Execute {
                        sql,
                        guard: guard.clone(),
                    },
                    cx,
                );
                this.busy
            })
            .unwrap_or(false)
        });
    }
    fn grid(&self, cx: &Context<Self>) -> AnyElement {
        if self.rows.columns.is_empty() && self.rows.values.is_empty() && self.insert_count == 0 {
            return div()
                .v_flex()
                .size_full()
                .border_1()
                .border_color(cx.theme().border)
                .child(
                    div()
                        .h_flex()
                        .gap_3()
                        .px_2()
                        .py_1()
                        .children(self.rows.columns.iter().take(64).cloned()),
                )
                .child(
                    div()
                        .flex_1()
                        .h_flex()
                        .justify_center()
                        .child(t(cx, "notes.empty_result")),
                )
                .into_any_element();
        }
        let state = self.grid_state.read(cx);
        let vertical = state.vertical_scroll_handle.clone();
        let horizontal = state.horizontal_scroll_handle.clone();
        // Match the component's medium header (32px) and the fixed 58px row-number column.
        // Scrollbar itself suppresses an axis when its content fits the viewport.
        div()
            .relative()
            .size_full()
            .on_drop(width_ui::drop_width)
            .child(Table::new(&self.grid_state).scrollbar_visible(false, false))
            .when(
                !self.rows.values.is_empty() || self.insert_count > 0,
                |grid| {
                    grid.child(
                        div()
                            .absolute()
                            .top(px(32.))
                            .right_0()
                            .bottom_0()
                            .w(px(16.))
                            .child(
                                Scrollbar::vertical(&vertical)
                                    .scrollbar_show(ScrollbarShow::Always),
                            ),
                    )
                },
            )
            .child(
                div()
                    .absolute()
                    .left(px(grid_ui::ROW_NUMBER_WIDTH))
                    .right_0()
                    .bottom_0()
                    .h(px(16.))
                    .child(
                        Scrollbar::horizontal(&horizontal).scrollbar_show(ScrollbarShow::Always),
                    ),
            )
            .into_any_element()
    }
    fn display_rows(&self) -> Rows {
        let mut rows = self.rows.clone();
        rows.values.resize(
            rows.values.len() + self.insert_count,
            vec![None; rows.columns.len()],
        );
        for (row, change) in &self.changes {
            if let Some(values) = &change.values
                && let Some(target) = rows.values.get_mut(*row)
            {
                for (name, value) in values {
                    if let Some(col) = rows.columns.iter().position(|c| c == name) {
                        target[col] = value.clone();
                    }
                }
            }
        }
        rows
    }
    fn show_document(
        &mut self,
        cell: Option<(usize, usize)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.mongo.is_some() {
            if let Some((row, col)) = cell {
                self.open_mongo_node(row, col, cx);
            }
            self.json_mode = true;
            cx.notify();
            return;
        }
        let mut rows = self.display_rows();
        if let Some((r, c)) = cell {
            let Some(value) = rows.values.get(r).and_then(|row| row.get(c)).cloned() else {
                return;
            };
            rows = Rows {
                columns: vec![rows.columns[c].clone()],
                values: vec![vec![value]],
            };
        }
        self.json_mode = true;
        self.document =
            Some(cx.new(|cx| crate::result_document::ResultDocument::new(rows, window, cx)));
        cx.notify();
    }
    fn document_controls(&self, cx: &Context<Self>) -> impl IntoElement {
        let owner = cx.entity().downgrade();
        crate::result_document::switches(self.json_mode, self.busy, cx, move |json, window, cx| {
            let _ = owner.update(cx, |this, cx| {
                if json {
                    this.show_document(None, window, cx);
                } else {
                    this.json_mode = false;
                    this.document = None;
                    cx.notify();
                }
            });
        })
    }
    fn result_content(&self, cx: &Context<Self>) -> AnyElement {
        if let Some(tree) = &self.mongo_tree {
            if self.json_mode {
                return tree.clone().into_any_element();
            }
            if self.mongo_inspect {
                return div()
                    .v_flex()
                    .size_full()
                    .min_h_0()
                    .child(div().flex_1().min_h_0().child(self.grid(cx)))
                    .child(
                        div()
                            .h(px(300.))
                            .min_h(px(120.))
                            .border_t_1()
                            .border_color(cx.theme().border)
                            .v_flex()
                            .min_h_0()
                            .child(
                                Button::new("mongo-inspector-close")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Close)
                                    .tooltip(t(cx, "mongo.node.hide"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.mongo_inspect = false;
                                        cx.notify();
                                    })),
                            )
                            .child(div().flex_1().min_h_0().child(tree.clone())),
                    )
                    .into_any_element();
            }
        }
        if self.json_mode
            && let Some(document) = &self.document
        {
            document.clone().into_any_element()
        } else {
            self.grid(cx)
        }
    }
    fn sync_grid(&mut self, cx: &mut Context<Self>) {
        if let Some(document) = &self.document {
            let rows = self.display_rows();
            document.update(cx, |document, cx| document.set_rows(rows, cx));
        }
        let data = grid_ui::GridDelegate::snapshot(self, cx.entity().downgrade());
        self.grid_state.update(cx, |table, cx| {
            let columns_changed = table.delegate().column_names() != data.column_names();
            table.clear_selection(cx);
            *table.delegate_mut() = data;
            if columns_changed {
                table.refresh(cx);
            } else {
                cx.notify();
            }
        });
        cx.notify();
    }
    fn sync_selection(&mut self, cx: &mut Context<Self>) {
        self.grid_state.update(cx, |table, cx| {
            table.clear_selection(cx);
            table.delegate_mut().selection = self.selected;
            table.delegate_mut().highlight = self.highlight;
            table.delegate_mut().highlight_color = self.highlight_color;
            cx.notify();
        });
        cx.notify();
    }
    fn select_cell(&mut self, row: usize, col: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.mongo.is_some() && !self.select_mongo_node(row, col, cx) {
            return;
        }
        self.selected = Some((row, col));
        self.focus.focus(window);
        self.sync_selection(cx);
        if cx.global::<crate::feedback::InspectorSource>().0.as_deref() == Some(self.key.as_str()) {
            self.show_details(Some(row), true, cx);
        }
    }
    fn can_insert(&self) -> bool {
        self.target.mode == Mode::Grid
            && !self.notebook.as_ref().is_some_and(|n| n.stale)
            && self.target.writable
            && self.target.object.kind == "table"
            && matches!(
                self.target.kind,
                turbodbn_services::DatabaseKind::MySql
                    | turbodbn_services::DatabaseKind::Sqlite
                    | turbodbn_services::DatabaseKind::PostgreSql
            )
            && self.schema.as_ref().is_some_and(|s| {
                self.target.kind != turbodbn_services::DatabaseKind::MySql
                    || s.ddl.to_lowercase().contains("engine=innodb")
            })
    }
    fn can_edit(&self) -> bool {
        self.can_insert()
            && self.schema.as_ref().is_some_and(|s| {
                s.columns.iter().any(|c| c.primary)
                    && s.columns.iter().filter(|c| c.primary).all(|c| {
                        !["binary", "blob", "bytea", "float", "double", "real"]
                            .iter()
                            .any(|ty| c.data_type.to_lowercase().contains(ty))
                    })
            })
    }
    fn can_edit_row(&self, row: usize) -> bool {
        if row >= self.rows.values.len() {
            self.can_insert()
        } else {
            self.can_edit()
        }
    }
    fn add_row(&mut self, duplicate: bool, cx: &mut Context<Self>) {
        if self.busy || !self.can_insert() {
            crate::feedback::error(t(cx, "grid.readonly"), cx);
            return;
        }
        let mut values = BTreeMap::new();
        if duplicate
            && let Some((row, _)) = self.selected
            && let Some(schema) = &self.schema
        {
            let data = self.display_rows();
            for (col, c) in schema
                .columns
                .iter()
                .enumerate()
                .filter(|(_, c)| !c.primary)
            {
                if let Some(value) = data.values.get(row).and_then(|r| r.get(col)) {
                    values.insert(c.name.clone(), value.clone());
                }
            }
        }
        let ix = self.rows.values.len() + self.insert_count;
        self.insert_count += 1;
        self.changes.insert(
            ix,
            Change {
                original: None,
                values: Some(values),
            },
        );
        let column = self
            .schema
            .as_ref()
            .and_then(|s| s.columns.iter().position(|c| !c.primary))
            .unwrap_or(0);
        self.selected = Some((ix, column));
        self.sync_grid(cx);
        self.grid_state.update(cx, |table, cx| {
            table.scroll_to_row(ix, cx);
            table.scroll_to_col(column + 1, cx);
        });
    }
    fn edit_cell(&mut self, row: usize, col: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.mongo.is_some() {
            self.open_mongo_node(row, col, cx);
            return;
        }
        if self.busy || !self.can_edit_row(row) {
            crate::feedback::error(t(cx, "grid.readonly"), cx);
            return;
        }
        let Some(name) = self.rows.columns.get(col).cloned() else {
            return;
        };
        let value = self
            .display_rows()
            .values
            .get(row)
            .and_then(|r| r.get(col))
            .cloned()
            .flatten();
        let allow_default = row >= self.rows.values.len();
        let uses_default = allow_default
            && self
                .changes
                .get(&row)
                .and_then(|change| change.values.as_ref())
                .is_none_or(|values| !values.contains_key(&name));
        let column = self
            .schema
            .as_ref()
            .and_then(|s| s.columns.iter().find(|c| c.name == name))
            .cloned()
            .unwrap_or(workbench::Column {
                name: name.clone(),
                data_type: String::new(),
                comment: String::new(),
                primary: false,
            });
        let restore_value = self
            .rows
            .values
            .get(row)
            .and_then(|r| r.get(col))
            .cloned()
            .unwrap_or_else(|| value.clone());
        let kind = self.target.kind;
        let data_type = column.data_type.as_str();
        let editor = Rc::new(value_ui::ValueInput::new(
            data_type,
            value.clone().unwrap_or_default(),
            true,
            false,
            window,
            cx,
        ));
        let mode = Rc::new(std::cell::Cell::new(if uses_default {
            "default"
        } else if value.is_none() {
            "null"
        } else {
            "value"
        }));
        let view = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let field = editor.input.clone();
            let mode = mode.clone();
            let null_mode = mode.clone();
            let default_mode = mode.clone();
            let restore_mode = mode.clone();
            let restore_input = editor.input.clone();
            let restore_value = restore_value.clone();
            let copy_mode = mode.clone();
            let copy_input = editor.input.clone();
            let copy_column = column.clone();
            let view = view.clone();
            let name = name.clone();
            dialog
                .title(name.clone())
                .w(px(640.))
                .on_ok(|_, _, _| false)
                .child(
                    div()
                        .h_flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            Button::new("cell-null")
                                .ghost()
                                .small()
                                .icon(IconName::CircleX)
                                .label(t(cx, "grid.null"))
                                .selected(mode.get() == "null")
                                .on_click(move |_, _, cx| {
                                    null_mode.set(if null_mode.get() == "null" {
                                        "value"
                                    } else {
                                        "null"
                                    });
                                    cx.refresh_windows();
                                }),
                        )
                        .child(
                            Button::new("cell-restore")
                                .ghost()
                                .small()
                                .icon(IconName::Undo2)
                                .label(t(cx, "grid.cell.restore"))
                                .on_click(move |_, window, cx| {
                                    restore_input.update(cx, |input, cx| {
                                        input.set_value(
                                            restore_value.clone().unwrap_or_default(),
                                            window,
                                            cx,
                                        )
                                    });
                                    restore_mode.set(if uses_default {
                                        "default"
                                    } else if restore_value.is_none() {
                                        "null"
                                    } else {
                                        "value"
                                    });
                                    crate::feedback::success(t(cx, "grid.cell.restored"), cx);
                                    cx.refresh_windows();
                                }),
                        )
                        .child(
                            Button::new("cell-copy-where")
                                .ghost()
                                .small()
                                .icon(IconName::Copy)
                                .label(t(cx, "grid.cell.copy_where"))
                                .disabled(mode.get() == "default")
                                .on_click(move |_, _, cx| {
                                    let value = copy_input.read(cx).value();
                                    let value = if copy_mode.get() == "null" {
                                        None
                                    } else {
                                        Some(value.as_str())
                                    };
                                    match workbench::cell_where(kind, &copy_column, value) {
                                        Ok(sql) => {
                                            cx.write_to_clipboard(ClipboardItem::new_string(sql));
                                            crate::feedback::success(t(cx, "feedback.copied"), cx);
                                        }
                                        Err(error) => crate::feedback::error(error, cx),
                                    }
                                }),
                        )
                        .when(allow_default, |bar| {
                            bar.child(
                                Button::new("cell-default")
                                    .ghost()
                                    .small()
                                    .icon(IconName::Asterisk)
                                    .label(t(cx, "grid.default"))
                                    .selected(mode.get() == "default")
                                    .on_click(move |_, _, cx| {
                                        default_mode.set(if default_mode.get() == "default" {
                                            "value"
                                        } else {
                                            "default"
                                        });
                                        cx.refresh_windows();
                                    }),
                            )
                        }),
                )
                .child(editor.render(mode.get() != "value", cx))
                .footer(move |_, _, _, cx| {
                    let field = field.clone();
                    let mode = mode.clone();
                    let view = view.clone();
                    let name = name.clone();
                    vec![
                        crate::feedback::footer_actions()
                            .child(crate::feedback::close_button(cx))
                            .child(
                                Button::new("apply-cell")
                                    .primary()
                                    .icon(IconName::Check)
                                    .label(t(cx, "grid.apply"))
                                    .on_click(move |_, window, cx| {
                                        let value = if mode.get() == "null" {
                                            None
                                        } else {
                                            Some(field.read(cx).value().to_string())
                                        };
                                        let _ = view.update(cx, |this, cx| {
                                            let original = this.rows.values.get(row).cloned();
                                            if original.as_ref().and_then(|r| r.get(col))
                                                == Some(&value)
                                            {
                                                if let Some(values) = this
                                                    .changes
                                                    .get_mut(&row)
                                                    .and_then(|change| change.values.as_mut())
                                                {
                                                    values.remove(&name);
                                                    if values.is_empty() {
                                                        this.changes.remove(&row);
                                                    }
                                                }
                                                this.sync_grid(cx);
                                                return;
                                            }

                                            let values = this
                                                .changes
                                                .entry(row)
                                                .or_insert_with(|| Change {
                                                    original,
                                                    values: Some(BTreeMap::new()),
                                                })
                                                .values
                                                .get_or_insert_with(BTreeMap::new);
                                            if mode.get() == "default" {
                                                values.remove(&name);
                                            } else {
                                                values.insert(name.clone(), value);
                                            }
                                            this.sync_grid(cx);
                                        });
                                        window.close_dialog(cx);
                                    }),
                            ),
                    ]
                })
        });
    }
    fn can_delete_row(&self, row: usize) -> bool {
        self.can_edit_row(row)
            && (row >= self.rows.values.len()
                || self
                    .notebook
                    .as_ref()
                    .is_none_or(|n| n.access_mode.allows_note("delete")))
            && (row >= self.rows.values.len()
                || self.notebook.as_ref().is_none_or(|n| n.source.complete_row))
    }
    fn delete_row(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || !self.can_delete_row(row) {
            return;
        }
        let view = cx.entity().downgrade();
        crate::feedback::confirm(
            t(cx, "grid.delete.row"),
            t(cx, "grid.delete.body"),
            window,
            cx,
            move |_, cx| {
                let _ = view.update(cx, |this, cx| {
                    if let Some(original) = this.rows.values.get(row).cloned() {
                        this.changes.insert(
                            row,
                            Change {
                                original: Some(original),
                                values: None,
                            },
                        );
                    } else {
                        this.changes.remove(&row);
                        this.changes = std::mem::take(&mut this.changes)
                            .into_iter()
                            .map(|(i, c)| (if i > row { i - 1 } else { i }, c))
                            .collect();
                        this.insert_count = this.insert_count.saturating_sub(1);
                        this.selected = None;
                    }
                    this.sync_grid(cx);
                });
            },
        );
    }
    fn rollback(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity().downgrade();
        crate::feedback::confirm(
            t(cx, "grid.rollback"),
            t(cx, "grid.rollback.body"),
            window,
            cx,
            move |_, cx| {
                let _ = view.update(cx, |this, cx| {
                    this.changes.clear();
                    this.insert_count = 0;
                    this.selected = None;
                    this.sync_grid(cx);
                });
            },
        );
    }
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.notebook.as_ref().is_some_and(|n| n.stale) {
            crate::feedback::error(t(cx, "notes.results_stale"), cx);
            return;
        }
        let Some(schema) = self.schema.clone() else {
            return;
        };
        let changes = self.changes.values().cloned().collect::<Vec<_>>();
        let sql = match workbench::edit_preview(
            self.target.kind,
            &self.target.database,
            &self.target.object,
            &schema,
            &changes,
        ) {
            Ok(sql) => sql,
            Err(e) => {
                crate::feedback::error(e, cx);
                return;
            }
        };
        let expected_sql = sql.clone();
        let view = cx.entity().downgrade();
        let object = self.target.object.clone();
        crate::feedback::sql_preview(
            sql,
            format!("{}.{}", self.target.database, object.name),
            window,
            cx,
            move |submitted, _, cx| {
                if submitted != expected_sql {
                    crate::feedback::error(t(cx, "grid.preview.changed"), cx);
                    return false;
                }
                view.update(cx, |this, cx| {
                    if this.busy {
                        return false;
                    }
                    this.committing = true;
                    this.request(
                        Action::Commit {
                            object: object.clone(),
                            ddl: schema.ddl.clone(),
                            changes: changes.clone(),
                        },
                        cx,
                    );
                    if !this.busy {
                        this.committing = false;
                    }
                    this.busy
                })
                .unwrap_or(false)
            },
        );
    }
    fn copy(&self, row: usize, col: usize, format: &str, cx: &mut App) {
        if let Some(data) = &self.mongo {
            if let Some(doc) = data.documents.get(row) {
                let value = if format == "cell" {
                    self.rows.columns.get(col).and_then(|k| doc.get(k))
                } else {
                    Some(doc)
                };
                if let Some(value) = value {
                    cx.write_to_clipboard(ClipboardItem::new_string(
                        serde_json::to_string_pretty(value).unwrap_or_default(),
                    ));
                    crate::feedback::success(t(cx, "feedback.copied"), cx);
                }
            }
            return;
        }
        if matches!(format, "sql" | "insert" | "update")
            && self
                .notebook
                .as_ref()
                .is_some_and(|n| n.source.object.is_none())
        {
            crate::feedback::error(t(cx, "notes.result_no_table"), cx);
            return;
        }
        let data = self.display_rows();
        if matches!(format, "sql" | "insert" | "update")
            && self.schema.as_ref().is_some_and(|schema| {
                schema.columns.iter().enumerate().any(|(i, c)| {
                    ["blob", "binary", "bytea"]
                        .iter()
                        .any(|ty| c.data_type.to_lowercase().contains(ty))
                        && data
                            .values
                            .get(row)
                            .and_then(|r| r.get(i))
                            .is_some_and(|v| v.is_some())
                })
            })
        {
            crate::feedback::error(t(cx, "grid.copy.binary"), cx);
            return;
        }
        let uses_default = row >= self.rows.values.len()
            && self
                .changes
                .get(&row)
                .and_then(|change| change.values.as_ref())
                .is_some_and(|values| {
                    data.columns
                        .get(col)
                        .is_some_and(|name| !values.contains_key(name))
                });
        let result = if format == "cell" {
            Ok(data
                .values
                .get(row)
                .and_then(|r| r.get(col))
                .and_then(Clone::clone)
                .unwrap_or_else(|| {
                    if uses_default {
                        "DEFAULT".into()
                    } else {
                        "NULL".into()
                    }
                }))
        } else if matches!(format, "sql" | "insert") && row >= self.rows.values.len() {
            match (self.schema.as_ref(), self.changes.get(&row)) {
                (Some(schema), Some(change)) => workbench::edit_preview(
                    self.target.kind,
                    &self.target.database,
                    &self.target.object,
                    schema,
                    std::slice::from_ref(change),
                )
                .map(|sql| format!("{sql};")),
                _ => Err(t(cx, "grid.filter.empty").to_string()),
            }
        } else if format == "update" {
            self.schema
                .as_ref()
                .ok_or("grid.copy.update_requires_key".to_string())
                .and_then(|schema| {
                    workbench::copy_update(
                        self.target.kind,
                        &self.target.database,
                        &self.target.object,
                        schema,
                        &data,
                        &self.rows,
                        row,
                    )
                })
        } else {
            workbench::copy_row(
                self.target.kind,
                &self.target.database,
                &self.target.object,
                &data,
                row,
                format,
            )
        };
        match result {
            Ok(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                crate::feedback::success(t(cx, "feedback.copied"), cx)
            }
            Err(e) => crate::feedback::error(crate::locale::message(cx, &e), cx),
        }
    }
    fn change_query(&mut self, cx: &mut Context<Self>) {
        self.offset = 0;
        self.options.after = None;
        self.anchors.clear();
        self.total = None;
        self.load(cx);
    }
    fn show_details(&self, row: Option<usize>, refresh: bool, cx: &mut App) {
        if let (Some(row), Some(data)) = (row, self.mongo.as_ref()) {
            if let Some(doc) = data.documents.get(row) {
                crate::feedback::details(
                    crate::feedback::DetailRequest {
                        inspector: true,
                        refresh,
                        title: format!(
                            "{}.{} · #{}",
                            self.target.database,
                            self.target.object.name,
                            self.offset + row + 1
                        ),
                        entries: doc
                            .as_object()
                            .into_iter()
                            .flatten()
                            .map(|(key, v)| (key.clone(), turbodbn_core::mongo::summary(v)))
                            .collect(),
                        text: serde_json::to_string_pretty(doc).unwrap_or_default(),
                    },
                    cx,
                );
            }
            return;
        }
        if let Some(row) = row {
            if !refresh {
                cx.global_mut::<crate::feedback::InspectorSource>().0 = Some(self.key.clone());
            }
            let data = self.display_rows();
            let entries = data
                .columns
                .iter()
                .cloned()
                .zip(
                    data.values
                        .get(row)
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .map(|v| v.unwrap_or_else(|| "NULL".into())),
                )
                .collect::<Vec<_>>();
            let object = data
                .columns
                .iter()
                .cloned()
                .zip(
                    data.values
                        .get(row)
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .map(|v| {
                            v.map(serde_json::Value::String)
                                .unwrap_or(serde_json::Value::Null)
                        }),
                )
                .collect::<serde_json::Map<String, serde_json::Value>>();
            let text = serde_json::to_string_pretty(&vec![object]).unwrap_or_default();
            crate::feedback::details(
                crate::feedback::DetailRequest {
                    inspector: true,
                    refresh,
                    title: format!(
                        "{}.{} · #{}",
                        self.target.database,
                        self.target.object.name,
                        self.offset + row + 1
                    ),
                    entries,
                    text,
                },
                cx,
            );
        } else if let Some(schema) = &self.schema {
            crate::feedback::details(
                structure_details(&self.target.database, &self.target.object, schema),
                cx,
            );
        }
    }
    fn er(&self, cx: &Context<Self>) -> AnyElement {
        let Some(schema) = &self.schema else {
            return div().child(self.notice.clone()).into_any_element();
        };
        div()
            .id("er-diagram")
            .size_full()
            .overflow_scroll()
            .v_flex()
            .gap_4()
            .p_4()
            .child(
                div()
                    .w(px(340.))
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded_lg()
                    .p_3()
                    .v_flex()
                    .gap_2()
                    .child(div().text_lg().child(self.target.object.name.clone()))
                    .children(schema.columns.iter().map(|c| {
                        div()
                            .h_flex()
                            .gap_2()
                            .child(if c.primary {
                                gpui_component::Icon::default()
                                    .path("icons/key.svg")
                                    .into_any_element()
                            } else {
                                div().child("·").into_any_element()
                            })
                            .child(c.name.clone())
                            .child(
                                div()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(c.data_type.clone()),
                            )
                    })),
            )
            .children(schema.foreign_keys.values.iter().map(|r| {
                div().h_flex().gap_3().child("└──→").child(
                    div()
                        .border_1()
                        .border_color(cx.theme().border)
                        .rounded_lg()
                        .p_3()
                        .child(
                            r.iter()
                                .filter_map(|v| v.as_deref())
                                .collect::<Vec<_>>()
                                .join(" · "),
                        ),
                )
            }))
            .when(schema.foreign_keys.values.is_empty(), |el| {
                el.child(t(cx, "dbwork.er.empty"))
            })
            .into_any_element()
    }
}
impl EventEmitter<PanelEvent> for TableView {}
impl Focusable for TableView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for TableView {
    fn panel_name(&self) -> &'static str {
        "table-workspace"
    }
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::tabs::title(
            self.key.clone(),
            IconName::GalleryVerticalEnd,
            format!(
                "{} · {}",
                self.target.object.name,
                t(
                    cx,
                    match self.target.mode {
                        Mode::Grid => "dbwork.data",
                        Mode::Note => "dbwork.note",
                        Mode::Er => "dbwork.er",
                        Mode::Compare => "dbwork.compare",
                        Mode::Import => "dbwork.import",
                        Mode::Export => "dbwork.export",
                    }
                )
            ),
            cx,
        )
    }
    fn closable(&self, _: &App) -> bool {
        false
    }
    fn zoomable(&self, _: &App) -> Option<gpui_component::dock::PanelControl> {
        None
    }
}
impl Render for TableView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(change) = self.mongo_preview.take() {
            cx.defer_in(window, move |this, window, cx| {
                this.preview_mongo(change, window, cx)
            });
        }
        let default_color = default_highlight_color(cx);
        if self.highlight_color.is_none()
            && self.color_picker.read(cx).value() != Some(default_color)
        {
            self.color_picker
                .update(cx, |picker, cx| picker.set_value(default_color, window, cx));
        }
        for (input, value) in [
            (&self.page_size_input, self.options.page_size.to_string()),
            (
                &self.page_number_input,
                (self.offset / self.options.page_size + 1).to_string(),
            ),
        ] {
            if (self.sync_pager_inputs || !input.read(cx).focus_handle(cx).is_focused(window))
                && input.read(cx).value().as_str() != value
            {
                input.update(cx, |input, cx| input.set_value(value, window, cx));
            }
        }
        self.sync_pager_inputs = false;
        if self.target.mode == Mode::Grid {
            self.render_grid(cx)
        } else {
            self.render_workspace(window, cx)
        }
    }
}
impl TableView {
    fn grid_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus.is_focused(window) {
            return;
        }
        let Some((row, col)) = self.selected else {
            return;
        };
        if event.keystroke.key.eq_ignore_ascii_case("c") && event.keystroke.modifiers.control {
            self.copy(row, col, "cell", cx);
            cx.stop_propagation();
            return;
        }
        if event.keystroke.key.eq_ignore_ascii_case("f2") {
            self.edit_cell(row, col, window, cx);
            cx.stop_propagation();
            return;
        }
        let rows = self.rows.values.len() + self.insert_count;
        let cols = self.rows.columns.len();
        if rows == 0 || cols == 0 {
            return;
        }
        let next = match event.keystroke.key.as_str() {
            "left" => (row, col.saturating_sub(1)),
            "right" => (row, (col + 1).min(cols - 1)),
            "up" => (row.saturating_sub(1), col),
            "down" => ((row + 1).min(rows - 1), col),
            _ => return,
        };
        self.select_cell(next.0, next.1, window, cx);
        self.grid_state.update(cx, |table, cx| {
            if !table.visible_range().rows().contains(&next.0) {
                table.scroll_to_row(next.0, cx);
            }
            if !table.visible_range().cols().contains(&(next.1 + 1)) {
                table.scroll_to_col(next.1 + 1, cx);
            }
        });
        cx.stop_propagation();
    }
    fn render_grid(&self, cx: &Context<Self>) -> AnyElement {
        if self.target.kind == turbodbn_services::DatabaseKind::Redis {
            return div()
                .v_flex()
                .size_full()
                .min_h_0()
                .gap_2()
                .p_2()
                .track_focus(&self.focus)
                .on_key_down(cx.listener(Self::grid_key))
                .child(
                    div()
                        .h_flex()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .whitespace_normal()
                                .child(format!("KEY: {}", self.target.object.name)),
                        )
                        .child(
                            Button::new("redis-copy-key")
                                .ghost()
                                .icon(IconName::Copy)
                                .tooltip(t(cx, "redis.copy_key"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        this.target.object.name.clone(),
                                    ));
                                    crate::feedback::success(t(cx, "feedback.copied"), cx);
                                })),
                        ),
                )
                .child(
                    div()
                        .h_flex()
                        .gap_2()
                        .text_sm()
                        .child(div().flex_1().child(self.redis_kind.clone()))
                        .child(self.redis_ttl_label(cx)),
                )
                .child(div().flex_1().min_h_0().child(self.result_content(cx)))
                .child(
                    div()
                        .h_flex()
                        .gap_2()
                        .child(self.document_controls(cx))
                        .child(self.redis_actions(cx))
                        .child(
                            Button::new("redis-refresh")
                                .icon(crate::assets::refresh_icon())
                                .label(t(cx, "dbwork.refresh"))
                                .disabled(self.busy)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.offset = 0;
                                    this.redis_cursor = "0".into();
                                    this.load(cx);
                                })),
                        )
                        .child(
                            Button::new("redis-more")
                                .icon(IconName::ChevronRight)
                                .label(t(cx, "redis.next"))
                                .disabled(self.busy || !self.more)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.offset += 100;
                                    this.load(cx);
                                })),
                        )
                        .child(
                            Button::new("redis-copy")
                                .icon(IconName::Copy)
                                .label(t(cx, "dbwork.copy"))
                                .disabled(self.sql.is_empty())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        this.sql.clone(),
                                    ));
                                })),
                        ),
                )
                .into_any_element();
        }
        let mut body = div()
            .v_flex()
            .size_full()
            .min_h_0()
            .p_2()
            .gap_2()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::grid_key));
        if self.query_controls() {
            body = body.child(self.filter_bar(cx));
        }
        if let Some(note) = &self.notebook {
            body = body.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(t(
                        cx,
                        if note.stale {
                            "notes.results_stale"
                        } else if self.target.writable {
                            "notes.result_editable"
                        } else {
                            "notes.result_readonly"
                        },
                    )),
            );
        }
        body.child(div().flex_1().min_h_0().child(self.result_content(cx)))
            .child(self.data_toolbar(cx))
            .into_any_element()
    }
    fn render_workspace(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let mode = self.target.mode;
        let mut body = div()
            .v_flex()
            .size_full()
            .min_h_0()
            .p_3()
            .gap_2()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::grid_key))
            .when(mode != Mode::Grid, |body| {
                body.child(
                    div()
                        .h_flex()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(format!(
                                    "{} / {}",
                                    self.target.database, self.target.object.name
                                )),
                        )
                        .child(
                            Button::new("table-refresh")
                                .ghost()
                                .icon(crate::assets::refresh_icon())
                                .tooltip(t(cx, "dbwork.refresh"))
                                .disabled(self.busy)
                                .on_click(cx.listener(|this, _, _, cx| this.load(cx))),
                        )
                        .when(!matches!(mode, Mode::Er | Mode::Import), |el| {
                            el.child(
                                Button::new("table-export")
                                    .ghost()
                                    .icon(gpui_component::Icon::default().path("icons/export.svg"))
                                    .label(t(cx, "dbwork.export.page"))
                                    .disabled(self.busy || self.rows.columns.is_empty())
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.export(window, cx)),
                                    ),
                            )
                        }),
                )
            });
        if !matches!(mode, Mode::Er | Mode::Import) && self.query_controls() {
            body = body.child(self.filter_bar(cx));
        }
        if let Some(note) = &self.notebook {
            body = body.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(t(
                        cx,
                        if note.stale {
                            "notes.results_stale"
                        } else if self.target.writable {
                            "notes.result_editable"
                        } else {
                            "notes.result_readonly"
                        },
                    )),
            );
        }
        if mode == Mode::Note {
            body = body
                .child(div().text_sm().child(self.sql.clone()))
                .child(crate::feedback::editable_multiline(&self.note, false, cx).h(px(140.)))
                .child(
                    Button::new("save-table-note")
                        .ghost()
                        .icon(gpui_component::Icon::default().path("icons/export.svg"))
                        .label(t(cx, "dbwork.note.save"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            let text = format!(
                                "# {}\n\n```sql\n{}\n```\n\n{}",
                                this.target.object.name,
                                this.sql,
                                this.note.read(cx).value()
                            );
                            save_text(text, "table-note.md", window, cx);
                        })),
                );
        }
        if mode == Mode::Compare {
            body = body.child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(Input::new(&self.comparison).cleanable(true).flex_1())
                    .child(
                        Button::new("compare-page")
                            .label(t(cx, "dbwork.compare.page"))
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.compare(cx))),
                    ),
            );
        }
        if mode == Mode::Er {
            body = body.child(self.er(cx));
        } else if mode == Mode::Import {
            body = body.child(t(cx, "dbwork.import.notice")).child(
                Button::new("import-csv-file")
                    .icon(gpui_component::Icon::default().path("icons/import.svg"))
                    .label(t(cx, "dbwork.import.csv"))
                    .disabled(self.busy || self.schema.is_none())
                    .on_click(cx.listener(|this, _, window, cx| this.import(window, cx))),
            );
        } else {
            body = body
                .child(div().flex_1().min_h_0().child(self.result_content(cx)))
                .child(self.data_toolbar(cx));
        }
        body.when(!self.notice.is_empty() && mode != Mode::Grid, |body| {
            body.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(self.notice.clone()),
            )
        })
        .into_any_element()
    }
}

impl TableView {
    fn jump_page(&mut self, page: usize, cx: &mut Context<Self>) {
        if self.busy || !self.changes.is_empty() || self.mongo_pending() {
            return;
        }
        let Some(offset) = page
            .checked_sub(1)
            .and_then(|p| p.checked_mul(self.options.page_size))
            .filter(|offset| *offset <= 10_000_000)
        else {
            crate::feedback::error(t(cx, "grid.pager.invalid_page"), cx);
            return;
        };
        if self
            .total
            .is_some_and(|n| page as u64 > n.div_ceil(self.options.page_size as u64).max(1))
        {
            crate::feedback::error(t(cx, "grid.pager.invalid_page"), cx);
            return;
        }
        self.offset = offset;
        self.options.after = self.anchors.get(&offset).cloned().flatten();
        self.load(cx);
    }
    fn apply_pager_input(&mut self, size: bool, cx: &mut Context<Self>) {
        if self.busy || !self.changes.is_empty() || self.mongo_pending() {
            return;
        }
        let input = if size {
            &self.page_size_input
        } else {
            &self.page_number_input
        };
        let value = input.read(cx).value().trim().parse::<usize>().ok();
        if size {
            let Some(value) =
                value.filter(|value| (1..=workbench::MAX_GRID_PAGE_SIZE).contains(value))
            else {
                crate::feedback::error(t(cx, "grid.pager.invalid_size"), cx);
                return;
            };
            if value != self.options.page_size {
                self.options.page_size = value;
                self.offset = 0;
                self.options.after = None;
                self.anchors.clear();
                self.load(cx);
            }
        } else if let Some(page) = value {
            self.jump_page(page, cx);
        } else {
            crate::feedback::error(t(cx, "grid.pager.invalid_page"), cx);
        }
    }
    fn data_toolbar(&self, cx: &Context<Self>) -> AnyElement {
        div()
            .h_flex()
            .items_center()
            .gap_1()
            .h(px(32.))
            .flex_shrink_0()
            .min_w_0()
            .text_xs()
            .whitespace_nowrap()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .id("data-toolbar-controls")
                    .h_flex()
                    .items_center()
                    .flex_1()
                    .min_w_0()
                    .overflow_x_scroll()
                    .gap_1()
                    .child(self.document_controls(cx))
                    .child(self.toolbar_actions(cx))
                    .child(self.toolbar_pager(cx))
                    .child(self.toolbar_highlight(cx)),
            )
            .child(self.toolbar_statistics(cx))
            .into_any_element()
    }
    fn toolbar_actions(&self, cx: &Context<Self>) -> AnyElement {
        let editable = self.mongo.is_none()
            && self.target.mode == Mode::Grid
            && self.notebook.as_ref().is_none_or(|n| n.source.editable);
        let locked = self.busy || !self.changes.is_empty() || self.mongo_pending();
        div()
            .h_flex()
            .gap_1()
            .flex_shrink_0()
            .child(
                Button::new("grid-toolbar-refresh")
                    .ghost()
                    .small()
                    .icon(crate::assets::refresh_icon())
                    .tooltip(t(cx, "dbwork.refresh"))
                    .disabled(locked)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.total = None;
                        this.anchors.clear();
                        this.options.after = None;
                        this.load(cx);
                    })),
            )
            .when(self.notebook.is_some() && self.busy, |bar| {
                bar.child(
                    Button::new("note-grid-stop")
                        .ghost()
                        .small()
                        .icon(gpui_component::Icon::default().path("icons/stop.svg"))
                        .tooltip(t(cx, "notes.stop"))
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_result(false, cx))),
                )
            })
            .when(editable, |bar| {
                bar.child(
                    Button::new("grid-add")
                        .ghost()
                        .small()
                        .icon(IconName::Plus)
                        .tooltip(t(cx, "grid.add"))
                        .disabled(self.busy || !self.can_insert())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.add_row(false, cx);
                            this.focus.focus(window);
                        })),
                )
            })
            .when(editable, |bar| {
                bar.child(
                    Button::new("grid-duplicate")
                        .ghost()
                        .small()
                        .icon(IconName::Copy)
                        .tooltip(t(cx, "grid.duplicate"))
                        .disabled(self.busy || self.selected.is_none() || !self.can_insert())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.add_row(true, cx);
                            this.focus.focus(window);
                        })),
                )
                .child(
                    Button::new("grid-edit")
                        .ghost()
                        .small()
                        .icon(IconName::Settings2)
                        .tooltip(t(cx, "grid.edit"))
                        .disabled(
                            self.busy || self.selected.is_none_or(|(r, _)| !self.can_edit_row(r)),
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            if let Some((r, c)) = this.selected {
                                this.edit_cell(r, c, window, cx);
                            }
                        })),
                )
            })
            .when(editable, |bar| {
                bar.child(
                    Button::new("grid-submit")
                        .ghost()
                        .small()
                        .icon(IconName::Check)
                        .tooltip(t(cx, "grid.submit"))
                        .disabled(self.busy || self.changes.is_empty())
                        .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                )
                .child(
                    Button::new("grid-rollback")
                        .ghost()
                        .small()
                        .icon(IconName::Undo2)
                        .tooltip(t(cx, "grid.rollback"))
                        .disabled(self.busy || self.changes.is_empty())
                        .on_click(cx.listener(|this, _, window, cx| this.rollback(window, cx))),
                )
            })
            .child(
                Button::new("grid-export")
                    .ghost()
                    .small()
                    .icon(gpui_component::Icon::default().path("icons/export.svg"))
                    .tooltip(t(cx, "dbwork.export.page"))
                    .disabled(self.busy || self.rows.columns.is_empty())
                    .on_click(cx.listener(|this, _, window, cx| this.export(window, cx))),
            )
            .child(
                div()
                    .w(px(1.))
                    .h(px(16.))
                    .mx_1()
                    .bg(cx.theme().border)
                    .flex_shrink_0(),
            )
            .into_any_element()
    }
    fn toolbar_pager(&self, cx: &Context<Self>) -> AnyElement {
        let locked = self.busy || !self.changes.is_empty() || self.mongo_pending();
        div()
            .h_flex()
            .gap_1()
            .flex_shrink_0()
            .child(
                div()
                    .id("page-size-tip")
                    .flex_shrink_0()
                    .tooltip(|window, cx| {
                        gpui_component::tooltip::Tooltip::new(t(cx, "grid.pager.size"))
                            .build(window, cx)
                    })
                    .child(
                        Input::new(&self.page_size_input)
                            .cleanable(!locked)
                            .small()
                            .w(px(64.))
                            .disabled(locked),
                    ),
            )
            .child(
                Button::new("page-first")
                    .ghost()
                    .small()
                    .icon(gpui_component::Icon::default().path("icons/page-first.svg"))
                    .tooltip(t(cx, "grid.pager.first"))
                    .disabled(locked || self.offset == 0)
                    .on_click(cx.listener(|this, _, _, cx| this.jump_page(1, cx))),
            )
            .child(
                Button::new("page-previous")
                    .ghost()
                    .small()
                    .icon(IconName::ChevronLeft)
                    .tooltip(t(cx, "grid.toolbar.previous"))
                    .disabled(locked || self.offset == 0)
                    .on_click(cx.listener(|this, _, _, cx| {
                        let previous = this.offset / this.options.page_size;
                        let page = this
                            .total
                            .map(|n| {
                                (n.div_ceil(this.options.page_size as u64).max(1))
                                    .min(previous as u64) as usize
                            })
                            .unwrap_or(previous);
                        this.jump_page(page, cx)
                    })),
            )
            .child(
                div()
                    .id("page-number-tip")
                    .flex_shrink_0()
                    .tooltip(|window, cx| {
                        gpui_component::tooltip::Tooltip::new(t(cx, "grid.pager.jump"))
                            .build(window, cx)
                    })
                    .child(
                        Input::new(&self.page_number_input)
                            .cleanable(!locked)
                            .small()
                            .w(px(64.))
                            .disabled(locked),
                    ),
            )
            .child(
                Button::new("page-next")
                    .ghost()
                    .small()
                    .icon(IconName::ChevronRight)
                    .tooltip(t(cx, "grid.toolbar.next"))
                    .disabled(locked || !self.more)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.jump_page(this.offset / this.options.page_size + 2, cx)
                    })),
            )
            .child(
                Button::new("page-last")
                    .ghost()
                    .small()
                    .icon(gpui_component::Icon::default().path("icons/page-last.svg"))
                    .tooltip(t(
                        cx,
                        if self.total.is_none() {
                            "grid.pager.last_unknown"
                        } else {
                            "grid.pager.last"
                        },
                    ))
                    .disabled(
                        locked
                            || self.total.is_none()
                            || self.total.is_some_and(|n| {
                                self.offset as u64 / self.options.page_size as u64 + 1
                                    == n.div_ceil(self.options.page_size as u64).max(1)
                            }),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(n) = this.total
                            && let Ok(page) =
                                usize::try_from(n.div_ceil(this.options.page_size as u64).max(1))
                        {
                            this.jump_page(page, cx);
                        }
                    })),
            )
            .into_any_element()
    }
    fn toolbar_highlight(&self, cx: &Context<Self>) -> AnyElement {
        div()
            .h_flex()
            .gap_1()
            .flex_shrink_0()
            .child(
                div()
                    .id("grid-highlight-controls")
                    .h_flex()
                    .gap_0()
                    .child(
                        Button::new("grid-highlight")
                            .small()
                            .ghost()
                            .selected(self.highlight)
                            .icon(gpui_component::Icon::default().path("icons/focus-cross.svg"))
                            .tooltip(t(cx, "grid.highlight"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.highlight = !this.highlight;
                                this.sync_selection(cx);
                            })),
                    )
                    .child(
                        div()
                            .id("grid-highlight-color")
                            .tooltip(|window, cx| {
                                gpui_component::tooltip::Tooltip::new(t(cx, "grid.highlight.color"))
                                    .build(window, cx)
                            })
                            .child(
                                ColorPicker::new(&self.color_picker)
                                    .small()
                                    .icon(IconName::ChevronDown)
                                    .anchor(Corner::BottomLeft)
                                    .featured_colors(
                                        [
                                            0xeab308, 0x22c55e, 0x06b6d4, 0x3b82f6, 0xa855f7,
                                            0xf43f5e,
                                        ]
                                        .into_iter()
                                        .map(|v| rgb(v).into())
                                        .collect(),
                                    ),
                            ),
                    )
                    .when(self.highlight_color.is_some(), |el| {
                        el.child(
                            Button::new("reset-highlight-color")
                                .small()
                                .ghost()
                                .icon(IconName::Undo2)
                                .tooltip(t(cx, "grid.highlight.default"))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.highlight_color = None;
                                    this.color_picker.update(cx, |picker, cx| {
                                        picker.set_value(default_highlight_color(cx), window, cx)
                                    });
                                    this.sync_selection(cx);
                                })),
                        )
                    }),
            )
            .when(!self.changes.is_empty(), |bar| {
                bar.child(div().flex_shrink_0().text_color(cx.theme().warning).child(
                    crate::locale::tr(
                        cx,
                        "grid.toolbar.pending",
                        &[("count", &self.changes.len().to_string())],
                    ),
                ))
            })
            .into_any_element()
    }
    fn toolbar_statistics(&self, cx: &Context<Self>) -> AnyElement {
        let details = format!(
            "{} / {}\n{}\n{}",
            self.target.database,
            self.target.object.name,
            crate::locale::tr(
                cx,
                "grid.status",
                &[
                    ("rows", &self.rows.values.len().to_string()),
                    ("ms", &crate::duration::elapsed(self.elapsed_ms)),
                    (
                        "cell",
                        &self
                            .selected
                            .map(|(r, c)| format!("{}, {}", self.offset + r + 1, c + 1))
                            .unwrap_or_else(|| "—".into())
                    ),
                    ("pending", &self.changes.len().to_string())
                ]
            ),
            t(
                cx,
                if self.keyset {
                    "grid.paging.cursor"
                } else {
                    "grid.paging.offset"
                }
            )
        );
        div()
            .h_flex()
            .items_center()
            .gap_1()
            .flex_shrink_0()
            .child(
                Button::new("grid-count")
                    .ghost()
                    .small()
                    .icon(gpui_component::Icon::default().path("icons/row-count.svg"))
                    .tooltip(t(cx, "grid.count"))
                    .disabled(self.busy)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.request(
                            Action::Count {
                                object: this.target.object.clone(),
                                filter: this.options.filter.clone(),
                            },
                            cx,
                        )
                    })),
            )
            .when(self.busy, |el| {
                el.child(
                    div()
                        .h_flex()
                        .gap_1()
                        .child(gpui_component::spinner::Spinner::new().small())
                        .child(t(
                            cx,
                            if self.counting {
                                "grid.count.loading"
                            } else {
                                "dbwork.loading"
                            },
                        )),
                )
            })
            .child({
                let tip = crate::locale::tr(
                    cx,
                    "grid.toolbar.rows",
                    &[
                        ("rows", &self.rows.values.len().to_string()),
                        (
                            "total",
                            &self
                                .total
                                .map(|n| n.to_string())
                                .unwrap_or_else(|| t(cx, "grid.pager.unknown").to_string()),
                        ),
                    ],
                );
                div()
                    .id("grid-row-counts")
                    .flex_shrink_0()
                    .child(format!(
                        "{} / {}",
                        self.rows.values.len(),
                        self.total
                            .map(|n| n.to_string())
                            .unwrap_or_else(|| "—".into())
                    ))
                    .tooltip(move |window, cx| {
                        gpui_component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                    })
            })
            .child(
                div()
                    .id("grid-toolbar-details")
                    .flex_shrink_0()
                    .text_color(cx.theme().muted_foreground)
                    .child(crate::duration::elapsed(self.elapsed_ms))
                    .tooltip(move |window, cx| {
                        gpui_component::tooltip::Tooltip::new(details.clone()).build(window, cx)
                    }),
            )
            .into_any_element()
    }
}

pub(super) fn default_highlight_color(cx: &App) -> Hsla {
    rgb(if cx.theme().mode.is_dark() {
        0x7395b8
    } else {
        0xeab308
    })
    .into()
}

pub(crate) fn structure_details(
    database: &str,
    object: &workbench::Object,
    schema: &Schema,
) -> crate::feedback::DetailRequest {
    crate::feedback::DetailRequest {
        inspector: false,
        refresh: false,
        title: if object.schema.is_empty() {
            format!("{database}.{}", object.name)
        } else {
            format!("{database}.{}.{}", object.schema, object.name)
        },
        entries: schema
            .columns
            .iter()
            .map(|c| {
                let mut value = c.data_type.clone();
                if c.primary {
                    value.push_str(" · PRIMARY KEY");
                }
                if !c.comment.is_empty() {
                    value.push_str(" · ");
                    value.push_str(&c.comment);
                }
                (c.name.clone(), value)
            })
            .collect(),
        text: schema.ddl.clone(),
    }
}
/// Write only after the native save picker. IO stays off the rendering thread.
pub fn save_text<T: 'static>(text: String, name: &str, window: &mut Window, cx: &mut Context<T>) {
    let directory = dirs::document_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    let pick = cx.prompt_for_new_path(&directory, Some(name));
    cx.spawn_in(window, async move |_, cx| {
        if let Ok(Ok(Some(path))) = pick.await {
            let result = cx
                .background_executor()
                .spawn(async move { std::fs::write(path, text).map_err(|e| e.to_string()) })
                .await;
            let _ = cx.update(|_, cx| match result {
                Ok(()) => crate::feedback::success(t(cx, "feedback.file.saved"), cx),
                Err(error) => crate::feedback::error(error, cx),
            });
        }
    })
    .detach();
}

//! Bounded runtime reports; never serialized into Notes or logged as SQL.
use crate::{
    database::{DatabaseView, SavedConnection},
    feedback,
    locale::{t, tr},
    query_grid::QueryGrid,
};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Disableable, Selectable, StyledExt,
    button::{Button, ButtonVariants},
    menu::{DropdownMenu, PopupMenuItem},
    table::{Table, TableState},
};
use std::{collections::VecDeque, sync::Arc, time::Instant};
use turbodbn_services::database::{
    notebook_query::ResultPage,
    transfer::{self, Control},
    workbench::{Action, Object, Reply, Request, Rows},
};
fn object_label(object: &Object) -> String {
    if object.schema.is_empty() {
        object.name.clone()
    } else {
        format!("{}.{}", object.schema, object.name)
    }
}
const MAX_DETAILS: usize = 500;
const MAX_TABLES: usize = 128;
#[derive(Default)]
struct Count {
    known: u64,
    unknown: u64,
}
impl Count {
    fn add(&mut self, value: Option<u64>) {
        if let Some(n) = value {
            self.known = self.known.saturating_add(n);
        } else {
            self.unknown += 1;
        }
    }
    fn text(&self, cx: &App) -> SharedString {
        if self.unknown == 0 {
            self.known.to_string().into()
        } else {
            tr(
                cx,
                "script.report.count_unknown",
                &[
                    ("known", &self.known.to_string()),
                    ("unknown", &self.unknown.to_string()),
                ],
            )
        }
    }
}
#[derive(Default)]
struct Stats {
    created: Count,
    skipped: Count,
    inserted: Count,
    updated: Count,
    deleted: Count,
    mixed: Count,
    ddl: u64,
    session: u64,
    reads: u64,
}
impl Stats {
    fn add(&mut self, result: &ResultPage) {
        if let Some(summary) = &result.execution {
            if summary.table_create {
                self.created.add(summary.created_tables);
                self.skipped.add(summary.skipped_tables);
            }
            match summary.operation.as_str() {
                "INSERT" if summary.mixed_write => self.mixed.add(summary.affected_rows),
                "INSERT" => self.inserted.add(summary.affected_rows),
                "UPDATE" => self.updated.add(summary.affected_rows),
                "DELETE" => self.deleted.add(summary.affected_rows),
                "SESSION" => self.session += 1,
                _ => self.ddl += 1,
            }
        } else if result.read_only {
            self.reads += 1;
        } else {
            self.session += 1;
        }
    }
}
struct Detail {
    number: u64,
    line: u64,
    operation: String,
    target: String,
    rows: Option<u64>,
    elapsed: Option<u64>,
    status: &'static str,
}
pub struct ScriptResults {
    connections: Entity<DatabaseView>,
    binding: Option<(SavedConnection, String)>,
    stats: Stats,
    details: VecDeque<Detail>,
    objects: Vec<Object>,
    grid: Entity<TableState<QueryGrid>>,
    query: Rows,
    query_label: SharedString,
    schema: Rows,
    schema_label: SharedString,
    selected: Option<Object>,
    tab: usize,
    success: u64,
    failed: u64,
    total: u64,
    shown: bool,
    running: bool,
    elapsed: u64,
    revision: u64,
    pending: Option<(u64, String, String, Instant)>,
    metadata_task: Option<Task<()>>,
    control: Option<Arc<Control>>,
    loading: bool,
    schema_notice: SharedString,
    metadata_job: Option<(uuid::Uuid, Instant)>,
}
impl ScriptResults {
    pub fn new(
        connections: Entity<DatabaseView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.on_release(|this, cx| this.cancel_metadata(cx)).detach();
        Self {
            connections,
            binding: None,
            stats: Stats::default(),
            details: VecDeque::new(),
            objects: vec![],
            grid: cx.new(|cx| {
                TableState::new(QueryGrid::new(Rows::default(), 0), window, cx)
                    .row_selectable(false)
                    .col_selectable(false)
            }),
            query: Rows::default(),
            query_label: "".into(),
            schema: Rows::default(),
            schema_label: "".into(),
            selected: None,
            tab: 0,
            success: 0,
            failed: 0,
            total: 0,
            shown: false,
            running: false,
            elapsed: 0,
            revision: 0,
            pending: None,
            metadata_task: None,
            control: None,
            loading: false,
            schema_notice: "".into(),
            metadata_job: None,
        }
    }
    fn cancel_metadata(&mut self, cx: &mut App) {
        if let Some((job, started)) = self.metadata_job.take() {
            self.schema_log(
                job,
                crate::operation_log::Phase::Cancelled,
                started.elapsed().as_millis() as u64,
                None,
                cx,
            );
        }
        if let Some(control) = self.control.take() {
            control.cancel();
        }
        self.metadata_task = None;
        self.loading = false;
        self.revision += 1;
    }
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.cancel_metadata(cx);
        self.binding = None;
        self.shown = false;
        self.query = Rows::default();
        self.schema = Rows::default();
        self.details.clear();
        self.pending = None;
        self.objects.clear();
        cx.notify();
    }
    pub fn reset(
        &mut self,
        saved: SavedConnection,
        database: String,
        total: u64,
        cx: &mut Context<Self>,
    ) {
        self.clear(cx);
        self.binding = Some((saved, database));
        self.stats = Stats::default();
        self.success = 0;
        self.failed = 0;
        self.total = total;
        self.tab = 0;
        self.elapsed = 0;
        self.running = true;
        self.selected = None;
        self.schema_label = "".into();
        self.query_label = "".into();
        self.sync_grid(cx);
    }
    pub fn register_targets(&mut self, objects: impl IntoIterator<Item = Object>) {
        for object in objects {
            if self.objects.len() < MAX_TABLES && !self.objects.contains(&object) {
                self.objects.push(object);
            }
        }
    }
    pub fn begin_statement(
        &mut self,
        number: u64,
        plan: &turbodbn_services::database::notebook_query::Plan,
    ) {
        self.register_targets(plan.guards.iter().take(32).map(|g| g.object.clone()));
        let operation = statement_operation(plan);
        let target = plan
            .guards
            .iter()
            .take(32)
            .map(|g| object_label(&g.object))
            .collect::<Vec<_>>()
            .join(", ");
        self.pending = Some((
            number,
            operation,
            target.chars().take(512).collect(),
            Instant::now(),
        ));
    }
    pub fn push(
        &mut self,
        result: ResultPage,
        number: u64,
        line: u64,
        refresh: bool,
        cx: &mut Context<Self>,
    ) {
        self.pending = None;
        self.shown = true;
        self.success += 1;
        self.elapsed = self.elapsed.saturating_add(result.elapsed_ms);
        self.stats.add(&result);
        let summary = result.execution.as_deref();
        if let Some(summary) = summary {
            self.register_targets(summary.objects.iter().cloned());
        }
        let operation = summary
            .map(|s| {
                if s.mixed_write {
                    "UPSERT"
                } else {
                    s.operation.as_str()
                }
            })
            .unwrap_or(if result.read_only { "QUERY" } else { "SESSION" })
            .to_string();
        let target = summary
            .map(|s| {
                s.objects
                    .iter()
                    .map(object_label)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        self.details.push_back(Detail {
            number,
            line,
            operation,
            target: target.chars().take(512).collect(),
            rows: result.affected_rows,
            elapsed: Some(result.elapsed_ms),
            status: "script.report.success",
        });
        if self.details.len() > MAX_DETAILS {
            self.details.pop_front();
        }
        // DDL/session results never replace the last genuine rowset with an empty grid.
        if !result.rows.columns.is_empty() || result.read_only && result.execution.is_none() {
            self.query_label = tr(
                cx,
                if result.more {
                    "script.report.query_truncated"
                } else {
                    "script.report.query"
                },
                &[
                    ("number", &(number + 1).to_string()),
                    ("rows", &result.rows.values.len().to_string()),
                ],
            );
            self.query = result.rows;
        }
        if refresh {
            self.sync_grid(cx);
        }
        cx.notify();
    }
    pub fn fail(&mut self, number: u64, line: u64, stage: &'static str, cx: &mut Context<Self>) {
        self.shown = true;
        self.failed = 1;
        if let Some(last) = self.details.back_mut().filter(|d| d.number == number) {
            last.status = stage;
            self.success = self.success.saturating_sub(1);
        } else {
            let pending = self.pending.take().filter(|(n, _, _, _)| *n == number);
            let (operation, target, elapsed) =
                if let Some((_, operation, target, started)) = pending {
                    let elapsed = started.elapsed().as_millis() as u64;
                    self.elapsed = self.elapsed.saturating_add(elapsed);
                    (operation, target, Some(elapsed))
                } else {
                    ("—".into(), String::new(), None)
                };
            self.details.push_back(Detail {
                number,
                line,
                operation,
                target,
                rows: None,
                elapsed,
                status: stage,
            });
            if self.details.len() > MAX_DETAILS {
                self.details.pop_front();
            }
        }
        self.sync_grid(cx);
    }
    pub fn finish(&mut self, cx: &mut Context<Self>) {
        self.running = false;
        self.sync_grid(cx);
        cx.notify();
    }
    fn sync_grid(&mut self, cx: &mut Context<Self>) {
        let rows = match self.tab {
            1 => self.query.clone(),
            2 => self.schema.clone(),
            _ => Rows {
                columns: [
                    "script.report.statement",
                    "script.report.operation",
                    "script.report.status",
                    "script.report.affected",
                    "script.report.target",
                    "script.report.elapsed",
                ]
                .map(|k| t(cx, k).to_string())
                .to_vec(),
                values: self
                    .details
                    .iter()
                    .map(|d| {
                        vec![
                            Some(format!("{} · L{}", d.number + 1, d.line)),
                            Some(d.operation.clone()),
                            Some(t(cx, d.status).to_string()),
                            Some(d.rows.map(|n| n.to_string()).unwrap_or_else(|| "—".into())),
                            Some(d.target.clone()),
                            Some(
                                d.elapsed
                                    .map(crate::duration::elapsed)
                                    .unwrap_or_else(|| "—".into()),
                            ),
                        ]
                    })
                    .collect(),
            },
        };
        let widths: &[f32] = match self.tab {
            0 => &[88., 82., 90., 86., 220., 88.],
            2 => &[180., 160., 80., 260.],
            _ => &[],
        };
        self.grid.update(cx, |grid, cx| {
            *grid.delegate_mut() = QueryGrid::new(rows, 0).with_column_widths(widths);
            grid.refresh(cx);
        });
    }
    fn schema_log(
        &self,
        job: uuid::Uuid,
        phase: crate::operation_log::Phase,
        elapsed: u64,
        error: Option<&str>,
        cx: &mut App,
    ) {
        if let Some(output) = self.connections.read(cx).output.clone() {
            let mut record =
                crate::operation_log::Record::new(job, crate::operation_log::Kind::Metadata, phase);
            record.source = self.binding.as_ref().map(|(saved, _)| saved.profile.id);
            record.elapsed_ms = elapsed;
            if let Some(error) = error {
                record.failure = crate::operation_log::failure_class(error);
            }
            output.update(cx, |output, cx| output.operation(record, cx));
        }
    }
    fn load_schema(&mut self, object: Object, cx: &mut Context<Self>) {
        if self.loading && self.selected.as_ref() == Some(&object) {
            return;
        }
        if self.running {
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
        self.cancel_metadata(cx);
        self.tab = 2;
        self.selected = Some(object.clone());
        self.schema = Rows::default();
        self.schema_notice = "".into();
        self.loading = true;
        self.schema_label = object_label(&object).into();
        self.sync_grid(cx);
        let revision = self.revision;
        let control = Arc::new(Control::default());
        self.control = Some(control.clone());
        let job = uuid::Uuid::new_v4();
        let started = Instant::now();
        self.metadata_job = Some((job, started));
        self.schema_log(job, crate::operation_log::Phase::Started, 0, None, cx);
        let work = cx.background_executor().spawn(async move {
            let endpoint = saved.endpoint(database.clone())?;
            let result = transfer::service_operation(
                &endpoint,
                turbodbn_services::database::OperationKind::Workbench(Box::new(Request {
                    database,
                    action: Action::NoteColumns(object),
                })),
                &control,
            )?;
            match result {
                turbodbn_services::database::OperationResult::Workbench(reply) => match *reply {
                    Reply::Schema(schema) => Ok(schema),
                    _ => Err("script.error.execution".into()),
                },
                _ => Err("script.error.execution".into()),
            }
        });
        self.metadata_task = Some(cx.spawn(async move |owner, cx| {
            let result: Result<turbodbn_services::database::workbench::Schema, String> = work.await;
            let _ = owner.update(cx, |this, cx| {
                if revision != this.revision {
                    return;
                }
                this.metadata_job = None;
                this.loading = false;
                this.control = None;
                if !this.binding.as_ref().is_some_and(|(saved, _)| {
                    this.connections.read(cx).connection_snapshot_current(saved)
                }) {
                    this.schema_log(
                        job,
                        crate::operation_log::Phase::Cancelled,
                        started.elapsed().as_millis() as u64,
                        None,
                        cx,
                    );
                    feedback::error(t(cx, "notes.preview_changed"), cx);
                    return;
                }
                match result {
                    Ok(schema) => {
                        this.schema_notice = if schema.columns.len() > 512 {
                            t(cx, "script.report.schema_limit")
                        } else if schema.columns.is_empty() {
                            t(cx, "script.report.schema_empty")
                        } else {
                            "".into()
                        };
                        this.schema = Rows {
                            columns: [
                                "script.report.field",
                                "script.report.type",
                                "script.report.primary",
                                "script.report.comment",
                            ]
                            .map(|k| t(cx, k).to_string())
                            .to_vec(),
                            values: schema
                                .columns
                                .into_iter()
                                .take(512)
                                .map(|c| {
                                    vec![
                                        Some(c.name),
                                        Some(c.data_type),
                                        Some(if c.primary { "PK".into() } else { "".into() }),
                                        Some(c.comment),
                                    ]
                                })
                                .collect(),
                        };
                        this.schema_log(
                            job,
                            crate::operation_log::Phase::Succeeded,
                            started.elapsed().as_millis() as u64,
                            None,
                            cx,
                        );
                        this.sync_grid(cx);
                    }
                    Err(error) => {
                        this.schema_notice = crate::locale::message(cx, &error);
                        this.schema_log(
                            job,
                            crate::operation_log::Phase::Failed,
                            started.elapsed().as_millis() as u64,
                            Some(&error),
                            cx,
                        );
                        feedback::error(crate::locale::message(cx, &error), cx);
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
}
impl Drop for ScriptResults {
    fn drop(&mut self) {
        if let Some(control) = &self.control {
            control.cancel();
        }
    }
}
impl Render for ScriptResults {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.shown {
            return div().into_any_element();
        }
        let summary = tr(
            cx,
            "script.report.summary",
            &[
                ("success", &self.success.to_string()),
                ("failed", &self.failed.to_string()),
                (
                    "pending",
                    &self
                        .total
                        .saturating_sub(self.success + self.failed)
                        .to_string(),
                ),
                ("created", &self.stats.created.text(cx)),
                ("skipped", &self.stats.skipped.text(cx)),
                ("inserted", &self.stats.inserted.text(cx)),
                ("updated", &self.stats.updated.text(cx)),
                ("deleted", &self.stats.deleted.text(cx)),
            ],
        );
        let mut body = div().v_flex().gap_2().min_w_0().child(summary).child(
            div()
                .h_flex()
                .gap_2()
                .child(
                    Button::new("script-report-details")
                        .ghost()
                        .icon(gpui_component::Icon::default().path("icons/md-list.svg"))
                        .label(t(cx, "script.report.details"))
                        .selected(self.tab == 0)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.tab = 0;
                            this.sync_grid(cx);
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("script-report-query")
                        .ghost()
                        .icon(gpui_component::Icon::default().path("icons/note-sql.svg"))
                        .label(t(cx, "script.report.query_tab"))
                        .selected(self.tab == 1)
                        .disabled(self.query_label.is_empty())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.tab = 1;
                            this.sync_grid(cx);
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("script-report-schema")
                        .ghost()
                        .icon(gpui_component::Icon::default().path("icons/note-ddl.svg"))
                        .max_w(px(220.))
                        .label(if self.schema_label.is_empty() {
                            t(cx, "script.report.schema_tab")
                        } else {
                            self.schema_label.clone()
                        })
                        .disabled(self.running || self.objects.is_empty())
                        .selected(self.tab == 2)
                        .dropdown_menu({
                            let objects = self.objects.clone();
                            let weak = cx.entity().downgrade();
                            move |mut menu, _, _| {
                                for object in &objects {
                                    let object = object.clone();
                                    let weak = weak.clone();
                                    menu =
                                        menu.item(
                                            PopupMenuItem::new(format!(
                                                "{}.{}",
                                                object.schema, object.name
                                            ))
                                            .on_click(move |_, _, cx| {
                                                let _ = weak.update(cx, |this, cx| {
                                                    this.load_schema(object.clone(), cx)
                                                });
                                            }),
                                        );
                                }
                                menu.scrollable(true).max_h(px(280.))
                            }
                        }),
                ),
        );
        if self.tab == 1 {
            body = body.child(self.query_label.clone());
        }
        if self.tab == 2 && !self.schema_notice.is_empty() {
            body = body.child(self.schema_notice.clone());
        }
        if self.loading {
            body = body.child(t(cx, "script.report.loading"));
        }
        let empty = match self.tab {
            1 => self.query.columns.is_empty(),
            2 => self.schema.columns.is_empty(),
            _ => self.details.is_empty(),
        };
        if !empty {
            let state = self.grid.read(cx);
            let vertical = state.vertical_scroll_handle.clone();
            let horizontal = state.horizontal_scroll_handle.clone();
            body = body.child(
                div()
                    .relative()
                    .h(px(220.))
                    .child(Table::new(&self.grid).scrollbar_visible(false, false))
                    .child(
                        div()
                            .absolute()
                            .occlude()
                            .right_0()
                            .top(px(32.))
                            .bottom_0()
                            .w(px(14.))
                            .child(
                                gpui_component::scroll::Scrollbar::vertical(&vertical)
                                    .scrollbar_show(gpui_component::scroll::ScrollbarShow::Always),
                            ),
                    )
                    .child(
                        div()
                            .absolute()
                            .occlude()
                            .left(px(52.))
                            .right_0()
                            .bottom_0()
                            .h(px(14.))
                            .child(
                                gpui_component::scroll::Scrollbar::horizontal(&horizontal)
                                    .scrollbar_show(gpui_component::scroll::ScrollbarShow::Always),
                            ),
                    ),
            );
        }
        body.child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(tr(
                    cx,
                    "script.report.hint",
                    &[
                        ("ddl", &self.stats.ddl.to_string()),
                        ("session", &self.stats.session.to_string()),
                        ("reads", &self.stats.reads.to_string()),
                        ("mixed", &self.stats.mixed.text(cx)),
                        ("elapsed", &crate::duration::elapsed(self.elapsed)),
                    ],
                )),
        )
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
            if event.keystroke.modifiers.control
                && event.keystroke.key.eq_ignore_ascii_case("c")
                && this
                    .grid
                    .read(cx)
                    .focus_handle(cx)
                    .contains_focused(window, cx)
                && let Some(value) = this.grid.read(cx).delegate().selected_value()
            {
                cx.write_to_clipboard(ClipboardItem::new_string(value));
                cx.stop_propagation();
            }
        }))
        .into_any_element()
    }
}
fn statement_operation(plan: &turbodbn_services::database::notebook_query::Plan) -> String {
    turbodbn_services::database::notebook_query::script_operation_label(&plan.sql).into()
}
#[cfg(test)]
mod tests {
    use super::Stats;
    use turbodbn_services::database::{
        notebook_query::{ExecutionSummary, ResultPage},
        workbench::Rows,
    };
    #[test]
    fn unknown_counts_and_mixed_writes_are_not_reported_as_inserts() {
        let mut stats = Stats::default();
        let mut result = ResultPage {
            execution: Some(Box::new(ExecutionSummary {
                operation: "INSERT".into(),
                affected_rows: Some(4),
                mixed_write: true,
                ..Default::default()
            })),
            affected_rows: Some(4),
            grid: None,
            rows: Rows::default(),
            more: false,
            offset: 0,
            elapsed_ms: 0,
            read_only: false,
        };
        stats.add(&result);
        assert_eq!(stats.inserted.known, 0);
        assert_eq!(stats.mixed.known, 4);
        result.execution.as_mut().unwrap().mixed_write = false;
        result.execution.as_mut().unwrap().affected_rows = None;
        stats.add(&result);
        assert_eq!(stats.inserted.unknown, 1);
        result.execution = Some(Box::new(ExecutionSummary {
            operation: "CREATE".into(),
            table_create: true,
            created_tables: Some(0),
            skipped_tables: Some(1),
            ..Default::default()
        }));
        stats.add(&result);
        assert_eq!(stats.created.known, 0);
        assert_eq!(stats.skipped.known, 1);
    }
}

use super::*;
use crate::database::SavedConnection;
use turbodbn_services::{
    database::{
        Operation, OperationKind, OperationResult,
        notebook_query::{GridAction, GridSource, Plan, ResultPage},
    },
    mysql::{Event, Session},
};
use uuid::Uuid;

pub(super) struct NotebookContext {
    saved: SavedConnection,
    pub access_mode: turbodbn_services::access_mode::AccessMode,
    plan: Plan,
    confirmed: bool,
    pub source: GridSource,
    pub stale: bool,
    pub page_only: bool,
    pub compact: bool,
    show_details: bool,
    ticket: Option<Uuid>,
    started: std::time::Instant,
    session: Option<Session>,
    task: Option<Task<()>>,
}
impl EventEmitter<crate::note_permissions::Advice> for TableView {}
impl TableView {
    pub(super) fn render_note_status(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let note = self.notebook.as_ref()?;
        if note.compact {
            return None;
        }
        let hint = t(
            cx,
            if note.page_only {
                "notes.multi.session_result"
            } else if note.stale {
                "notes.results_stale"
            } else if self.target.writable {
                "notes.result_editable"
            } else {
                "notes.result_readonly"
            },
        );
        let status = if note.stale || note.page_only {
            hint.clone()
        } else {
            crate::locale::tr(
                cx,
                "notes.results.complete",
                &[
                    ("count", "1"),
                    ("elapsed", &crate::duration::elapsed(self.elapsed_ms)),
                ],
            )
        };
        Some(
            div()
                .v_flex()
                .gap_1()
                .min_w_0()
                .child(
                    div()
                        .h_flex()
                        .items_center()
                        .gap_2()
                        .min_w_0()
                        .text_sm()
                        .child(gpui_component::Icon::new(if note.stale {
                            IconName::Info
                        } else {
                            IconName::Check
                        }))
                        .child(div().flex_1().min_w_0().truncate().child(status))
                        .child(
                            Button::new("note-single-result-info")
                                .ghost()
                                .small()
                                .icon(
                                    gpui_component::Icon::default()
                                        .path("icons/ai-permissions.svg"),
                                )
                                .tooltip(hint),
                        )
                        .child(
                            Button::new("note-single-result-details")
                                .ghost()
                                .small()
                                .icon(if note.show_details {
                                    IconName::ChevronUp
                                } else {
                                    IconName::ChevronDown
                                })
                                .label(t(cx, "notes.results.details"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(note) = &mut this.notebook {
                                        note.show_details = !note.show_details;
                                    }
                                    cx.notify();
                                })),
                        ),
                )
                .when(note.show_details, |body| {
                    body.child(
                        div()
                            .id("note-single-frozen-sql")
                            .max_h(px(80.))
                            .overflow_y_scroll()
                            .text_xs()
                            .child(note.plan.sql.clone()),
                    )
                })
                .into_any_element(),
        )
    }
    pub(crate) fn set_note_access_mode(
        &mut self,
        mode: turbodbn_services::access_mode::AccessMode,
        cx: &mut Context<Self>,
    ) {
        let Some(note) = &mut self.notebook else {
            return;
        };
        note.access_mode = mode;
        self.target.writable = mode != turbodbn_services::access_mode::AccessMode::ReadOnly
            && note.source.editable
            && note.saved.profile.supports_mutation()
            && !(note.saved.profile.kind == turbodbn_services::DatabaseKind::MySql
                && turbodbn_services::mysql::uses_legacy(&note.saved.profile));
        cx.notify();
    }
    pub(crate) fn from_note(
        source: WeakEntity<DatabaseView>,
        binding: (SavedConnection, String),
        execution: (Plan, bool, ResultPage),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (saved, database) = binding;
        let (plan, confirmed, result) = execution;
        let meta = result.grid.unwrap_or(GridSource {
            object: None,
            schema: Schema::default(),
            editable: false,
            complete_row: false,
        });
        let object = meta.object.clone().unwrap_or(workbench::Object {
            schema: String::new(),
            name: "SQL".into(),
            kind: "query".into(),
            comment: String::new(),
            parent: String::new(),
        });
        let target = OpenTable {
            connection: saved.profile.id,
            database,
            object,
            mode: Mode::Grid,
            generation: saved.generation,
            kind: saved.profile.kind,
            writable: false,
            mysql_raw_gbk: saved.profile.mysql_raw_gbk,
        };
        let mut this = Self::create(
            format!("note-result-{}", Uuid::new_v4()),
            source,
            target,
            false,
            window,
            cx,
        );
        this.options.page_size = 20;
        this.notebook = Some(NotebookContext {
            saved,
            access_mode: Default::default(),
            plan: plan.clone(),
            confirmed,
            source: meta.clone(),
            stale: false,
            page_only: false,
            compact: false,
            show_details: false,
            ticket: None,
            started: std::time::Instant::now(),
            session: None,
            task: None,
        });
        this.receive(
            Ok(Reply::GridPage {
                rows: result.rows,
                schema: meta.schema,
                offset: result.offset,
                more: result.more,
                sql: plan.sql,
                next: None,
                elapsed_ms: result.elapsed_ms,
                keyset: false,
            }),
            cx,
        );
        this
    }
    pub(crate) fn result_pending(&self) -> bool {
        !self.changes.is_empty()
    }
    pub(crate) fn result_busy(&self) -> bool {
        self.busy
    }
    pub(super) fn query_controls(&self) -> bool {
        if matches!(
            self.target.kind,
            turbodbn_services::DatabaseKind::Redis | turbodbn_services::DatabaseKind::External
        ) {
            return false;
        }
        self.notebook
            .as_ref()
            .is_none_or(|note| !note.stale && note.source.object.is_some())
    }
    pub(crate) fn compact_note_result(&mut self) {
        if let Some(note) = &mut self.notebook {
            note.compact = true;
        }
    }
    pub(crate) fn freeze_note_page(&mut self, cx: &mut Context<Self>) {
        if let Some(note) = &mut self.notebook {
            note.page_only = true;
            note.stale = true;
        }
        self.sync_grid(cx);
    }
    pub(crate) fn stale_result(&mut self, cx: &mut Context<Self>) {
        let Some(note) = &mut self.notebook else {
            return;
        };
        if note.stale {
            return;
        }
        note.stale = true;
        self.sync_grid(cx);
    }
    pub(crate) fn cancel_result(&mut self, discard: bool, cx: &mut Context<Self>) {
        let id = self.notebook.as_ref().and_then(|n| n.ticket);
        if let Some(id) = id {
            self.note_log(id, crate::operation_log::Phase::Cancelled, None, cx);
            self.notice = t(cx, "notes.stopped").to_string();
            if self.committing {
                crate::feedback::error(self.notice.clone(), cx);
            }
        }
        if let Some(note) = &mut self.notebook {
            note.ticket = None;
            note.session = None;
            note.task = None;
        }
        self.busy = false;
        self.counting = false;
        self.committing = false;
        if discard {
            self.changes.clear();
            self.insert_count = 0;
        }
        self.sync_grid(cx);
    }
    fn note_log(
        &self,
        id: Uuid,
        phase: crate::operation_log::Phase,
        error: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let Some(output) = self
            .source
            .upgrade()
            .and_then(|db| db.read(cx).output.clone())
        else {
            return;
        };
        let mut record =
            crate::operation_log::Record::new(id, crate::operation_log::Kind::Query, phase);
        record.source = Some(self.target.connection);
        record.elapsed_ms = self
            .notebook
            .as_ref()
            .map_or(0, |n| n.started.elapsed().as_millis() as u64);
        if let Some(error) = error {
            record.failure = crate::operation_log::failure_class(error);
        }
        output.update(cx, |output, cx| output.operation(record, cx));
    }
    pub(super) fn note_request(&mut self, action: Action, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(note) = &self.notebook else {
            return;
        };
        if note.stale
            || !self
                .source
                .upgrade()
                .is_some_and(|db| db.read(cx).connection_snapshot_current(&note.saved))
        {
            self.receive(Err(t(cx, "notes.results_stale").to_string()), cx);
            return;
        }
        let mut commands = vec![];
        if let Some(key) = turbodbn_services::database::notebook_query::command_required(
            note.saved.profile.kind,
            &note.plan.sql,
        ) {
            commands.push(key);
        }
        match &action {
            Action::Commit { changes, .. } => {
                for change in changes {
                    commands.push(if change.original.is_none() {
                        "insert"
                    } else if change.values.is_none() {
                        "delete"
                    } else {
                        "update"
                    });
                }
            }
            Action::Count { .. } => commands.push("select"),
            _ => {}
        }
        if let Some(advice) = commands.into_iter().find_map(|key| {
            crate::note_permissions::Advice::for_command(
                note.access_mode,
                &note.saved.profile,
                key,
                cx,
            )
        }) {
            self.receive(Err(advice.message(cx).to_string()), cx);
            cx.emit(advice);
            return;
        }
        let policy = &cx.global::<crate::preferences::Preferences>().database;
        let query_allowed = turbodbn_services::database::notebook_query::command_required(
            note.saved.profile.kind,
            &note.plan.sql,
        )
        .is_some_and(|key| {
            note.access_mode
                .allows_note_effective(key, policy.command_enabled(key))
        });
        if matches!(&action, Action::Count { .. }) && !policy.command_enabled("select") {
            self.receive(Err(t(cx, "settings.sql_policy_blocked").to_string()), cx);
            return;
        }
        let writes_allowed = match &action {
            Action::Commit { changes, .. } => changes.iter().all(|change| {
                let key = if change.original.is_none() {
                    "insert"
                } else if change.values.is_none() {
                    "delete"
                } else {
                    "update"
                };
                note.access_mode
                    .allows_note_effective(key, policy.command_enabled(key))
            }),
            _ => true,
        };
        if !query_allowed || !writes_allowed {
            self.receive(Err(t(cx, "settings.sql_policy_blocked").to_string()), cx);
            return;
        }
        let columns = self.rows.columns.clone();
        let (action, confirmed, counting) = match action {
            Action::GridPage { options, .. } => {
                (GridAction::Page { options, columns }, note.confirmed, false)
            }
            Action::Count { filter, .. } => {
                (GridAction::Count { filter, columns }, note.confirmed, true)
            }
            Action::Commit { ddl, changes, .. } if self.target.writable => (
                GridAction::Commit {
                    ddl,
                    columns,
                    changes,
                },
                true,
                false,
            ),
            _ => {
                self.receive(Err(t(cx, "grid.readonly").to_string()), cx);
                return;
            }
        };
        let saved = note.saved.clone();
        let request = Request {
            database: self.target.database.clone(),
            action: Action::NoteGrid {
                plan: note.plan.clone(),
                confirmed,
                action,
            },
        };
        let id = Uuid::new_v4();
        let note = self.notebook.as_mut().unwrap();
        note.ticket = Some(id);
        note.started = std::time::Instant::now();
        self.busy = true;
        self.counting = counting;
        self.note_log(id, crate::operation_log::Phase::Started, None, cx);
        let database = self.target.database.clone();
        let resolve = saved.clone();
        let endpoint = cx
            .background_executor()
            .spawn(async move { resolve.endpoint(database) });
        let task = cx.spawn(async move |view, cx| {
            let result = async {
                let endpoint = endpoint.await?;
                let (session, events) = Session::start(endpoint.profile, endpoint.password, false);
                view.update(cx, |this, _| {
                    if let Some(note) = &mut this.notebook {
                        note.session = Some(session);
                    }
                })
                .map_err(|_| "查询单元已关闭")?;
                while let Ok(event) = events.recv().await {
                    match event {
                        Event::Connected(_) => {
                            view.update(cx, |this, cx| {
                                if !this.source.upgrade().is_some_and(|db| {
                                    db.read(cx).connection_snapshot_current(&saved)
                                }) {
                                    return Err(t(cx, "notes.results_stale").to_string());
                                }
                                this.notebook
                                    .as_ref()
                                    .and_then(|n| n.session.as_ref())
                                    .ok_or("查询已取消")?
                                    .request(Operation {
                                        id: 1,
                                        kind: OperationKind::Workbench(Box::new(request.clone())),
                                    })
                            })
                            .map_err(|_| "查询单元已关闭")??;
                        }
                        Event::Operation { result, .. } => {
                            return match result {
                                Ok(OperationResult::Workbench(reply)) => Ok(*reply),
                                Ok(_) => Err("查询响应类型无效".into()),
                                Err(error) => Err(error),
                            };
                        }
                        Event::Failed(error) => return Err(error),
                        Event::Disconnected => return Err("查询连接已断开".into()),
                        _ => {}
                    }
                }
                Err("查询连接已关闭".into())
            }
            .await;
            let _ = view.update(cx, |this, cx| {
                if this.notebook.as_ref().and_then(|n| n.ticket) != Some(id) {
                    return;
                }
                this.note_log(
                    id,
                    if result.is_ok() {
                        crate::operation_log::Phase::Succeeded
                    } else {
                        crate::operation_log::Phase::Failed
                    },
                    result.as_ref().err().map(String::as_str),
                    cx,
                );
                if let Some(note) = &mut this.notebook {
                    note.session = None;
                    note.ticket = None;
                }
                let current = this
                    .source
                    .upgrade()
                    .is_some_and(|db| db.read(cx).connection_snapshot_current(&saved));
                this.receive(
                    if current {
                        result
                    } else {
                        Err(t(cx, "notes.results_stale").to_string())
                    },
                    cx,
                );
            });
        });
        self.notebook.as_mut().unwrap().task = Some(task);
        cx.notify();
    }
}

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
    plan: Plan,
    confirmed: bool,
    pub source: GridSource,
    pub stale: bool,
    ticket: Option<Uuid>,
    started: std::time::Instant,
    session: Option<Session>,
    task: Option<Task<()>>,
}
impl TableView {
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
        let writable = meta.editable
            && !(saved.profile.kind == turbodbn_services::DatabaseKind::Sqlite
                && saved.profile.sqlite_read_only)
            && !(saved.profile.kind == turbodbn_services::DatabaseKind::MySql
                && turbodbn_services::mysql::uses_legacy(&saved.profile));
        let target = OpenTable {
            connection: saved.profile.id,
            database,
            object,
            mode: Mode::Grid,
            generation: saved.generation,
            kind: saved.profile.kind,
            writable,
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
            plan: plan.clone(),
            confirmed,
            source: meta.clone(),
            stale: false,
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
        self.notebook
            .as_ref()
            .is_none_or(|note| !note.stale && note.source.object.is_some())
    }
    pub(crate) fn stale_result(&mut self, cx: &mut Context<Self>) {
        if let Some(note) = &mut self.notebook {
            note.stale = true;
        }
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

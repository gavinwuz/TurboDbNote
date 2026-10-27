use crate::{
    feedback,
    locale::{t, tr},
};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Disableable, Icon, IconName, Selectable, StyledExt, WindowExt,
    button::{Button, ButtonVariants},
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use turbodbn_core::{Cell, CellContent, Note, notebook::SqlDialect};
use turbodbn_services::{
    note_store,
    sql_script::{self, Prepared},
};

pub struct Imported {
    pub cells: Vec<Cell>,
    pub accepted: Arc<AtomicBool>,
}
pub struct ImportView {
    source: PathBuf,
    path: PathBuf,
    note: Note,
    dialect: Option<SqlDialect>,
    prepared: Option<Arc<Prepared>>,
    status: SharedString,
    busy: bool,
    epoch: u64,
    cancel: Arc<AtomicBool>,
    task: Option<Task<()>>,
    preview: Vec<sql_script::Entry>,
    output: Option<Entity<crate::chat::AiLog>>,
    job: uuid::Uuid,
    started: std::time::Instant,
}
impl EventEmitter<Imported> for ImportView {}
impl ImportView {
    pub fn new(
        source: PathBuf,
        path: PathBuf,
        note: Note,
        dialect: Option<SqlDialect>,
        output: Option<Entity<crate::chat::AiLog>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        if let Some(dialect) = dialect {
            cx.defer_in(window, move |this, window, cx| {
                this.parse(dialect, window, cx)
            });
        }
        Self {
            source,
            path,
            note,
            dialect,
            prepared: None,
            status: t(cx, "script.choose_dialect"),
            busy: false,
            epoch: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            task: None,
            preview: vec![],
            output,
            job: uuid::Uuid::new_v4(),
            started: std::time::Instant::now(),
        }
    }
    fn log(
        &self,
        phase: crate::operation_log::Phase,
        bytes: u64,
        error: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        if let Some(output) = &self.output {
            let mut record = crate::operation_log::Record::new(
                self.job,
                crate::operation_log::Kind::Import,
                phase,
            );
            record.bytes = bytes;
            record.elapsed_ms = self.started.elapsed().as_millis() as u64;
            if let Some(error) = error {
                record.failure = crate::operation_log::failure_class(error);
            }
            output.update(cx, |log, cx| log.operation(record, cx));
        }
    }
    fn parse(&mut self, dialect: SqlDialect, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.epoch += 1;
        let epoch = self.epoch;
        self.dialect = Some(dialect);
        self.busy = true;
        self.prepared = None;
        self.status = t(cx, "script.parsing");
        self.job = uuid::Uuid::new_v4();
        self.started = std::time::Instant::now();
        self.log(crate::operation_log::Phase::Started, 0, None, cx);
        let source = self.source.clone();
        let cancel = self.cancel.clone();
        let (sender, receiver) = async_channel::bounded(2);
        let work = cx.background_executor().spawn(async move {
            let mut last = std::time::Instant::now();
            let prepared = sql_script::prepare(&source, dialect, &cancel, |bytes, count| {
                if last.elapsed() >= std::time::Duration::from_millis(250) {
                    let _ = sender.try_send((bytes, count));
                    last = std::time::Instant::now();
                }
            })?;
            let preview = prepared.page()?;
            Ok::<_, String>((prepared, preview))
        });
        self.task = Some(cx.spawn_in(window, async move |owner, cx| {
            while let Ok((bytes, count)) = receiver.recv().await {
                if owner
                    .update(cx, |this, cx| {
                        if this.epoch == epoch {
                            this.status = if count > 0 {
                                tr(cx, "script.indexing", &[("count", &count.to_string())])
                            } else {
                                tr(
                                    cx,
                                    "script.reading",
                                    &[("size", &format!("{:.1}", bytes as f64 / 1048576.))],
                                )
                            };
                            this.log(crate::operation_log::Phase::Progress, bytes, None, cx);
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    return;
                }
            }
            let result = work.await;
            let _ = owner.update(cx, |this, cx| {
                if this.epoch != epoch {
                    return;
                }
                this.busy = false;
                match result {
                    Ok((prepared, preview)) => {
                        this.preview = preview;
                        this.log(
                            crate::operation_log::Phase::Prepared,
                            prepared.script.bytes,
                            None,
                            cx,
                        );
                        this.status = tr(
                            cx,
                            "script.summary",
                            &[
                                ("count", &prepared.script.statements.to_string()),
                                ("writes", &prepared.script.writes.to_string()),
                                (
                                    "reads",
                                    &(prepared.script.statements
                                        - prepared.script.writes
                                        - prepared.script.unsupported)
                                        .to_string(),
                                ),
                                ("unsupported", &prepared.script.unsupported.to_string()),
                                (
                                    "size",
                                    &format!("{:.1}", prepared.script.bytes as f64 / 1048576.),
                                ),
                            ],
                        );
                        this.prepared = Some(Arc::new(prepared));
                    }
                    Err(error) => {
                        this.log(crate::operation_log::Phase::Failed, 0, Some(&error), cx);
                        this.status = crate::locale::message(cx, &error);
                        feedback::error(this.status.clone(), cx);
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(prepared) = self.prepared.clone() else {
            return;
        };
        let path = self.path.clone();
        let note = self.note.clone();
        let cancel = self.cancel.clone();
        let epoch = self.epoch;
        self.busy = true;
        self.status = t(cx, "script.importing");
        let work = cx.background_executor().spawn(async move {
            let cells = vec![Cell::new(
                prepared.script.name.clone(),
                CellContent::SqlScript {
                    script: prepared.script.clone(),
                    connection_id: None,
                    database: None,
                },
            )];
            let mut candidate = note;
            candidate.cells.extend(cells.clone());
            if candidate.cells.len() > note_store::MAX_CELLS
                || candidate
                    .to_json()
                    .map_err(|_| "script.error.resource")?
                    .len() as u64
                    > note_store::MAX_NOTE_BYTES
            {
                return Err("script.error.note_limit".to_string());
            }
            prepared.publish(&path, &cancel)?;
            if cancel.load(Ordering::Relaxed) {
                return Err("script.cancelled".into());
            }
            Ok::<_, String>(cells)
        });
        self.task = Some(cx.spawn_in(window, async move |owner, cx| {
            let result = work.await;
            let _ = owner.update_in(cx, |this, window, cx| {
                if this.epoch != epoch {
                    return;
                }
                this.busy = false;
                match result {
                    Ok(cells) => {
                        let accepted = Arc::new(AtomicBool::new(false));
                        cx.emit(Imported {
                            cells,
                            accepted: accepted.clone(),
                        });
                        cx.defer_in(window, move |this, window, cx| {
                            if !accepted.load(Ordering::Relaxed) {
                                this.status = t(cx, "notes.preview_changed");
                                this.log(
                                    crate::operation_log::Phase::Failed,
                                    0,
                                    Some("notes.preview_changed"),
                                    cx,
                                );
                                cx.notify();
                                return;
                            }
                            this.log(
                                crate::operation_log::Phase::Succeeded,
                                this.prepared
                                    .as_ref()
                                    .map_or(0, |prepared| prepared.script.bytes),
                                None,
                                cx,
                            );
                            window.close_dialog(cx);
                            feedback::success(t(cx, "script.imported"), cx);
                        });
                    }
                    Err(error) => {
                        this.log(crate::operation_log::Phase::Failed, 0, Some(&error), cx);
                        this.status = crate::locale::message(cx, &error);
                        feedback::error(this.status.clone(), cx);
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        self.cancel.store(true, Ordering::Relaxed);
        self.epoch += 1;
        if self.busy {
            self.log(crate::operation_log::Phase::Cancelled, 0, None, cx);
        }
        self.busy = false;
    }
}
impl Drop for ImportView {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
impl Render for ImportView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut dialects = div().h_flex().gap_3().child(t(cx, "script.dialect"));
        for (dialect, name) in [
            (SqlDialect::MySql, "MySQL"),
            (SqlDialect::PostgreSql, "PostgreSQL"),
            (SqlDialect::Sqlite, "SQLite"),
            (SqlDialect::ClickHouse, "ClickHouse"),
        ] {
            dialects = dialects.child(
                Button::new(SharedString::from(format!("script-dialect-{name}")))
                    .label(name)
                    .selected(self.dialect == Some(dialect))
                    .disabled(self.busy)
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.parse(dialect, window, cx)),
                    ),
            );
        }
        div()
            .v_flex()
            .gap_3()
            .child(
                div().min_w_0().truncate().child(
                    self.source
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                ),
            )
            .child(dialects)
            .when(!self.preview.is_empty(), |el| {
                el.child(
                    div()
                        .v_flex()
                        .gap_1()
                        .children(self.preview.iter().take(6).map(|entry| {
                            div()
                                .h_flex()
                                .gap_2()
                                .h(px(26.))
                                .min_w_0()
                                .child((entry.number + 1).to_string())
                                .child(Icon::new(if !entry.supported {
                                    IconName::TriangleAlert
                                } else if entry.write {
                                    IconName::ArrowRight
                                } else {
                                    IconName::Info
                                }))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .child(entry.preview.clone()),
                                )
                        })),
                )
            })
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(Icon::new(
                        if self
                            .prepared
                            .as_ref()
                            .is_some_and(|prepared| prepared.script.unsupported > 0)
                        {
                            IconName::TriangleAlert
                        } else {
                            IconName::Info
                        },
                    ))
                    .child(self.status.clone()),
            )
            .child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .whitespace_normal()
                    .child(t(cx, "script.import_hint")),
            )
            .child(
                feedback::actions()
                    .child(feedback::close_button(cx).on_click(cx.listener(
                        |this, _, window, cx| {
                            this.cancel(cx);
                            window.close_dialog(cx);
                        },
                    )))
                    .child(
                        Button::new("confirm-script-import")
                            .primary()
                            .icon(IconName::ArrowDown)
                            .label(t(cx, "script.import"))
                            .disabled(self.busy || self.prepared.is_none())
                            .on_click(cx.listener(|this, _, window, cx| this.import(window, cx))),
                    ),
            )
    }
}

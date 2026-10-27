use super::*;
use crate::locale::{message, tr};
use turbodbn_services::note_sql_export;

struct ConnectionExport {
    note: Note,
    profiles: Vec<turbodbn_services::mysql::Profile>,
    path: PathBuf,
    choices: Vec<(Uuid, usize)>,
}
enum ExportPreparation {
    Ready(note_sql_export::Export),
    Choose(std::sync::Arc<ConnectionExport>),
}

impl NoteView {
    pub(super) fn export_sql(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.picking || self.saving || self.cells.iter().any(|c| c.read(cx).indexing(cx)) {
            return;
        }
        let (mut note, cells) = self.snapshot_deferred(cx);
        let profiles = self.connections.read(cx).note_profiles();
        let path = self.path.clone();
        self.picking = true;
        feedback::notify(
            gpui_component::notification::NotificationType::Info,
            t(cx, "notes.export_preparing"),
            cx,
        );
        let task = Uuid::new_v4();
        let started = std::time::Instant::now();
        self.export_log(
            task,
            started,
            crate::operation_log::Phase::Started,
            None,
            cx,
        );
        cx.spawn_in(window, async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    note.cells = cells
                        .into_iter()
                        .map(crate::cell::CellSnapshot::finish)
                        .collect();
                    let choices = note_sql_export::connections(&note)?;
                    if choices.len() > 1 {
                        Ok(ExportPreparation::Choose(std::sync::Arc::new(
                            ConnectionExport {
                                note,
                                profiles,
                                path,
                                choices,
                            },
                        )))
                    } else {
                        note_sql_export::prepare(&note, &path, &profiles)
                            .map(ExportPreparation::Ready)
                    }
                })
                .await;
            let _ = view.update_in(cx, |this, window, cx| {
                this.finish_sql_export(result, task, started, window, cx);
            });
        })
        .detach();
        cx.notify();
    }
    fn finish_sql_export(
        &mut self,
        result: Result<ExportPreparation, String>,
        task: Uuid,
        started: std::time::Instant,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.picking = false;
        match result {
            Ok(ExportPreparation::Ready(export)) => {
                self.export_log(
                    task,
                    started,
                    crate::operation_log::Phase::Succeeded,
                    None,
                    cx,
                );
                self.export_sql_preview(export, window, cx);
            }
            Ok(ExportPreparation::Choose(request)) => {
                self.export_log(
                    task,
                    started,
                    crate::operation_log::Phase::Prepared,
                    None,
                    cx,
                );
                self.export_connection_dialog(request, window, cx);
            }
            Err(error) => {
                self.export_log(
                    task,
                    started,
                    crate::operation_log::Phase::Failed,
                    Some(&error),
                    cx,
                );
                feedback::error(message(cx, &error), cx);
            }
        }
        if self.flush_again {
            self.flush_again = false;
            self.save_now(cx);
        } else if self.dirty() {
            self.schedule(cx);
        }
        cx.emit(NoteEvent::Ready);
        cx.notify();
    }
    fn export_connection_dialog(
        &self,
        request: std::sync::Arc<ConnectionExport>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let mut choices = div()
                .id("note-export-connections")
                .v_flex()
                .gap_1()
                .max_h(px(300.))
                .overflow_y_scroll();
            for (index, (id, count)) in request.choices.iter().enumerate() {
                let profile = request.profiles.iter().find(|profile| profile.id == *id);
                let label = profile
                    .map(note_sql_export::connection_label)
                    .unwrap_or_else(|| t(cx, "notes.export_missing_connection").to_string());
                let label = tr(
                    cx,
                    "notes.export_connection_choice",
                    &[("connection", &label), ("cells", &count.to_string())],
                );
                let request = request.clone();
                let view = view.clone();
                let id = *id;
                choices = choices.child(
                    Button::new(("export-connection", index))
                        .ghost()
                        .w_full()
                        .justify_start()
                        .icon(IconName::ChevronRight)
                        .label(label.clone())
                        .tooltip(label)
                        .disabled(profile.is_none())
                        .on_click(move |_, window, cx| {
                            window.close_dialog(cx);
                            let request = request.clone();
                            let _ = view.update(cx, |this, cx| {
                                this.export_selected_connection(request, id, window, cx)
                            });
                        }),
                );
            }
            dialog
                .title(t(cx, "notes.export_choose_connection"))
                .w(px(660.))
                .on_ok(|_, _, _| false)
                .child(
                    div()
                        .v_flex()
                        .gap_3()
                        .child(t(cx, "notes.export_connections_hint"))
                        .child(choices),
                )
                .footer(|_, _, _, cx| {
                    vec![feedback::footer_actions().child(feedback::close_button(cx))]
                })
        });
    }
    fn export_selected_connection(
        &mut self,
        request: std::sync::Arc<ConnectionExport>,
        connection: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.picking || self.deleting {
            return;
        }
        self.picking = true;
        let task = Uuid::new_v4();
        let started = std::time::Instant::now();
        self.export_log(
            task,
            started,
            crate::operation_log::Phase::Started,
            None,
            cx,
        );
        cx.spawn_in(window, async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    note_sql_export::prepare_selected(
                        &request.note,
                        &request.path,
                        &request.profiles,
                        connection,
                    )
                    .map(ExportPreparation::Ready)
                })
                .await;
            let _ = view.update_in(cx, |this, window, cx| {
                this.finish_sql_export(result, task, started, window, cx)
            });
        })
        .detach();
        cx.notify();
    }
    fn export_log(
        &self,
        task: Uuid,
        started: std::time::Instant,
        phase: crate::operation_log::Phase,
        error: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        if let Some(output) = self.connections.read(cx).output.clone() {
            let mut record =
                crate::operation_log::Record::new(task, crate::operation_log::Kind::Export, phase);
            record.elapsed_ms = started.elapsed().as_millis() as u64;
            if let Some(error) = error {
                record.failure = crate::operation_log::failure_class(error);
            }
            output.update(cx, |output, cx| output.operation(record, cx));
        }
    }
    fn export_sql_preview(
        &self,
        export: note_sql_export::Export,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let summary = tr(
            cx,
            "notes.export_summary",
            &[
                ("cells", &export.cells.to_string()),
                ("skipped", &export.skipped.to_string()),
                ("databases", &export.databases.join(", ")),
            ],
        );
        let connection_label = export.connection_label.clone();
        let filtered = tr(
            cx,
            "notes.export_filtered",
            &[("count", &export.excluded.to_string())],
        );
        let has_excluded = export.excluded > 0;
        let limited = export.sql.chars().count() > 64_000;
        let preview = export.sql.chars().take(64_000).collect::<String>();
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .default_value(preview)
        });
        let sql = std::sync::Arc::new(export.sql);
        let name = format!(
            "{}_{}.sql",
            self.path.file_stem().unwrap_or_default().to_string_lossy(),
            chrono::Local::now().format("%Y%m%d_%H%M%S")
        );
        let view = cx.entity().downgrade();
        let native_use = matches!(
            export.kind,
            turbodbn_services::DatabaseKind::MySql | turbodbn_services::DatabaseKind::ClickHouse
        );
        window.open_dialog(cx, move |dialog, _, cx| {
            let sql = sql.clone();
            let name = name.clone();
            let view = view.clone();
            dialog
                .title(t(cx, "notes.export_sql"))
                .w(px(760.))
                .on_ok(|_, _, _| false)
                .child(
                    div()
                        .v_flex()
                        .gap_2()
                        .child(connection_label.clone())
                        .child(summary.clone())
                        .when(has_excluded, |el| {
                            el.child(div().text_color(cx.theme().warning).child(filtered.clone()))
                        })
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(t(
                                    cx,
                                    if native_use {
                                        "notes.export_hint"
                                    } else {
                                        "notes.export_single_database"
                                    },
                                )),
                        )
                        .when(limited, |el| {
                            el.child(t(cx, "notes.export_preview_limited"))
                        })
                        .child(Input::new(&input).disabled(true).h(px(340.))),
                )
                .footer(move |_, _, _, cx| {
                    let sql = sql.clone();
                    let name = name.clone();
                    let view = view.clone();
                    vec![
                        feedback::footer_actions()
                            .child(feedback::close_button(cx))
                            .child(
                                Button::new("save-note-sql")
                                    .primary()
                                    .icon(IconName::FolderOpen)
                                    .label(t(cx, "notes.export_sql"))
                                    .on_click(move |_, window, cx| {
                                        let directory = dirs::document_dir()
                                            .unwrap_or_else(|| PathBuf::from("."));
                                        let selected =
                                            cx.prompt_for_new_path(&directory, Some(&name));
                                        let sql = sql.clone();
                                        let view = view.clone();
                                        let task = Uuid::new_v4();
                                        let started = std::time::Instant::now();
                                        let _ = view.update(cx, |this, cx| {
                                            this.export_log(
                                                task,
                                                started,
                                                crate::operation_log::Phase::Started,
                                                None,
                                                cx,
                                            )
                                        });
                                        window.close_dialog(cx);
                                        cx.spawn(async move |cx| {
                                            let result = match selected.await {
                                                Ok(Ok(Some(mut path))) => {
                                                    if path.extension().is_none() {
                                                        path.set_extension("sql");
                                                    }
                                                    cx.background_executor()
                                                        .spawn(async move {
                                                            use std::io::Write;
                                                            let parent = path.parent().ok_or(
                                                                "notes.export_save_failed",
                                                            )?;
                                                            let mut temp =
                                                                tempfile::NamedTempFile::new_in(
                                                                    parent,
                                                                )
                                                                .map_err(
                                                                    |_| "notes.export_save_failed",
                                                                )?;
                                                            temp.write_all(sql.as_bytes())
                                                                .map_err(
                                                                    |_| "notes.export_save_failed",
                                                                )?;
                                                            temp.as_file().sync_all().map_err(
                                                                |_| "notes.export_save_failed",
                                                            )?;
                                                            temp.persist_noclobber(&path).map_err(
                                                                |_| "notes.export_save_failed",
                                                            )?;
                                                            Ok::<_, &str>(true)
                                                        })
                                                        .await
                                                }
                                                Ok(Ok(None)) => Ok(false),
                                                _ => Err("notes.export_save_failed"),
                                            };
                                            let _ = view.update(cx, |this, cx| {
                                                use crate::operation_log::Phase;
                                                this.export_log(
                                                    task,
                                                    started,
                                                    match result {
                                                        Ok(true) => Phase::Succeeded,
                                                        Ok(false) => Phase::Cancelled,
                                                        Err(_) => Phase::Failed,
                                                    },
                                                    result.as_ref().err().copied(),
                                                    cx,
                                                );
                                                match result {
                                                    Ok(true) => feedback::success(
                                                        t(cx, "feedback.saved"),
                                                        cx,
                                                    ),
                                                    Err(error) => {
                                                        feedback::error(message(cx, error), cx)
                                                    }
                                                    _ => {}
                                                }
                                            });
                                        })
                                        .detach();
                                    }),
                            ),
                    ]
                })
        });
    }
}

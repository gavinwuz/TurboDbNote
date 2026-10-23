use super::*;
impl CellView {
    pub(super) fn execution_sql(
        &mut self,
        explain: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let source = self.editor.read(cx).value().to_string();
        let Some(kind) = self.assist_context(cx).0 else {
            return Some(source);
        };
        let Some(dialect) = turbodbn_services::sql_script::dialect(kind) else {
            return Some(source);
        };
        // Batch execution freezes whole cells; never silently run a subset of a batch item.
        let cursor = self.editor.read(cx).cursor();
        let selection = if self.batch_token.is_some() {
            0..source.len()
        } else if self.editing {
            self.editor.read(cx).selection_byte_range()
        } else {
            cursor..cursor
        };
        let ranges = match turbodbn_services::sql_script::editor_query_target(
            &source, dialect, selection, cursor, explain,
        ) {
            Ok(value) => value,
            Err(error) => {
                feedback::error(crate::locale::message(cx, &error), cx);
                self.finish_batch(false, cx);
                return None;
            }
        };
        if ranges.len() == 1 && (explain || ranges[0].len() <= 60_000) {
            return Some(source[ranges[0].clone()].to_owned());
        }
        if !explain {
            self.start_sql_batch(
                source[ranges[0].start..ranges.last().unwrap().end].to_owned(),
                window,
                cx,
            );
            return None;
        }
        self.finish_batch(false, cx);
        feedback::error(t(cx, "notes.query.explain_one"), cx);
        None
    }
}

impl CellView {
    fn start_sql_batch(&mut self, source: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy() || self.result_pending(cx) {
            return;
        }
        let Ok((saved, database)) = self.binding(cx) else {
            feedback::error(t(cx, "notes.choose_connection"), cx);
            self.finish_batch(false, cx);
            return;
        };
        let Some(dialect) = turbodbn_services::sql_script::dialect(saved.profile.kind) else {
            return;
        };
        let task_id = Uuid::new_v4();
        let started = std::time::Instant::now();
        self.operation = Some((task_id, started));
        self.log(
            task_id,
            crate::operation_log::Phase::Started,
            0,
            0,
            None,
            cx,
        );
        let revision = self.revision;
        let path = self.note_path.clone();
        let target_path = path.clone();
        if source.len() > turbodbn_core::notebook::MAX_SCRIPT_TEXT_BYTES {
            feedback::error(t(cx, "notes.script.too_large"), cx);
            self.operation = None;
            self.log(
                task_id,
                crate::operation_log::Phase::Failed,
                0,
                0,
                Some("script.error.resource"),
                cx,
            );
            self.finish_batch(false, cx);
            return;
        }
        let work = cx.background_executor().spawn(async move {
            let prepared = turbodbn_services::sql_script::prepare_text(
                &source,
                dialect,
                &std::sync::atomic::AtomicBool::new(false),
            )?;
            let script = prepared.script.clone();
            prepared.publish(&target_path, &std::sync::atomic::AtomicBool::new(false))?;
            Ok(script)
        });
        self.batch_preparing = Some(cx.spawn_in(window, async move |cell, cx| {
            let result = work.await;
            let _ = cell.update_in(cx, |this, window, cx| {
                this.batch_preparing = None;
                this.operation = None;
                this.log(
                    task_id,
                    if result.is_ok() {
                        crate::operation_log::Phase::Succeeded
                    } else {
                        crate::operation_log::Phase::Failed
                    },
                    started.elapsed().as_millis() as u64,
                    0,
                    result.as_ref().err().map(String::as_str),
                    cx,
                );
                if this.revision != revision
                    || this.note_path != path
                    || !this
                        .connections
                        .read(cx)
                        .connection_snapshot_current(&saved)
                {
                    this.finish_batch(false, cx);
                    return;
                }
                let script = match result {
                    Ok(script) => script,
                    Err(error) => {
                        feedback::error(crate::locale::message(cx, &error), cx);
                        this.finish_batch(false, cx);
                        return;
                    }
                };
                let connections = this.connections.clone();
                let view = cx.new(|cx| {
                    crate::script_cell::ScriptView::new(path, script, connections, window, cx)
                });
                view.update(cx, |v, cx| {
                    v.compact_results();
                    v.set_access_mode(this.access_mode, cx);
                });
                this.batch_subscription =
                    Some(
                        cx.subscribe_in(&view, window, |this, _, event, _, cx| match event {
                            crate::script_cell::ScriptEvent::Finished { run, success } => {
                                if this.script_run_id == *run {
                                    this.script_running = false;
                                    this.finish_batch(*success, cx);
                                    cx.notify();
                                }
                            }
                            crate::script_cell::ScriptEvent::PermissionBlocked(advice) => {
                                cx.emit(CellEvent::PermissionBlocked(advice.clone()))
                            }
                            _ => {}
                        }),
                    );
                this.results = None;
                this.execution_summary = None;
                this.notice.clear();
                this.batch_view = Some(view.clone());
                let (active, run) = view.update(cx, |v, cx| {
                    let active = v.run(saved, database, false, window, cx);
                    (active, v.generation())
                });
                this.script_running = active;
                this.script_run_id = run;
                if !active {
                    this.finish_batch(false, cx);
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
}

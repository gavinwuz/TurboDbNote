use super::*;
actions!(note_sql, [ToggleComments]);
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
                let token = self.query.begin();
                self.failed(token, error, cx);
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
                        let token = this.query.begin();
                        this.failed(token, error, cx);
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

/// Toggle whole selected lines, leaving a trailing boundary line untouched.
fn comment_edit(
    source: &str,
    selection: std::ops::Range<usize>,
) -> (std::ops::Range<usize>, String) {
    let start = source[..selection.start].rfind('\n').map_or(0, |n| n + 1);
    let last = if selection.end > selection.start && source[..selection.end].ends_with('\n') {
        selection.end - 1
    } else {
        selection.end
    };
    let end = source[last..].find('\n').map_or(source.len(), |n| last + n);
    let block = &source[start..end];
    if selection.is_empty() && block.trim().is_empty() {
        let indent = block.trim_end_matches('\r');
        return (
            start..end,
            format!(
                "{indent}-- {}",
                if block.ends_with('\r') { "\r" } else { "" }
            ),
        );
    }
    let uncomment = block
        .lines()
        .filter(|line| !line.trim().is_empty())
        .all(|line| line.trim_start().starts_with("-- ") || line.trim_start() == "--");
    let text = block
        .split('\n')
        .map(|line| {
            let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
            let (spaces, body) = line.split_at(indent);
            if body.trim().is_empty() {
                return line.to_string();
            }
            if uncomment {
                format!(
                    "{spaces}{}",
                    body.strip_prefix("-- ")
                        .or_else(|| body.strip_prefix("--"))
                        .unwrap_or(body)
                )
            } else {
                format!("{spaces}-- {body}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    (start..end, text)
}
impl CellView {
    pub(super) fn toggle_sql_comments(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |input, cx| {
            if input.marked_text_range(window, cx).is_some() {
                return;
            }
            let source = input.value();
            let selection = input.selection_byte_range();
            let (range, text) = comment_edit(&source, selection.clone());
            let utf16 = source[..range.start].encode_utf16().count()
                ..source[..range.end].encode_utf16().count();
            input.break_undo_group();
            input.replace_text_in_range(Some(utf16), &text, window, cx);
            input.break_undo_group();
            if selection.is_empty() {
                let caret = (selection.start as isize + text.len() as isize
                    - (range.end - range.start) as isize)
                    .max(range.start as isize) as usize;
                input.select_byte_range(
                    caret.min(range.start + text.len())..caret.min(range.start + text.len()),
                    window,
                    cx,
                );
            } else {
                input.select_byte_range(range.start..range.start + text.len(), window, cx);
            }
        });
    }
}
#[cfg(test)]
mod comment_tests {
    use super::comment_edit;
    #[test]
    fn rapid_explicit_edits_have_independent_undo_steps() {
        #[derive(Clone, PartialEq)]
        struct Item(usize);
        impl gpui_component::history::HistoryItem for Item {
            fn version(&self) -> usize {
                self.0
            }
            fn set_version(&mut self, version: usize) {
                self.0 = version;
            }
        }
        let mut history = gpui_component::history::History::new()
            .group_interval(std::time::Duration::from_secs(60));
        history.push(Item(0));
        history.break_group();
        history.push(Item(0));
        history.break_group();
        history.push(Item(0));
        assert_eq!(history.undo().unwrap().len(), 1);
        assert_eq!(history.undo().unwrap().len(), 1);
        assert_eq!(history.undo().unwrap().len(), 1);
    }
    #[test]
    fn shortcut_is_scoped_to_sql_editor_input() {
        let predicate = gpui::KeyBindingContextPredicate::parse("NoteSql > Input").unwrap();
        let sql = [
            gpui::KeyContext::parse("NoteSql").unwrap(),
            gpui::KeyContext::parse("Input").unwrap(),
        ];
        assert!(predicate.eval_inner(&sql, &sql));
        for name in ["NoteMarkdown", "Workspace", "Settings"] {
            let contexts = [
                gpui::KeyContext::parse(name).unwrap(),
                gpui::KeyContext::parse("Input").unwrap(),
            ];
            assert!(!predicate.eval_inner(&contexts, &contexts));
        }
    }
    #[test]
    fn comments_preserve_unicode_crlf_indent_and_selection_boundary() {
        let source = "  SELECT '中文';\r\n\tSELECT 2;\r\nSELECT 3;";
        let end = source.find("SELECT 3").unwrap();
        let (range, commented) = comment_edit(source, 0..end);
        assert_eq!(commented, "  -- SELECT '中文';\r\n\t-- SELECT 2;\r");
        let (_, restored) = comment_edit(&commented, 0..commented.len());
        assert_eq!(restored, &source[range]);
    }
    #[test]
    fn blank_line_can_be_commented() {
        assert_eq!(comment_edit("", 0..0).1, "-- ");
        assert_eq!(comment_edit("  \r\n", 2..2).1, "  -- \r");
    }
    #[test]
    fn mixed_selection_comments_all_lines_in_one_edit() {
        assert_eq!(comment_edit("-- one\ntwo", 0..10).1, "-- -- one\n-- two");
    }
}

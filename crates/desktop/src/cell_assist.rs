use super::*;
use gpui_component::{
    Rope, RopeExt,
    input::{CompletionProvider, Enter, Escape},
};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionResponse, CompletionTextEdit, Documentation,
    TextEdit,
};
use turbodbn_services::database::sql_assist::{self, FormatError, SuggestionKind};

pub(super) struct SqlCompletion(pub WeakEntity<CellView>);
impl CompletionProvider for SqlCompletion {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        _: CompletionContext,
        window: &mut Window,
        cx: &mut Context<InputState>,
    ) -> Task<anyhow::Result<CompletionResponse>> {
        let owner = self.0.clone();
        window.defer(cx, move |window, cx| {
            let _ = owner.update(cx, |cell, cx| {
                if cell.editing {
                    cell.prepare_columns(window, cx);
                }
            });
        });
        let items = self
            .0
            .upgrade()
            .filter(|_| text.len() <= sql_assist::MAX_SOURCE)
            .map(|view| {
                let view = view.read(cx);
                let (kind, id, database) = view.assist_context(cx);
                let tables = id
                    .map(|id| view.connections.read(cx).note_tables(id, &database))
                    .unwrap_or_default();
                let source = text.to_string();
                let suggestions =
                    if let Some(request) = sql_assist::column_request(kind, &source, offset) {
                        id.and_then(|id| {
                            let db = view.connections.read(cx);
                            let object = db.note_column_object(id, &database, &request.table)?;
                            db.cached_note_columns(id, &database, &object)
                        })
                        .map(|columns| sql_assist::column_suggestions(kind, &request, &columns))
                        .unwrap_or_default()
                    } else {
                        sql_assist::complete(kind, &source, offset, &tables)
                    };
                suggestions
                    .into_iter()
                    .map(|s| {
                        let category = match s.kind {
                            SuggestionKind::Keyword => "sql_assist.keyword",
                            SuggestionKind::Function => "sql_assist.function",
                            SuggestionKind::Table => "sql_assist.table",
                            SuggestionKind::Query => "sql_assist.query",
                            SuggestionKind::Command => "sql_assist.command",
                            SuggestionKind::Column => "sql_assist.column",
                        };
                        let detail = if s.kind == SuggestionKind::Column {
                            s.signature.clone()
                        } else {
                            t(cx, category).to_string()
                        };
                        let doc = if s.signature.is_empty() || s.kind == SuggestionKind::Query {
                            s.insert.clone()
                        } else {
                            s.signature.clone()
                        };
                        CompletionItem {
                            label: if s.kind == SuggestionKind::Column {
                                format!("{}  {}", column_badge(&s.signature), s.label)
                            } else {
                                s.label
                            },
                            detail: Some(detail),
                            documentation: Some(Documentation::String(doc)),
                            // Component highlights byte ranges, so do not reuse an unrelated trigger query.
                            filter_text: Some(String::new()),
                            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                                range: lsp_types::Range::new(
                                    text.offset_to_position(s.range.start),
                                    text.offset_to_position(s.range.end),
                                ),
                                new_text: s.insert,
                            })),
                            ..Default::default()
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Task::ready(Ok(CompletionResponse::Array(items)))
    }
    fn is_completion_trigger(
        &self,
        _: usize,
        _new_text: &str,
        _cx: &mut Context<InputState>,
    ) -> bool {
        true
    }
}

fn column_badge(signature: &str) -> &'static str {
    let ty = signature
        .split(" · ")
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    if ["INT", "DEC", "NUM", "FLOAT", "DOUBLE", "REAL"]
        .iter()
        .any(|v| ty.contains(v))
    {
        "123"
    } else if ["DATE", "TIME"].iter().any(|v| ty.contains(v)) {
        "◷"
    } else if ty.contains("BOOL") {
        "☑"
    } else if ty.contains("JSON") {
        "{}"
    } else if ["BLOB", "BINARY", "BYTE"].iter().any(|v| ty.contains(v)) {
        "01"
    } else {
        "ABC"
    }
}

impl CellView {
    pub(super) fn prepare_columns(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (kind, id, database) = self.assist_context(cx);
        let editor = self.editor.read(cx);
        let Some(request) = sql_assist::column_request(kind, &editor.value(), editor.cursor())
        else {
            return;
        };
        let Some(id) = id else {
            return;
        };
        let db = self.connections.read(cx);
        let Some(object) = db.note_column_object(id, &database, &request.table) else {
            return;
        };
        if db.cached_note_columns(id, &database, &object).is_some() || self.column_wait.is_some() {
            return;
        }
        // Automatic work never retries a failed or manually disconnected profile.
        if db.note_connection_state(id) == Some(false) {
            return;
        }
        let attempt = format!(
            "{id}:{}:{database}:{}:{}",
            db.note_generation(id),
            object.schema,
            object.name
        );
        if self.column_auto_attempt.as_ref() == Some(&attempt) {
            return;
        }
        self.column_auto_attempt = Some(attempt);
        self.load_columns(window, cx);
    }

    pub(super) fn publish_ddl_context(&self, open: bool, cx: &mut Context<Self>) {
        if !self.note_active {
            return;
        }
        let (_, id, database) = self.assist_context(cx);
        let scope = if self.editing {
            id.filter(|_| !database.is_empty()).map(|id| (id, database))
        } else {
            None
        };
        feedback::ddl_context(
            feedback::DdlContext {
                origin: if self.editing && scope.is_none() {
                    None
                } else {
                    Some(self.cell.id)
                },
                scope,
                object: None,
                open,
            },
            cx,
        );
    }
    pub(super) fn auto_connect(&self, cx: &mut Context<Self>) {
        if !matches!(self.cell.content, CellContent::Sql { .. }) {
            return;
        }
        let owner = cx.entity().downgrade();
        cx.defer(move |cx| {
            let _ = owner.update(cx, |this, cx| {
                let (_, id, database) = this.assist_context(cx);
                if !database.is_empty()
                    && let Some(id) = id
                {
                    this.connections
                        .update(cx, |db, cx| db.ensure_note_connection(id, false, cx));
                }
            });
        });
    }

    pub(super) fn load_columns(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (kind, id, database) = self.assist_context(cx);
        let editor = self.editor.read(cx);
        let Some(request) = sql_assist::column_request(kind, &editor.value(), editor.cursor())
        else {
            feedback::error(t(cx, "sql_assist.column_position"), cx);
            return;
        };
        let Some(id) = id else {
            feedback::error(t(cx, "notes.choose_connection"), cx);
            return;
        };
        let Some(object) =
            self.connections
                .read(cx)
                .note_column_object(id, &database, &request.table)
        else {
            feedback::error(t(cx, "sql_assist.columns_connect"), cx);
            return;
        };
        if self.column_wait.is_some() {
            return;
        }
        self.connections
            .update(cx, |db, cx| db.ensure_note_connection(id, true, cx));
        let snapshot = match self.connections.read(cx).saved_connection(id, cx) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                feedback::error(error, cx);
                return;
            }
        };
        let revision = self.revision;
        let scope = self.assist_context(cx);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        self.column_wait = Some(cx.spawn_in(window, async move |view, cx| {
            loop {
                let waiting = view
                    .update(cx, |this, cx| {
                        if this.revision != revision
                            || this.assist_context(cx) != scope
                            || !this
                                .connections
                                .read(cx)
                                .connection_snapshot_current(&snapshot)
                        {
                            this.column_wait = None;
                            this.column_auto_attempt = None;
                            cx.notify();
                            return false;
                        }
                        match this.connections.read(cx).note_connection_state(id) {
                            Some(false) => {
                                this.column_wait = None;
                                feedback::error(t(cx, "sql_assist.connection_failed"), cx);
                                cx.notify();
                                return false;
                            }
                            Some(true) => {
                                if this.connections.update(cx, |db, cx| {
                                    db.request_note_columns(
                                        id,
                                        database.clone(),
                                        object.clone(),
                                        cx,
                                    )
                                }) {
                                    this.column_wait = None;
                                    cx.notify();
                                    return false;
                                }
                            }
                            None => {}
                        }
                        if std::time::Instant::now() >= deadline {
                            this.column_wait = None;
                            feedback::error(t(cx, "sql_assist.columns_busy"), cx);
                            cx.notify();
                            return false;
                        }
                        true
                    })
                    .unwrap_or(false);
                if !waiting {
                    break;
                }
                gpui::Timer::after(std::time::Duration::from_millis(100)).await;
            }
        }));
        self.editor.update(cx, |input, cx| input.focus(window, cx));
    }
    pub(super) fn trigger_completion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let editor = self.editor.clone();
        window.defer(cx, move |window, cx| {
            editor.update(cx, |input, cx| {
                if !input.focus_handle(cx).is_focused(window) {
                    return;
                }
                let cursor = input.cursor();
                let utf16 = input.value()[..cursor].encode_utf16().count();
                input.replace_text_in_range(Some(utf16..utf16), "", window, cx);
            })
        });
    }
    /// Editing context does not load credentials or require an executable driver.
    pub(super) fn assist_context(&self, cx: &App) -> (Option<DatabaseKind>, Option<Uuid>, String) {
        let id = self.connection_id();
        let connections = self.connections.read(cx);
        let profile = id.and_then(|id| connections.note_profile(id));
        let value = self.database.read(cx).value().trim().to_owned();
        let database = if let Some(p) = &profile {
            if p.kind == DatabaseKind::Sqlite {
                std::path::Path::new(&p.sqlite_path)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            } else if !value.is_empty() {
                value
            } else if self.inherits_connection() && !self.inherited_database.is_empty() {
                self.inherited_database.clone()
            } else {
                p.database.clone()
            }
        } else {
            value
        };
        (profile.map(|p| p.kind), id, database)
    }
    pub(super) fn assist_execution_supported(&self, cx: &App) -> bool {
        matches!(
            self.assist_context(cx).0,
            Some(DatabaseKind::MySql | DatabaseKind::Sqlite | DatabaseKind::PostgreSql)
        )
    }
    pub(super) fn assist_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.editing
            || !matches!(self.cell.content, CellContent::Sql { .. })
            || !self.editor.read(cx).focus_handle(cx).is_focused(window)
        {
            return false;
        }
        let key = &event.keystroke.key;
        let modifiers = &event.keystroke.modifiers;
        if modifiers.control && (key == "space" || key == " ") {
            self.trigger_completion(window, cx);
            cx.stop_propagation();
            return true;
        }
        if modifiers.control && key.eq_ignore_ascii_case("e") {
            self.explain(window, cx);
            cx.stop_propagation();
            return true;
        }
        if modifiers.control && modifiers.shift && key.eq_ignore_ascii_case("f") {
            self.format_sql(window, cx);
            cx.stop_propagation();
            return true;
        }
        false
    }
    pub(super) fn assist_accept(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.editing
            || !matches!(self.cell.content, CellContent::Sql { .. })
            || !self.editor.read(cx).focus_handle(cx).is_focused(window)
        {
            return false;
        }
        self.editor.update(cx, |input, cx| {
            input.handle_action_for_context_menu(Box::new(Enter { secondary: false }), window, cx)
        })
    }
    pub(super) fn assist_enter(
        &mut self,
        action: &Enter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.assist_accept(window, cx) {
            cx.stop_propagation();
            return;
        }
        if self.editing
            && matches!(self.cell.content, CellContent::Sql { .. })
            && self.editor.read(cx).focus_handle(cx).is_focused(window)
            && (action.secondary
                || cx
                    .global::<crate::preferences::Preferences>()
                    .database
                    .sql_enter)
        {
            cx.stop_propagation();
            self.run(0, window, cx);
        } else {
            cx.propagate();
        }
    }
    pub(super) fn assist_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.editing {
            cx.propagate();
            return;
        }
        let handled = self.editor.update(cx, |input, cx| {
            input.handle_action_for_context_menu(Box::new(Escape), window, cx)
        });
        if !handled {
            self.set_editing(false, window, cx);
        }
        cx.stop_propagation();
    }
    pub(super) fn format_sql(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.working(cx) || self.batch_locked || self.formatting.is_some() {
            return;
        }
        if self.result_pending(cx) {
            feedback::error(t(cx, "grid.pending"), cx);
            return;
        }
        if self.editor.read(cx).text().len() > sql_assist::MAX_SOURCE {
            feedback::error(t(cx, "sql_assist.too_large"), cx);
            return;
        }
        let context = self.assist_context(cx);
        let source = self.editor.read(cx).value().to_string();
        let cursor = self.editor.read(cx).cursor_position();
        let original = source.clone();
        let revision = self.revision;
        let kind = context.0;
        let work = cx
            .background_executor()
            .spawn(async move { sql_assist::format(kind, &source) });
        self.formatting = Some(cx.spawn_in(window, async move |view, cx| {
            let result = work.await;
            let _ = view.update_in(cx, |this, window, cx| {
                this.formatting = None;
                if this.working(cx)
                    || this.batch_locked
                    || this.revision != revision
                    || this.assist_context(cx) != context
                    || this.editor.read(cx).value().as_ref() != original
                {
                    feedback::error(t(cx, "sql_assist.format_changed"), cx);
                    cx.notify();
                    return;
                }
                match result {
                    Ok(formatted) => {
                        if formatted != original {
                            this.editor.update(cx, |input, cx| {
                                let end = input.text().offset_to_position(input.text().len());
                                input.apply_lsp_edits(
                                    &vec![TextEdit {
                                        range: lsp_types::Range::new(Default::default(), end),
                                        new_text: formatted,
                                    }],
                                    window,
                                    cx,
                                );
                                input.set_cursor_position(cursor, window, cx);
                            });
                        }
                        feedback::success(t(cx, "sql_assist.formatted"), cx);
                    }
                    Err(error) => feedback::error(
                        t(
                            cx,
                            match error {
                                FormatError::Invalid => "sql_assist.format_invalid",
                                FormatError::Unsupported => "sql_assist.redis_format",
                                FormatError::TooLarge => "sql_assist.too_large",
                            },
                        ),
                        cx,
                    ),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
}

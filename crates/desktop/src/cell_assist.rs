#[path = "function_help.rs"]
mod function_help;
use super::*;
use crate::locale::tr;
use gpui_component::{
    Rope, RopeExt,
    input::{CompletionProvider, Enter, Escape},
};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionResponse, CompletionTextEdit, Documentation,
    TextEdit,
};
use turbodbn_services::database::sql_assist::{self, FormatError, SuggestionKind};

const COMPLETION_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(50);
fn identifier_start(text: &Rope, offset: usize) -> usize {
    offset
        - text
            .chars_at(offset)
            .reversed()
            .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '$'))
            .map(char::len_utf8)
            .sum::<usize>()
}

#[derive(Default)]
struct AssistBudget {
    slow: std::cell::Cell<u8>,
    until: std::cell::Cell<Option<std::time::Instant>>,
}
impl AssistBudget {
    fn allowed(&self, now: std::time::Instant) -> bool {
        if let Some(until) = self.until.get() {
            if now < until {
                return false;
            }
            self.until.set(None);
            self.slow.set(0);
        }
        true
    }
    fn record(&self, elapsed: std::time::Duration, now: std::time::Instant) {
        if elapsed >= std::time::Duration::from_millis(80) {
            let slow = self.slow.get().saturating_add(1);
            self.slow.set(slow);
            if slow >= 3 {
                self.until
                    .set(Some(now + std::time::Duration::from_secs(5)));
            }
        } else {
            self.slow.set(0);
        }
    }
}
struct AssistCost {
    budget: std::rc::Rc<AssistBudget>,
    started: std::time::Instant,
    manual: bool,
}
impl Drop for AssistCost {
    fn drop(&mut self) {
        if !self.manual {
            let now = std::time::Instant::now();
            self.budget
                .record(now.saturating_duration_since(self.started), now);
        }
    }
}

type AssistContext = (Option<DatabaseKind>, Option<Uuid>, String);

struct CompletionGate {
    last_input: std::cell::Cell<std::time::Instant>,
    task: std::cell::RefCell<Option<Task<()>>>,
    visible: std::cell::Cell<bool>,
    accepted: std::cell::RefCell<Option<CompletionRequest>>,
    missing_column: std::cell::RefCell<Option<sql_assist::ColumnRequest>>,
    paused: std::cell::Cell<bool>,
    catalog_pending: std::cell::RefCell<Option<CompletionRequest>>,
}

#[derive(Default)]
struct AssistMemo {
    words: sql_assist::WordMemo,
    columns: sql_assist::ColumnMemo,
    tables: std::collections::VecDeque<(
        CompletionScope,
        String,
        Option<String>,
        Vec<turbodbn_services::database::workbench::Object>,
    )>,
}
pub(super) struct SqlCompletion {
    owner: WeakEntity<CellView>,
    serial: std::rc::Rc<std::cell::Cell<u64>>,
    manual_next: std::cell::Cell<bool>,
    automatic_enabled: std::cell::Cell<bool>,
    budget: std::rc::Rc<AssistBudget>,
    tables_only: std::cell::Cell<bool>,
    select_allowed: std::cell::Cell<bool>,
    select_override: std::cell::Cell<Option<bool>>,
    has_tables: std::cell::Cell<bool>,
    expanded_help: std::cell::RefCell<Option<String>>,
    failed_columns: std::cell::RefCell<Option<sql_assist::ColumnRequest>>,
    column_snapshot: std::cell::RefCell<Option<(sql_assist::ColumnRequest, u64)>>,
    columns_only: std::cell::Cell<bool>,
    column_choices: std::cell::RefCell<Vec<sql_assist::ColumnRequest>>,
    forced_column: std::cell::RefCell<Option<(Rope, usize, sql_assist::ColumnRequest)>>,
    table_anchor: std::cell::RefCell<Option<(CompletionScope, usize)>>,
    announce_tables: std::cell::Cell<bool>,
    memo: std::sync::Arc<std::sync::Mutex<AssistMemo>>,
    gate: std::rc::Rc<CompletionGate>,
    window: AnyWindowHandle,
}
impl SqlCompletion {
    pub(super) fn new(owner: WeakEntity<CellView>, window: AnyWindowHandle) -> Self {
        Self {
            owner,
            serial: Default::default(),
            manual_next: Default::default(),
            automatic_enabled: std::cell::Cell::new(true),
            budget: Default::default(),
            tables_only: Default::default(),
            select_allowed: Default::default(),
            select_override: Default::default(),
            has_tables: Default::default(),
            expanded_help: Default::default(),
            failed_columns: Default::default(),
            column_snapshot: Default::default(),
            columns_only: Default::default(),
            column_choices: Default::default(),
            forced_column: Default::default(),
            table_anchor: Default::default(),
            announce_tables: Default::default(),
            memo: Default::default(),
            gate: std::rc::Rc::new(CompletionGate {
                last_input: std::cell::Cell::new(std::time::Instant::now()),
                task: Default::default(),
                visible: Default::default(),
                accepted: Default::default(),
                missing_column: Default::default(),
                paused: Default::default(),
                catalog_pending: Default::default(),
            }),
            window,
        }
    }

    fn accepts(
        &self,
        request: &CompletionRequest,
        text: &Rope,
        offset: usize,
        cell: &CellView,
        cx: &App,
    ) -> bool {
        let scope = CompletionScope::capture(cell, cx);
        if request.matches(text, offset, &scope) {
            return true;
        }
        if !request.matches_resume(text, offset, &scope) {
            return false;
        }
        // Unrelated directory updates must not invalidate a still-identical column snapshot.
        let snapshot = self.column_snapshot.borrow();
        let Some((column, fingerprint)) = snapshot.as_ref() else {
            return false;
        };
        let columns = if let Some(names) = &column.local_columns {
            names
                .iter()
                .map(|name| turbodbn_services::database::workbench::Column {
                    name: name.clone(),
                    ..Default::default()
                })
                .collect()
        } else {
            let Some(id) = scope.context.1 else {
                return false;
            };
            let db = cell.connections.read(cx);
            let Some(object) = db.note_column_object(id, &scope.context.2, &column.table) else {
                return false;
            };
            let Some(columns) = db.cached_note_columns(id, &scope.context.2, &object) else {
                return false;
            };
            columns
        };
        column_fingerprint(&columns) == *fingerprint
    }
    pub(super) fn cancel(&self) {
        self.tables_only.set(false);
        self.select_override.set(None);
        self.has_tables.set(false);
        self.columns_only.set(false);
        self.column_snapshot.borrow_mut().take();
        self.forced_column.borrow_mut().take();
        self.column_choices.borrow_mut().clear();
        self.table_anchor.borrow_mut().take();
        self.announce_tables.set(false);
        self.serial.set(self.serial.get().wrapping_add(1));
        self.gate.task.borrow_mut().take();
        self.gate.visible.set(false);
        self.gate.accepted.borrow_mut().take();
        self.gate.paused.set(true);
    }

    pub(super) fn set_automatic(&self, enabled: bool) {
        self.cancel();
        self.automatic_enabled.set(enabled);
    }
    pub(super) fn typing(&self) -> bool {
        self.gate.task.borrow().is_some()
    }
    pub(super) fn can_refresh(&self) -> bool {
        !self.gate.paused.get() && !self.typing()
    }

    fn schedule(&self, cx: &mut Context<InputState>) {
        self.gate.paused.set(false);
        self.serial.set(self.serial.get().wrapping_add(1));
        self.gate.last_input.set(std::time::Instant::now());
        self.gate.accepted.borrow_mut().take();
        if self.gate.visible.replace(false) {
            let input = cx.entity().downgrade();
            let window = self.window;
            cx.defer(move |cx| {
                let _ = window.update(cx, |_, window, cx| {
                    let _ = input.update(cx, |input, cx| {
                        input.handle_action_for_context_menu(Box::new(Escape), window, cx);
                    });
                });
            });
        }
        let manual_session = self.tables_only.get() || self.columns_only.get();
        if manual_session {
            self.manual_next.set(true);
        }
        if !manual_session
            && (!self.automatic_enabled.get() || !self.budget.allowed(std::time::Instant::now()))
            || self.typing()
        {
            return;
        }
        let gate = std::rc::Rc::downgrade(&self.gate);
        let owner = self.owner.clone();
        let window = self.window;
        *self.gate.task.borrow_mut() = Some(cx.spawn(async move |_, cx| {
            loop {
                let Some(state) = gate.upgrade() else { return };
                let remaining =
                    COMPLETION_DEBOUNCE.saturating_sub(state.last_input.get().elapsed());
                drop(state);
                if remaining.is_zero() {
                    break;
                }
                gpui::Timer::after(remaining).await;
            }
            let Some(state) = gate.upgrade() else { return };
            state.task.borrow_mut().take();
            let _ = window.update(cx, |_, window, cx| {
                let _ = owner.update(cx, |cell, cx| {
                    if cell.editing && cell.note_active {
                        cell.trigger_cached_completion(window, cx);
                    }
                });
            });
        }));
    }
}

#[derive(Clone, PartialEq, Eq)]
struct CompletionScope {
    context: AssistContext,
    generation: u64,
    catalog: (u64, u64),
    connected: bool,
}
impl CompletionScope {
    fn capture(cell: &CellView, cx: &App) -> Self {
        let context = cell.assist_context(cx);
        let generation = context
            .1
            .map_or(0, |id| cell.connections.read(cx).note_generation(id));
        let connected = context
            .1
            .is_some_and(|id| cell.connections.read(cx).note_connection_state(id) == Some(true));
        Self {
            context,
            generation,
            catalog: cell.connections.read(cx).note_completion_revision(),
            connected,
        }
    }
}

#[derive(Clone)]
struct CompletionRequest {
    text: Rope,
    offset: usize,
    scope: CompletionScope,
    serial: std::rc::Rc<std::cell::Cell<u64>>,
    ticket: u64,
}
impl CompletionRequest {
    fn matches(&self, text: &Rope, offset: usize, scope: &CompletionScope) -> bool {
        self.serial.get() == self.ticket
            && offset == self.offset
            && scope == &self.scope
            && text == &self.text
    }

    // A completed metadata request must re-read the newest cache rather than requiring
    // the old directory revision. Text, caret, target, connection epoch and serial stay strict.
    fn matches_resume(&self, text: &Rope, offset: usize, scope: &CompletionScope) -> bool {
        let mut scope = scope.clone();
        scope.catalog = self.scope.catalog;
        self.matches(text, offset, &scope)
    }

    fn current(
        &self,
        owner: &WeakEntity<CellView>,
        editor: &WeakEntity<InputState>,
        cx: &mut AsyncWindowContext,
    ) -> bool {
        editor
            .update_in(cx, |input, window, cx| {
                input.focus_handle(cx).is_focused(window)
                    && input.marked_text_range(window, cx).is_none()
                    && owner.upgrade().is_some_and(|cell| {
                        let cell = cell.read(cx);
                        cell.editing
                            && cell.note_active
                            && self.matches(
                                input.text(),
                                input.cursor(),
                                &CompletionScope::capture(cell, cx),
                            )
                    })
            })
            .unwrap_or(false)
    }
}

impl CompletionProvider for SqlCompletion {
    fn completion_leading(&self, item: &CompletionItem, cx: &App) -> Option<AnyElement> {
        if item.kind != Some(lsp_types::CompletionItemKind::FIELD) {
            return None;
        }
        let data = item.data.as_ref();
        let ty = data
            .and_then(|d| d.get("column_type"))
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| {
                item.detail
                    .as_deref()
                    .unwrap_or_default()
                    .split(" · ")
                    .next()
                    .unwrap_or_default()
            });
        let column = turbodbn_services::database::workbench::Column {
            data_type: ty.into(),
            primary: data
                .and_then(|d| d.get("column_primary"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            ..Default::default()
        };
        Some(
            div()
                .w(px(24.))
                .flex_shrink_0()
                .h_flex()
                .justify_center()
                .child(crate::table_view::column_icon(Some(&column), cx))
                .into_any_element(),
        )
    }
    fn completion_documentation(
        &self,
        item: &CompletionItem,
        _: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        if item.kind != Some(lsp_types::CompletionItemKind::FUNCTION) {
            return None;
        }
        let request = self.gate.accepted.borrow().clone()?;
        let help = function_help::lookup(request.scope.context.0, &item.label)?;
        let call = help.call.replacen(
            help.call.split('(').next().unwrap_or_default(),
            &item.label,
            1,
        );
        let owner = self.owner.clone();
        let selected = item.clone();
        let mut card = div()
            .v_flex()
            .gap_1()
            .py_1()
            .child(div().text_sm().whitespace_normal().child(call))
            .child(div().whitespace_normal().child(t(cx, help.summary)))
            .children(t(cx, help.parameters).lines().map(|line| {
                div()
                    .text_color(cx.theme().muted_foreground)
                    .whitespace_normal()
                    .child(line.to_owned())
            }))
            .child(
                div()
                    .mt_1()
                    .text_color(cx.theme().muted_foreground)
                    .child(t(cx, "sql_function.example")),
            )
            .child(div().whitespace_normal().child(help.example));
        let expanded = self.expanded_help.borrow().as_deref() == Some(item.label.as_str());
        if expanded && let Some(more) = help.more {
            card = card.child(div().whitespace_normal().child(more));
        }
        let mut actions = div().h_flex().gap_1().child(
            Button::new("insert-function-call")
                .small()
                .ghost()
                .label(t(cx, "sql_function.insert"))
                .on_click(move |_, window, cx| {
                    let owner = owner.clone();
                    let selected = selected.clone();
                    window.defer(cx, move |window, cx| {
                        let Some((editor, completion)) = owner
                            .read_with(cx, |cell, _| (cell.editor.clone(), cell.completion.clone()))
                            .ok()
                        else {
                            return;
                        };
                        editor.update(cx, |input, cx| {
                            input.focus(window, cx);
                            completion.completion_selected_with_kind(&selected, true, window, cx);
                            input.handle_action_for_context_menu(Box::new(Escape), window, cx);
                        });
                    });
                }),
        );
        let example = help.example;
        let copy_owner = self.owner.clone();
        actions = actions.child(
            Button::new("copy-function-example")
                .small()
                .ghost()
                .icon(IconName::Copy)
                .tooltip(t(cx, "sql_function.copy_example"))
                .on_click(move |_, window, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(example.to_owned()));
                    feedback::success(t(cx, "feedback.copied"), cx);
                    let owner = copy_owner.clone();
                    window.defer(cx, move |window, cx| {
                        if let Ok(Some(editor)) = owner.read_with(cx, |cell, _| {
                            (cell.editing && cell.note_active).then(|| cell.editor.clone())
                        }) {
                            editor.update(cx, |input, cx| input.focus(window, cx));
                        }
                    });
                }),
        );
        if help.more.is_some() {
            let owner = self.owner.clone();
            let name = item.label.clone();
            actions = actions.child(
                Button::new("more-function-examples")
                    .small()
                    .ghost()
                    .label(t(
                        cx,
                        if expanded {
                            "sql_function.less"
                        } else {
                            "sql_function.more"
                        },
                    ))
                    .on_click(move |_, window, cx| {
                        let owner = owner.clone();
                        let name = name.clone();
                        window.defer(cx, move |window, cx| {
                            if let Ok((editor, completion)) = owner.read_with(cx, |cell, _| {
                                (cell.editor.clone(), cell.completion.clone())
                            }) {
                                *completion.expanded_help.borrow_mut() =
                                    if expanded { None } else { Some(name) };
                                editor.update(cx, |input, cx| {
                                    input.focus(window, cx);
                                    input.refresh_completion_menu(cx);
                                });
                            }
                        });
                    }),
            );
        }
        Some(
            card.child(actions)
                .child(
                    div()
                        .whitespace_normal()
                        .text_color(cx.theme().muted_foreground)
                        .child(t(cx, "sql_function.keys")),
                )
                .into_any_element(),
        )
    }
    fn supports_inline_completion(&self) -> bool {
        false
    }

    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        _: CompletionContext,
        window: &mut Window,
        cx: &mut Context<InputState>,
    ) -> Task<anyhow::Result<CompletionResponse>> {
        self.gate.paused.set(false);
        self.gate.task.borrow_mut().take();
        let manual = self.manual_next.replace(false);
        let cost = AssistCost {
            budget: self.budget.clone(),
            started: std::time::Instant::now(),
            manual,
        };
        if !manual && !self.budget.allowed(std::time::Instant::now()) {
            return Task::ready(Ok(CompletionResponse::Array(vec![])));
        }
        let ticket = self.serial.get().wrapping_add(1);
        self.serial.set(ticket);
        let Some(scope) = self
            .owner
            .upgrade()
            .map(|cell| CompletionScope::capture(cell.read(cx), cx))
        else {
            return Task::ready(Ok(CompletionResponse::Array(vec![])));
        };
        if text.len() > sql_assist::MAX_SOURCE {
            return Task::ready(Ok(CompletionResponse::Array(vec![])));
        }
        // Rope clones share storage; no whole-document conversion or parsing on a keystroke.
        let request = CompletionRequest {
            text: text.clone(),
            offset,
            scope,
            ticket,
            serial: self.serial.clone(),
        };
        let owner = self.owner.clone();
        let gate = self.gate.clone();
        if self.tables_only.get() || self.columns_only.get() {
            let word_start = identifier_start(text, offset);
            if self
                .table_anchor
                .borrow()
                .as_ref()
                .is_none_or(|(anchor, start)| anchor != &request.scope || *start != word_start)
            {
                self.tables_only.set(false);
                self.columns_only.set(false);
                self.forced_column.borrow_mut().take();
                self.table_anchor.borrow_mut().take();
            }
        }
        let tables_only = self.tables_only.get();
        let columns_only = self.columns_only.get();
        let forced = self
            .forced_column
            .borrow()
            .clone()
            .filter(|(old, cursor, _)| old == text && *cursor == offset)
            .map(|(_, _, request)| request);
        let announce_tables = self.announce_tables.replace(false);
        let memo = self.memo.clone();
        cx.spawn_in(window, async move |editor, cx| {
            let _cost = cost;
            // Hide earlier candidates before waiting, so stale edit ranges cannot be accepted.
            // Escape preserves the component's pending menu/task; run outside its update stack.
            if request.serial.get() != request.ticket {
                return Ok(CompletionResponse::Array(vec![]));
            }
            editor.update_in(cx, |input, window, cx| {
                input.handle_action_for_context_menu(Box::new(Escape), window, cx);
            })?;
            if !request.current(&owner, &editor, cx) {
                return Ok(CompletionResponse::Array(vec![]));
            }
            let text = request.text.clone();
            let kind = request.scope.context.0;
            let analysis = memo.clone();
            let (source, column, word, choices) = cx
                .background_executor()
                .spawn(async move {
                    let source = text.to_string();
                    let (column, word) = if let Ok(mut memo) = analysis.lock() {
                        let word = memo.words.request(kind, &source, offset);
                        let column = if tables_only {
                            None
                        } else {
                            word.as_ref().and_then(|word| {
                                memo.columns.request_word(kind, &source, offset, word)
                            })
                        };
                        (column, word)
                    } else {
                        (None, None)
                    };
                    let mut column = forced.or(column);
                    let mut choices = vec![];
                    if columns_only
                        && column.is_none()
                        && let Some(word) = &word
                    {
                        choices = sql_assist::column_sources(kind, &source, offset)
                            .into_iter()
                            .map(|table| sql_assist::ColumnRequest {
                                table,
                                local_columns: None,
                                prefix: word.prefix.clone(),
                                range: word.range.clone(),
                            })
                            .collect();
                        if choices.len() == 1 {
                            column = choices.pop();
                        }
                    }
                    (source, column, word, choices)
                })
                .await;
            if !request.current(&owner, &editor, cx) {
                return Ok(CompletionResponse::Array(vec![]));
            }
            let (tables, columns, cache, labels) = owner.update_in(cx, |cell, _, cx| {
                let (_, id, database) = &request.scope.context;
                let db = cell.connections.read(cx);
                let columns = column
                    .as_ref()
                    .and_then(|column| {
                        if let Some(names) = &column.local_columns {
                            return Some(
                                names
                                    .iter()
                                    .map(|name| turbodbn_services::database::workbench::Column {
                                        name: name.clone(),
                                        ..Default::default()
                                    })
                                    .collect(),
                            );
                        }
                        let id = (*id)?;
                        let object = db.note_column_object(id, database, &column.table)?;
                        db.cached_note_columns(id, database, &object)
                    })
                    .unwrap_or_default();
                let tables = if column.is_none() && word.is_some() {
                    id.map(|id| {
                        db.note_table_prefix(
                            id,
                            database,
                            word.as_ref().map_or("", |w| w.prefix.as_str()),
                        )
                    })
                    .unwrap_or_default()
                } else {
                    vec![]
                };
                let cache = if column.is_none() && word.is_some() {
                    id.and_then(|id| db.note_completion_cache(id, database))
                } else {
                    None
                };
                let labels = [
                    t(cx, "sql_assist.keyword"),
                    t(cx, "sql_assist.function"),
                    t(cx, "sql_assist.table"),
                    t(cx, "sql_assist.query"),
                    t(cx, "sql_assist.command"),
                    t(cx, "sql_assist.column"),
                ]
                .map(|label| label.to_string());
                (tables, columns, cache, labels)
            })?;
            owner.update_in(cx, |cell, _, _| {
                *cell.completion.column_snapshot.borrow_mut() = column
                    .clone()
                    .filter(|_| !columns.is_empty())
                    .map(|column| (column, column_fingerprint(&columns)));
            })?;
            let text = request.text.clone();
            let scope = request.scope.clone();
            let resolved_column = column.is_some();
            let missing_column = column.clone().filter(|_| columns.is_empty());
            let (mut items, missing, select_allowed) = cx
                .background_executor()
                .spawn(async move {
                    let mut tables = tables;
                    let mut missing = false;
                    if column.is_none()
                        && let Some(word) = &word
                    {
                        let cached = memo.lock().ok().and_then(|memo| {
                            memo.tables
                                .iter()
                                .find(|(old, prefix, schema, _)| {
                                    old == &scope
                                        && prefix == &word.prefix
                                        && schema == &word.schema
                                })
                                .map(|(_, _, _, tables)| tables.clone())
                        });
                        if let Some(cached) = cached {
                            tables = cached;
                        } else {
                            let disk = cache.as_ref().and_then(|cache| {
                                cache
                                    .table_prefix(&word.prefix, word.schema.as_deref(), 8)
                                    .ok()
                            });
                            let publish = disk.is_some();
                            if let Some(disk) = disk {
                                tables.extend(disk);
                            }
                            tables.sort_by(|a, b| (&a.schema, &a.name).cmp(&(&b.schema, &b.name)));
                            tables.dedup_by(|a, b| a.schema == b.schema && a.name == b.name);
                            if publish && let Ok(mut memo) = memo.lock() {
                                memo.tables.push_back((
                                    scope,
                                    word.prefix.clone(),
                                    word.schema.clone(),
                                    tables.clone(),
                                ));
                                while memo.tables.len() > 32 {
                                    memo.tables.pop_front();
                                }
                            }
                        }
                        missing = tables.is_empty();
                    }
                    let names = tables
                        .iter()
                        .map(|table| sql_assist::TableName {
                            schema: table.schema.clone(),
                            name: table.name.clone(),
                        })
                        .collect::<Vec<_>>();
                    let mut suggestions = if tables_only {
                        word.as_ref()
                            .map(|word| sql_assist::table_suggestions(kind, word, &names, 8))
                            .unwrap_or_default()
                    } else if let Some(column) = column {
                        sql_assist::column_suggestions(kind, &column, &columns)
                            .into_iter()
                            .take(8)
                            .collect()
                    } else if columns_only {
                        vec![]
                    } else {
                        sql_assist::complete(kind, &source, offset, &names)
                    };
                    for suggestion in &mut suggestions {
                        if suggestion.kind == SuggestionKind::Table
                            && let Some(table) = tables.iter().find(|table| {
                                suggestion.label
                                    == if table.schema.is_empty() {
                                        table.name.clone()
                                    } else {
                                        format!("{}.{}", table.schema, table.name)
                                    }
                            })
                        {
                            suggestion.signature = table.comment.clone();
                        }
                    }
                    let allowed = tables_only
                        && word.as_ref().is_some_and(|word| {
                            sql_assist::table_select_allowed(kind, &source, word)
                        });
                    let mut items = completion_items(&text, suggestions, &labels);
                    for item in &mut items {
                        if item.kind==Some(lsp_types::CompletionItemKind::FIELD)
                            && let Some(column)=columns.iter().find(|column|column.name==item.label) {
                            item.data=Some(serde_json::json!({"column_type":column.data_type,"column_primary":column.primary}));
                        }
                    }

                    if tables_only {
                        for item in &mut items {
                            if let Some(CompletionTextEdit::Edit(edit)) = &item.text_edit {
                                let sql = word.as_ref().filter(|_| allowed).and_then(|word| {
                                    sql_assist::table_select_text(
                                        kind,
                                        &source,
                                        word,
                                        &edit.new_text,
                                    )
                                });
                                item.data = Some(serde_json::json!({"table_select": sql}));
                            }
                        }
                    }
                    (items, missing, allowed)
                })
                .await;
            if !request.current(&owner, &editor, cx) {
                return Ok(CompletionResponse::Array(vec![]));
            }
            owner.update_in(cx, |cell, _, _| {
                cell.completion.select_allowed.set(select_allowed);
                cell.completion
                    .has_tables
                    .set(tables_only && !items.is_empty());
            })?;
            if !choices.is_empty() {
                for (index, choice) in choices.iter().enumerate() {
                    items.push(CompletionItem {
                        label: choice.table.name.clone(),
                        data: Some(serde_json::Value::String(format!("choose-columns:{index}"))),
                        ..Default::default()
                    });
                }
            }
            if columns_only && !resolved_column && items.is_empty() {
                editor.update_in(cx, |_, _, cx| {
                    feedback::error(t(cx, "sql_assist.column_context"), cx)
                })?;
            }
            if announce_tables && items.is_empty() && !missing {
                editor.update_in(cx, |_, _, cx| {
                    feedback::error(
                        t(
                            cx,
                            if !request.scope.connected {
                                "sql_tables.connect_first"
                            } else if missing {
                                "sql_tables.no_cached_match"
                            } else {
                                "sql_tables.in_code"
                            },
                        ),
                        cx,
                    )
                })?;
            }
            if items.is_empty()
                && let Some(column) = missing_column
            {
                let item = owner.update_in(cx, |cell, _, cx| CompletionItem {
                    label: if !request.scope.connected && cell.column_loading.is_none() {
                        t(cx, "sql_assist.connect_load").to_string()
                    } else if cell.column_loading.is_some() {
                        t(cx, "sql_assist.loading_columns").to_string()
                    } else {
                        tr(
                            cx,
                            if cell.completion.failed_columns.borrow().as_ref() == Some(&column) {
                                "sql_assist.retry_columns"
                            } else {
                                "sql_assist.load_columns"
                            },
                            &[("table", &column.table.name)],
                        )
                        .to_string()
                    },
                    detail: None,
                    data: Some(serde_json::Value::String("load-columns".into())),
                    ..Default::default()
                })?;
                items.push(item);
                *gate.missing_column.borrow_mut() = Some(column);
            } else {
                gate.missing_column.borrow_mut().take();
            }
            if missing && !columns_only && (tables_only || manual) {
                items.push(owner.update_in(cx, |cell, _, cx| {
                    CompletionItem {
                        label: t(
                            cx,
                            if cell.column_loading.is_some()
                                || cell.completion.gate.catalog_pending.borrow().is_some()
                            {
                                "sql_assist.loading_tables"
                            } else if !request.scope.connected {
                                "sql_assist.connect_load"
                            } else {
                                "sql_assist.load_tables"
                            },
                        )
                        .to_string(),
                        data: Some(serde_json::Value::String("load-tables".into())),
                        ..Default::default()
                    }
                })?);
            }
            owner.update_in(cx, |cell, _, cx| {
                for item in &mut items {
                    if item.kind == Some(lsp_types::CompletionItemKind::KEYWORD)
                        && let Some(key) = function_help::keyword_summary(&item.label) {
                        item.detail = Some(t(cx,key).to_string());
                    }
                    if item.kind==Some(lsp_types::CompletionItemKind::FUNCTION) && let Some(help)=function_help::lookup(request.scope.context.0,&item.label) {item.detail=Some(t(cx,help.summary).to_string());item.documentation=None;}
                }
                *cell.completion.column_choices.borrow_mut() = choices;
            })?;
            gate.visible.set(!items.is_empty());
            *gate.accepted.borrow_mut() = Some(request);
            Ok(CompletionResponse::Array(items))
        })
    }
    fn completion_cancelled(&self) {
        self.serial.set(self.serial.get().wrapping_add(1));
        self.gate.accepted.borrow_mut().take();
        self.gate.visible.set(false);
        self.gate.paused.set(true);
    }
    fn completion_loading_label(&self, item: &CompletionItem, cx: &App) -> Option<String> {
        match item.data.as_ref().and_then(|v| v.as_str()) {
            Some("load-columns") => Some(t(cx, "sql_assist.loading_columns").to_string()),
            Some("load-tables") => Some(t(cx, "sql_assist.loading_tables").to_string()),
            _ => None,
        }
    }
    fn completion_selected_with_kind(
        &self,
        item: &CompletionItem,
        tab: bool,
        window: &mut Window,
        cx: &mut Context<InputState>,
    ) -> bool {
        let Some(current) = self.gate.accepted.borrow().clone() else {
            return true;
        };
        if !self.owner.upgrade().is_some_and(|cell| {
            self.accepts(&current, &current.text, current.offset, cell.read(cx), cx)
        }) {
            return true;
        }
        if tab && item.kind == Some(lsp_types::CompletionItemKind::FUNCTION) {
            let Some(CompletionTextEdit::Edit(edit)) = &item.text_edit else {
                return true;
            };
            let edit = edit.clone();
            let input = cx.entity();
            let name = item.label.clone();
            let gate = self.gate.clone();
            window.defer(cx, move |window, cx| {
                input.update(cx, |input, cx| {
                    if input.text() != &current.text
                        || input.cursor() != current.offset
                        || current.serial.get() != current.ticket
                        || input.marked_text_range(window, cx).is_some()
                    {
                        return;
                    }
                    let start = input.text().position_to_offset(&edit.range.start);
                    let end = input.text().position_to_offset(&edit.range.end);
                    let (call, parameter) = function_help::insertion(
                        current.scope.context.0,
                        &name,
                        &input.text().slice(end..).to_string(),
                    );
                    let utf16 = input
                        .text()
                        .slice(..start)
                        .chars()
                        .map(char::len_utf16)
                        .sum::<usize>()
                        ..input
                            .text()
                            .slice(..end)
                            .chars()
                            .map(char::len_utf16)
                            .sum::<usize>();
                    input.replace_text_in_range(Some(utf16), &call, window, cx);
                    if let Some(parameter) = parameter {
                        input.select_byte_range(
                            start + parameter.start..start + parameter.end,
                            window,
                            cx,
                        );
                    }
                    gate.task.borrow_mut().take();
                    gate.visible.set(false);
                    input.focus(window, cx);
                });
            });
            return true;
        }
        if let Some(data) = item
            .data
            .as_ref()
            .filter(|v| v.get("table_select").is_some())
        {
            if !tab || !self.select_allowed.get() || !self.select_override.get().unwrap_or(true) {
                return false;
            }
            let Some(sql) = data.get("table_select").and_then(|v| v.as_str()) else {
                return false;
            };
            let request = current;
            let Some(CompletionTextEdit::Edit(edit)) = &item.text_edit else {
                return true;
            };
            let edit = edit.clone();
            let sql = sql.to_owned();
            let input = cx.entity();
            window.defer(cx, move |window, cx| {
                input.update(cx, |input, cx| {
                    if input.text() != &request.text
                        || input.cursor() != request.offset
                        || request.serial.get() != request.ticket
                        || input.marked_text_range(window, cx).is_some()
                    {
                        return;
                    }
                    let start = input.text().position_to_offset(&edit.range.start);
                    let end = input.text().position_to_offset(&edit.range.end);
                    let range = input
                        .text()
                        .slice(..start)
                        .chars()
                        .map(char::len_utf16)
                        .sum::<usize>()
                        ..input
                            .text()
                            .slice(..end)
                            .chars()
                            .map(char::len_utf16)
                            .sum::<usize>();
                    input.replace_text_in_range(Some(range), &sql, window, cx);
                    input.focus(window, cx);
                })
            });
            return true;
        }
        self.completion_selected(item, window, cx)
    }
    fn completion_footer(&self, _: &mut Window, cx: &mut App) -> Option<AnyElement> {
        if !self.tables_only.get() || !self.has_tables.get() {
            return None;
        }
        let allowed = self.select_allowed.get();
        let selected = allowed && self.select_override.get().unwrap_or(true);
        let mut row = div()
            .h_flex()
            .items_center()
            .gap_1()
            .text_xs()
            .child("Tab:");
        for (sql, key) in [
            (false, "sql_assist.insert_name"),
            (true, "sql_assist.insert_select"),
        ] {
            let owner = self.owner.clone();
            row = row.child(
                Button::new(if sql {
                    "completion-select"
                } else {
                    "completion-name"
                })
                .ghost()
                .xsmall()
                .label(t(cx, key))
                .selected(selected == sql)
                .disabled(sql && !allowed)
                .tooltip(t(
                    cx,
                    if sql && !allowed {
                        "sql_assist.select_unavailable"
                    } else {
                        "sql_assist.tab_mode"
                    },
                ))
                .on_click(move |_, window, cx| {
                    let owner = owner.clone();
                    window.defer(cx, move |window, cx| {
                        let _ = owner.update(cx, |cell, cx| {
                            cell.completion.select_override.set(Some(sql));
                            cell.editor.update(cx, |input, cx| {
                                input.focus(window, cx);
                                input.refresh_completion_menu(cx);
                            });
                        });
                    });
                }),
            );
        }
        Some(
            row.child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .child(t(cx, "sql_assist.enter_name")),
            )
            .into_any_element(),
        )
    }
    fn completion_selected(
        &self,
        item: &CompletionItem,
        window: &mut Window,
        cx: &mut Context<InputState>,
    ) -> bool {
        if let Some(index) = item
            .data
            .as_ref()
            .and_then(|v| v.as_str())
            .and_then(|s| s.strip_prefix("choose-columns:"))
            .and_then(|s| s.parse::<usize>().ok())
        {
            let column = self.column_choices.borrow().get(index).cloned();
            let request = self.gate.accepted.borrow().clone();
            let owner = self.owner.clone();
            window.defer(cx, move |window, cx| {
                if let (Some(column), Some(request)) = (column, request) {
                    let _ = owner.update(cx, |cell, cx| {
                        if request.matches(
                            cell.editor.read(cx).text(),
                            cell.editor.read(cx).cursor(),
                            &CompletionScope::capture(cell, cx),
                        ) {
                            *cell.completion.forced_column.borrow_mut() =
                                Some((request.text, request.offset, column));
                            cell.completion.manual_next.set(true);
                            cell.trigger_cached_completion(window, cx);
                        }
                    });
                }
            });
            return true;
        }
        if item.data.as_ref().and_then(|v| v.as_str()) == Some("load-tables") {
            let request = self.gate.accepted.borrow().clone();
            let owner = self.owner.clone();
            window.defer(cx, move |window, cx| {
                if let Some(request) = request {
                    let _ = owner.update(cx, |cell, cx| {
                        let scope = CompletionScope::capture(cell, cx);
                        if request.matches(
                            cell.editor.read(cx).text(),
                            cell.editor.read(cx).cursor(),
                            &scope,
                        ) && cell.completion.gate.catalog_pending.borrow().is_none()
                            && scope.context.1.is_some()
                        {
                            cell.load_assist_metadata(None, request, window, cx);
                        }
                    });
                }
            });
            return true;
        }
        if item.data.as_ref().and_then(|v| v.as_str()) != Some("load-columns") {
            return false;
        }
        let column = self.gate.missing_column.borrow().clone();
        let request = self.gate.accepted.borrow().clone();
        let owner = self.owner.clone();
        window.defer(cx, move |window, cx| {
            if let (Some(column), Some(request)) = (column, request) {
                let _ = owner.update(cx, |cell, cx| {
                    let scope = CompletionScope::capture(cell, cx);
                    if request.matches(
                        cell.editor.read(cx).text(),
                        cell.editor.read(cx).cursor(),
                        &scope,
                    ) {
                        cell.load_assist_metadata(Some(column), request, window, cx);
                    }
                });
            }
        });
        true
    }
    fn is_completion_trigger(&self, _: usize, _: &str, cx: &mut Context<InputState>) -> bool {
        self.schedule(cx);
        false
    }
}

fn column_fingerprint(columns: &[turbodbn_services::database::workbench::Column]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    for column in columns {
        (&column.name, &column.data_type, &column.comment).hash(&mut hash);
    }
    hash.finish()
}

fn completion_items(
    text: &Rope,
    suggestions: Vec<sql_assist::Suggestion>,
    labels: &[String; 6],
) -> Vec<CompletionItem> {
    suggestions
        .into_iter()
        .map(|s| {
            let category = match s.kind {
                SuggestionKind::Keyword => 0,
                SuggestionKind::Function => 1,
                SuggestionKind::Table => 2,
                SuggestionKind::Query => 3,
                SuggestionKind::Command => 4,
                SuggestionKind::Column => 5,
            };
            let detail = if matches!(s.kind, SuggestionKind::Column | SuggestionKind::Table)
                && !s.signature.is_empty()
            {
                s.signature
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(240)
                    .collect()
            } else {
                labels[category].clone()
            };
            let doc = if s.signature.is_empty() || s.kind == SuggestionKind::Query {
                s.insert.clone()
            } else {
                format!("{}\n{}", s.label, s.signature)
                    .chars()
                    .take(4096)
                    .collect()
            };
            let documentation = if s.kind == SuggestionKind::Keyword
                || matches!(s.kind, SuggestionKind::Column | SuggestionKind::Table)
                    && s.signature.chars().count() <= 240
            {
                None
            } else {
                Some(Documentation::String(doc))
            };
            CompletionItem {
                label: s.label.clone(),
                kind: match s.kind {
                    SuggestionKind::Column => Some(lsp_types::CompletionItemKind::FIELD),
                    SuggestionKind::Function => Some(lsp_types::CompletionItemKind::FUNCTION),
                    SuggestionKind::Keyword => Some(lsp_types::CompletionItemKind::KEYWORD),
                    _ => None,
                },
                detail: Some(detail),
                documentation,
                // Component highlights byte ranges, so do not reuse an unrelated trigger query.
                filter_text: Some(String::new()),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range: lsp_types::Range::new(
                        text.offset_to_position(s.range.start),
                        text.offset_to_position(s.range.end),
                    ),
                    new_text: if s.kind == SuggestionKind::Function {
                        s.label.clone()
                    } else {
                        s.insert
                    },
                })),
                ..Default::default()
            }
        })
        .collect()
}

impl CellView {
    pub(super) fn publish_ddl_context(&self, open: bool, cx: &mut Context<Self>) {
        if !self.note_active {
            return;
        }
        let ticket = self.ddl_epoch.get().wrapping_add(1);
        self.ddl_epoch.set(ticket);
        let (_, id, database) = self.assist_context(cx);
        let scope = if self.editing {
            id.filter(|_| !database.is_empty()).map(|id| (id, database))
        } else {
            None
        };
        let origin = if self.editing && scope.is_none() {
            None
        } else {
            Some(self.cell.id)
        };
        if !open {
            feedback::ddl_context(
                feedback::DdlContext {
                    note: None,
                    origin,
                    scope,
                    object: None,
                    candidates: vec![],
                    open,
                },
                cx,
            );
            return;
        }
        let request = feedback::ddl_request(cx);
        let revision = self.revision;
        let text = self.editor.read(cx).text().clone();
        let offset = self.editor.read(cx).cursor();
        let selection = self.editor.read(cx).selection_byte_range();
        let has_selection = !selection.is_empty();
        let kind = self.assist_context(cx).0;
        let original_scope = self.assist_context(cx);
        let epoch = id.map(|id| self.connections.read(cx).note_generation(id));
        cx.spawn(async move |cell, cx| {
            let source = text.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    sql_assist::ddl_target(kind, &source.to_string(), selection, offset)
                })
                .await;
            let _ = cell.update(cx, |this, cx| {
                if !feedback::ddl_request_current(request, cx)
                    || !this.editing
                    || !this.note_active
                    || this.revision != revision
                    || this.ddl_epoch.get() != ticket
                    || this.assist_context(cx) != original_scope
                    || this.editor.read(cx).text() != &text
                    || id.map(|id| this.connections.read(cx).note_generation(id)) != epoch
                {
                    return;
                }
                let target = |table: &sql_assist::DdlTable| {
                    let (Some(kind), Some(id), database) = this.assist_context(cx) else {
                        return None;
                    };
                    let db = if matches!(kind, DatabaseKind::MySql | DatabaseKind::ClickHouse)
                        && !table.table.schema.is_empty()
                    {
                        table.table.schema.clone()
                    } else {
                        database
                    };
                    let object =
                        this.connections
                            .read(cx)
                            .note_column_object(id, &db, &table.table)?;
                    Some(feedback::NoteDdlTable {
                        database: db,
                        object,
                        line: table.line,
                    })
                };
                let tables = result.tables.iter().filter_map(target).collect::<Vec<_>>();
                let fallback = result.fallback.iter().filter_map(target).collect();
                let selected = result
                    .selected
                    .as_ref()
                    .and_then(|name| result.tables.iter().find(|t| &t.table == name))
                    .and_then(target);
                let selected_scope = selected
                    .as_ref()
                    .and_then(|target| id.map(|id| (id, target.database.clone())))
                    .or(scope);
                feedback::ddl_context(
                    feedback::DdlContext {
                        note: Some(feedback::NoteDdlContext {
                            selection: has_selection,
                            tables,
                            fallback,
                            recognized: result.recognized,
                        }),
                        origin,
                        scope: selected_scope,
                        object: selected.map(|t| t.object),
                        candidates: vec![],
                        open,
                    },
                    cx,
                );
            });
        })
        .detach();
    }

    pub(super) fn auto_connect(&self, cx: &mut Context<Self>) {
        if !self.cell.content.is_editable_sql() || !self.editing || !self.note_active {
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

    fn load_assist_metadata(
        &mut self,
        column: Option<sql_assist::ColumnRequest>,
        mut request: CompletionRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.column_loading.is_some() {
            return;
        }
        let Some(id) = request.scope.context.1 else {
            return;
        };
        let profile = self.connections.read(cx).note_profile(id).cloned();
        if !self
            .connections
            .read(cx)
            .note_database_visible(id, &request.scope.context.2)
        {
            feedback::error(t(cx, "sql_tables.no_cached_match"), cx);
            self.completion.manual_next.set(true);
            self.trigger_cached_completion(window, cx);
            return;
        }
        if request.scope.connected {
            if let Some(column) = column {
                self.load_column_hint(column, request, window, cx);
            } else {
                *self.completion.gate.catalog_pending.borrow_mut() = Some(request.clone());
                self.connections.update(cx, |db, cx| {
                    db.load_note_catalog(id, request.scope.context.2, cx)
                });
            }
            return;
        }
        self.connections
            .update(cx, |db, cx| db.ensure_note_connection(id, true, cx));
        let started = std::time::Instant::now();
        self.column_loading = Some(cx.spawn_in(window, async move |cell, cx| {
            loop {
                gpui::Timer::after(std::time::Duration::from_millis(100)).await;
                let state = cell
                    .update_in(cx, |this, window, cx| {
                        let scope = CompletionScope::capture(this, cx);
                        let valid = request.serial.get() == request.ticket
                            && this.editing
                            && this.note_active
                            && this.editor.read(cx).text() == &request.text
                            && this.editor.read(cx).cursor() == request.offset
                            && scope.context == request.scope.context
                            && this.connections.read(cx).note_profile(id).cloned() == profile;
                        if !valid {
                            this.column_loading = None;
                            return false;
                        }
                        let state = this.connections.read(cx).note_connection_state(id);
                        if state == Some(true) {
                            this.column_loading = None;
                            request.scope = scope.clone();
                            *this.completion.gate.accepted.borrow_mut() = Some(request.clone());
                            if this.completion.tables_only.get()
                                || this.completion.columns_only.get()
                            {
                                *this.completion.table_anchor.borrow_mut() =
                                    Some((scope, identifier_start(&request.text, request.offset)));
                            }
                            if let Some(column) = column.clone() {
                                this.load_column_hint(column, request.clone(), window, cx);
                            } else {
                                *this.completion.gate.catalog_pending.borrow_mut() =
                                    Some(request.clone());
                                this.connections.update(cx, |db, cx| {
                                    db.load_note_catalog(id, request.scope.context.2.clone(), cx)
                                });
                            }
                            return false;
                        }
                        if started.elapsed().as_secs() >= 60
                            || (started.elapsed().as_secs() >= 1 && state == Some(false))
                        {
                            this.column_loading = None;
                            *this.completion.failed_columns.borrow_mut() = column.clone();
                            feedback::error(t(cx, "sql_assist.connect_retry"), cx);
                            this.completion.manual_next.set(true);
                            this.trigger_cached_completion(window, cx);
                            return false;
                        }
                        true
                    })
                    .unwrap_or(false);
                if !state {
                    break;
                }
            }
        }));
    }
    fn load_column_hint(
        &mut self,
        column: sql_assist::ColumnRequest,
        request: CompletionRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.column_loading.is_some() {
            return;
        }
        let (_, Some(id), database) = self.assist_context(cx) else {
            return;
        };
        let Some(object) =
            self.connections
                .read(cx)
                .note_column_object(id, &database, &column.table)
        else {
            *self.completion.failed_columns.borrow_mut() = Some(column);
            self.completion.manual_next.set(true);
            self.trigger_cached_completion(window, cx);
            return;
        };
        let generation = self.connections.read(cx).note_generation(id);
        if self.connections.read(cx).note_connection_state(id) != Some(true) {
            feedback::error(t(cx, "sql_tables.connect_first"), cx);
            return;
        }
        self.completion.failed_columns.borrow_mut().take();
        let receiver = self.connections.update(cx, |db, cx| {
            db.mcp_request(
                id,
                generation,
                database.clone(),
                Action::NoteColumns(object.clone()),
                cx,
            )
        });
        self.column_loading = Some(cx.spawn_in(window, async move |cell, cx| {
            let reply = receiver.recv().await;
            let _ = cell.update_in(cx, |this, window, cx| {
                this.column_loading = None;
                if this.connections.read(cx).note_generation(id) != generation
                    || this.connections.read(cx).note_connection_state(id) != Some(true)
                {
                    return;
                }
                match reply {
                    Ok(Ok(Reply::Schema(schema))) => {
                        let origin = cx.entity_id();
                        this.connections.update(cx, |db, cx| {
                            db.remember_note_columns(id, &database, &object, &schema.columns);
                            cx.emit(crate::database::NoteColumnsReady(id, Some(origin)));
                        });
                        let scope = CompletionScope::capture(this, cx);
                        if request.matches_resume(
                            this.editor.read(cx).text(),
                            this.editor.read(cx).cursor(),
                            &scope,
                        ) {
                            this.completion.manual_next.set(true);
                            this.trigger_cached_completion(window, cx);
                        }
                    }
                    _ => {
                        *this.completion.failed_columns.borrow_mut() = Some(column);
                        feedback::error(t(cx, "sql_assist.columns_failed"), cx);
                        if request.matches_resume(
                            this.editor.read(cx).text(),
                            this.editor.read(cx).cursor(),
                            &CompletionScope::capture(this, cx),
                        ) {
                            this.completion.manual_next.set(true);
                            this.trigger_cached_completion(window, cx);
                        }
                    }
                }
            });
        }));
    }
    pub(super) fn trigger_table_completion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.editing || !self.note_active || self.working(cx) || self.batch_locked {
            return;
        }
        let editor = self.editor.clone();
        let completion = self.completion.clone();
        self.completion.cancel();
        self.completion.manual_next.set(true);
        let scope = CompletionScope::capture(self, cx);
        window.defer(cx, move |window, cx| {
            editor.update(cx, |input, cx| {
                if input.marked_text_range(window, cx).is_some() {
                    return;
                }
                input.focus(window, cx);
                let cursor = input.cursor();
                let start = identifier_start(input.text(), cursor);
                *completion.table_anchor.borrow_mut() = Some((scope, start));
                completion.announce_tables.set(true);
                completion.tables_only.set(true);
                input.request_completions(window, cx);
            })
        });
    }
    pub(super) fn catalog_ready(
        &mut self,
        event: &crate::database::NoteCatalogReady,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(request) = self.completion.gate.catalog_pending.borrow_mut().take() else {
            return;
        };
        if request.scope.context.1 != Some(event.0)
            || request.scope.context.2 != event.1
            || request.scope.generation != event.2
        {
            *self.completion.gate.catalog_pending.borrow_mut() = Some(request);
            return;
        }
        let scope = CompletionScope::capture(self, cx);
        if scope.context != request.scope.context
            || scope.generation != request.scope.generation
            || !scope.connected
        {
            return;
        }
        if !event.3 {
            feedback::error(t(cx, "sql_assist.columns_failed"), cx);
            if request.serial.get() == request.ticket
                && self.editor.read(cx).text() == &request.text
                && self.editor.read(cx).cursor() == request.offset
            {
                self.completion.manual_next.set(true);
                self.trigger_cached_completion(window, cx);
            }
            return;
        }
        if request.serial.get() == request.ticket
            && self.editor.read(cx).text() == &request.text
            && self.editor.read(cx).cursor() == request.offset
            && self.editor.read(cx).focus_handle(cx).is_focused(window)
            && !self.completion.gate.paused.get()
        {
            if self.completion.tables_only.get() {
                let start = identifier_start(self.editor.read(cx).text(), request.offset);
                *self.completion.table_anchor.borrow_mut() = Some((scope, start));
            }
            self.completion.manual_next.set(true);
            self.trigger_cached_completion(window, cx);
        }
    }
    pub(super) fn trigger_column_completion(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.editing || !self.note_active || self.working(cx) || self.batch_locked {
            return;
        }
        self.completion.cancel();
        self.completion.manual_next.set(true);
        self.completion.columns_only.set(true);
        let editor = self.editor.clone();
        let completion = self.completion.clone();
        let scope = CompletionScope::capture(self, cx);
        window.defer(cx, move |window, cx| {
            editor.update(cx, |input, cx| {
                if input.marked_text_range(window, cx).is_none() {
                    input.focus(window, cx);
                    *completion.table_anchor.borrow_mut() =
                        Some((scope, identifier_start(input.text(), input.cursor())));
                    input.request_completions(window, cx);
                }
            });
        });
    }
    pub(super) fn trigger_completion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.completion.tables_only.set(false);
        self.completion.table_anchor.borrow_mut().take();
        self.completion.manual_next.set(true);
        self.trigger_cached_completion(window, cx);
    }
    pub(super) fn trigger_cached_completion(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let editor = self.editor.clone();
        let completion = self.completion.clone();
        let serial = completion.serial.get();
        window.defer(cx, move |window, cx| {
            editor.update(cx, |input, cx| {
                if completion.serial.get() != serial
                    || !input.focus_handle(cx).is_focused(window)
                    || input.marked_text_range(window, cx).is_some()
                {
                    return;
                }
                input.request_completions(window, cx);
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
            || !self.cell.content.is_editable_sql()
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
    pub(super) fn assist_accept(
        &mut self,
        tab: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.editing
            || !self.cell.content.is_editable_sql()
            || !self.editor.read(cx).focus_handle(cx).is_focused(window)
        {
            return false;
        }
        let valid = self
            .completion
            .gate
            .accepted
            .borrow()
            .as_ref()
            .is_some_and(|request| {
                self.completion.accepts(
                    request,
                    self.editor.read(cx).text(),
                    self.editor.read(cx).cursor(),
                    self,
                    cx,
                )
            });
        if !valid {
            self.editor.update(cx, |input, cx| {
                input.handle_action_for_context_menu(Box::new(Escape), window, cx);
            });
            return false;
        }
        self.editor.update(cx, |input, cx| {
            if input.marked_text_range(window, cx).is_some() {
                return false;
            }
            let action: Box<dyn gpui::Action> = if tab {
                Box::new(gpui_component::input::IndentInline)
            } else {
                Box::new(Enter { secondary: false })
            };
            input.handle_action_for_context_menu(action, window, cx)
        })
    }
    pub(super) fn assist_enter(
        &mut self,
        action: &Enter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.editor.update(cx, |input, cx| {
            input.marked_text_range(window, cx).is_some()
        }) {
            cx.propagate();
            return;
        }
        if !action.secondary && self.assist_accept(false, window, cx) {
            cx.stop_propagation();
            return;
        }
        if self.editing
            && self.cell.content.is_editable_sql()
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
        if !self.editor.read(cx).focus_handle(cx).is_focused(window)
            && !self.focus.is_focused(window)
        {
            cx.propagate();
            return;
        }
        if !self.editing {
            cx.propagate();
            return;
        }
        self.completion.cancel();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(source: &str) -> CompletionRequest {
        CompletionRequest {
            text: Rope::from(source),
            offset: source.len(),
            scope: CompletionScope {
                context: (
                    Some(DatabaseKind::Sqlite),
                    Some(Uuid::new_v4()),
                    "main".into(),
                ),
                generation: 4,
                catalog: (0, 0),
                connected: true,
            },
            serial: std::rc::Rc::new(std::cell::Cell::new(1)),
            ticket: 1,
        }
    }

    #[::core::prelude::v1::test]
    fn long_line_highlighting_is_bounded_without_truncating_sql_or_disabling_completion() {
        let exact = "x".repeat(8192);
        assert!(!long_sql_line(exact.bytes()));
        let source = format!("-- {exact}\nSEL");
        assert!(long_sql_line(source.bytes()));
        let reading = sql_reading(&source, DatabaseKind::MySql);
        assert!(reading.starts_with("~~~sql-light\n"));
        assert!(reading.contains(&source));
        assert!(
            sql_assist::complete(Some(DatabaseKind::MySql), &source, source.len(), &[])
                .iter()
                .any(|item| item.label == "SELECT")
        );
        let multiline = (exact.clone() + "\r\n").repeat(4);
        assert!(!long_sql_line(multiline.bytes()));
        assert!(sql_reading(&multiline, DatabaseKind::MySql).starts_with("~~~mysql\n"));
        assert!(long_sql_line(("中".repeat(2731)).bytes()));
        assert!(!long_sql_line(("中".repeat(2730)).bytes()));
        // Rope chunk boundaries must not reset the byte count in a long physical line.
        let rope = Rope::from(source.as_str());
        assert!(long_sql_line(rope.chunks().flat_map(str::bytes)));
    }

    #[::core::prelude::v1::test]
    fn sql_typing_keeps_shell_stable_but_updates_height_and_empty_actions() {
        let shape = |source: &str, limit| editor_shape(&Rope::from(source), limit);
        assert_eq!(shape("SELECT a", 12), shape("SELECT alphabet", 12));
        assert_eq!(shape("", 12), shape(" \t\n", 12));
        assert_ne!(shape(" \t", 12), shape("S", 12));
        assert_ne!(shape("a\nb\nc", 12), shape("a\nb\nc\nd", 12));
        let long = "SELECT 1\n".repeat(30);
        assert_eq!(shape(&long, 6), shape(&(long.clone() + "another line"), 6));
        assert_eq!(shape(&long, 6).0, 6);
        assert_eq!(shape(&long, 12).0, 12);
    }

    #[::core::prelude::v1::test]
    fn completion_rejects_old_edits_cursor_scope_and_reconnected_sessions() {
        let request = pending("SELECT 名称 FROM items WHERE na");
        let scope = request.scope.clone();
        assert!(request.matches(&request.text, request.offset, &scope));
        // An equal-length replacement must not publish the old text edit range.
        let replacement = Rope::from("SELECT 名称 FROM items WHERE id");
        assert!(!request.matches(&replacement, request.offset, &scope));
        assert!(!request.matches(&request.text, request.offset - 1, &scope));
        let mut changed = scope.clone();
        changed.context.2 = "other".into();
        assert!(!request.matches(&request.text, request.offset, &changed));
        changed = scope.clone();
        changed.context.1 = Some(Uuid::new_v4());
        assert!(!request.matches(&request.text, request.offset, &changed));
        changed = scope.clone();
        changed.generation += 1;
        assert!(!request.matches(&request.text, request.offset, &changed));
        let mut refreshed = scope.clone();
        refreshed.catalog.0 += 1;
        assert!(!request.matches(&request.text, request.offset, &refreshed));
        assert!(request.matches_resume(&request.text, request.offset, &refreshed));
        refreshed.generation += 1;
        assert!(!request.matches_resume(&request.text, request.offset, &refreshed));
        assert!(!request.matches_resume(&replacement, request.offset, &scope));
        assert!(!request.matches_resume(&request.text, request.offset - 1, &scope));
        // Even if undo restores identical text/caret/scope, a superseded task stays stale.
        request.serial.set(2);
        assert!(!request.matches(&request.text, request.offset, &scope));
    }

    #[::core::prelude::v1::test]
    fn background_completion_preserves_dialect_unicode_edits_and_source() {
        let source = "-- 😀中文\nSEL";
        let original = Rope::from(source);
        let labels = std::array::from_fn(|i| format!("category {i}"));
        for kind in [
            DatabaseKind::MySql,
            DatabaseKind::PostgreSql,
            DatabaseKind::Sqlite,
        ] {
            let text = original.clone();
            let labels = labels.clone();
            let items = std::thread::spawn(move || {
                let suggestions =
                    sql_assist::complete(Some(kind), &text.to_string(), text.len(), &[]);
                completion_items(&text, suggestions, &labels)
            })
            .join()
            .unwrap();
            let item = items.iter().find(|item| item.label == "SELECT").unwrap();
            let Some(CompletionTextEdit::Edit(edit)) = &item.text_edit else {
                panic!("missing text edit")
            };
            assert_eq!(edit.range.start, lsp_types::Position::new(1, 0));
            assert_eq!(edit.range.end, lsp_types::Position::new(1, 3));
            assert_eq!(edit.new_text, "SELECT");
            assert_eq!(item.detail.as_deref(), Some("category 0"));
        }
        assert_eq!(original.to_string(), source);
        for source in ["SELECT 'SEL", "SELECT 1 -- SEL"] {
            assert!(
                sql_assist::complete(Some(DatabaseKind::Sqlite), source, source.len(), &[])
                    .is_empty()
            );
        }
    }
}

#[cfg(test)]
mod input_first_contract_tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn slow_automatic_assistance_pauses_and_recovers_without_affecting_manual_work() {
        let budget = AssistBudget::default();
        let now = std::time::Instant::now();
        for _ in 0..2 {
            budget.record(std::time::Duration::from_millis(90), now);
            assert!(budget.allowed(now));
        }
        budget.record(std::time::Duration::from_millis(90), now);
        assert!(!budget.allowed(now));
        assert!(budget.allowed(now + std::time::Duration::from_secs(6)));
    }
    #[::core::prelude::v1::test]
    fn comments_and_types_are_display_metadata_not_inserted_sql() {
        let text = Rope::from("u.na");
        let labels = std::array::from_fn::<String, 6, _>(|_| "kind".into());
        let suggestions = vec![sql_assist::Suggestion {
            label: "name".into(),
            insert: "`name`".into(),
            signature: "VARCHAR(64) · 昵称".into(),
            kind: SuggestionKind::Column,
            range: 2..4,
        }];
        let items = completion_items(&text, suggestions, &labels);
        assert_eq!(items[0].detail.as_deref(), Some("VARCHAR(64) · 昵称"));
        let Some(CompletionTextEdit::Edit(edit)) = &items[0].text_edit else {
            panic!("edit missing")
        };
        assert_eq!(edit.new_text, "`name`");
        let items = completion_items(
            &Rope::from("us"),
            vec![sql_assist::Suggestion {
                label: "users".into(),
                insert: "`users`".into(),
                signature: "用户表".into(),
                kind: SuggestionKind::Table,
                range: 0..2,
            }],
            &labels,
        );
        assert_eq!(items[0].detail.as_deref(), Some("用户表"));
    }
}

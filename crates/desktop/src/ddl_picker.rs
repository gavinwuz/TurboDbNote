use crate::{database::DatabaseView, locale::t};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Disableable, IconName, Selectable, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    input::{Input, InputState},
    menu::{DropdownMenu, PopupMenuItem},
};
use turbodbn_services::database::workbench::{Action, Column, Object, Reply};
use uuid::Uuid;
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PickAction {
    Select,
    Copy,
    SelectSql,
    Structure,
    Data,
}
pub struct Selected {
    pub scope: (Uuid, String, u64),
    pub object: Object,
}
pub struct Picker {
    inline: bool,
    sql_tables: bool,
    columns_mode: bool,
    column_object: Option<Object>,
    column_cache: Option<(Object, Vec<Column>)>,
    column_task: Option<Task<()>>,
    column_error: String,
    table_search: String,
    scope_label: String,
    action_object: Option<Object>,
    include_views: bool,
    selected: usize,
    scroll: UniformListScrollHandle,
    source: WeakEntity<DatabaseView>,
    scope: (Uuid, String, u64),
    input: Entity<InputState>,
    objects: Vec<Object>,
    query: String,
    offset: usize,
    more: bool,
    busy: bool,
    loaded: bool,
    error: String,
    paged: bool,
    timer: Option<Task<()>>,
    _observer: Subscription,
}
impl EventEmitter<Selected> for Picker {}
impl EventEmitter<DismissEvent> for Picker {}
impl Picker {
    pub fn new(
        source: WeakEntity<DatabaseView>,
        id: Uuid,
        database: String,
        generation: u64,
        paged: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(t(cx, "ddl.filter")));
        let mut previous = input.read(cx).value();
        let mut previous_composing = false;
        let observer = cx.subscribe_in(&input, window, move |this, input, event, window, cx| {
            if !matches!(event, gpui_component::input::InputEvent::Change) {
                return;
            }
            let value = input.read(cx).value();
            let composing = input.update(cx, |input, cx| {
                input.marked_text_range(window, cx).is_some()
            });
            if value == previous && composing == previous_composing {
                return;
            }
            previous = value;
            previous_composing = composing;
            this.selected = 0;
            this.scroll.scroll_to_item(0, ScrollStrategy::Top);
            if composing {
                this.timer = None;
                cx.notify();
                return;
            }
            let query = input.read(cx).value().trim().to_lowercase();
            this.timer = None;
            if this.paged && this.column_object.is_none() && query != this.query {
                this.timer = Some(cx.spawn(async move |view, cx| {
                    gpui::Timer::after(std::time::Duration::from_millis(350)).await;
                    let _ = view.update(cx, |this, cx| {
                        this.timer = None;
                        this.query = query;
                        this.loaded = false;
                        this.offset = 0;
                        if !this.busy {
                            this.load(false, cx);
                        }
                        cx.notify();
                    });
                }));
            }
            cx.notify();
        });
        cx.defer_in(window, |this, _, cx| this.load(false, cx));
        Self {
            inline: false,
            sql_tables: false,
            columns_mode: false,
            column_object: None,
            column_cache: None,
            column_task: None,
            column_error: String::new(),
            table_search: String::new(),
            scope_label: String::new(),
            action_object: None,
            include_views: false,
            selected: 0,
            scroll: UniformListScrollHandle::new(),
            source,
            scope: (id, database, generation),
            input,
            objects: vec![],
            query: String::new(),
            offset: 0,
            more: false,
            busy: false,
            loaded: false,
            error: String::new(),
            paged,
            timer: None,
            _observer: observer,
        }
    }
    pub fn scope_matches(&self, scope: &(Uuid, String, u64)) -> bool {
        &self.scope == scope
    }
    fn scope_current(&self, cx: &App) -> bool {
        self.source.upgrade().is_some_and(|source| {
            let source = source.read(cx);
            source.note_generation(self.scope.0) == self.scope.2
                && source.note_connection_state(self.scope.0) == Some(true)
        })
    }
    pub fn focus_search(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }
    pub fn inline(mut self) -> Self {
        self.inline = true;
        self
    }
    fn filtered_columns(&self, cx: &App) -> Vec<Column> {
        let query = self.input.read(cx).value().trim().to_lowercase();
        self.column_cache
            .as_ref()
            .filter(|(object, _)| Some(object) == self.column_object.as_ref())
            .map(|(_, columns)| {
                columns
                    .iter()
                    .filter(|c| c.name.to_lowercase().contains(&query))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }
    fn choose_column(&self, _name: String, cx: &mut Context<Self>) {
        if let Some(object) = &self.column_object {
            cx.emit(Selected {
                scope: self.scope.clone(),
                object: object.clone(),
            });
        }
    }
    fn show_columns(&mut self, object: Object, window: &mut Window, cx: &mut Context<Self>) {
        if self.column_object.is_none() {
            self.table_search = self.input.read(cx).value().to_string();
        }
        self.column_object = Some(object.clone());
        self.timer = None;
        self.input.update(cx, |input, cx| {
            input.set_placeholder(t(cx, "sql_columns.search"), window, cx);
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        self.selected = 0;
        self.column_error.clear();
        if self
            .column_cache
            .as_ref()
            .is_some_and(|(cached, _)| cached == &object)
        {
            cx.notify();
            return;
        }
        let Some(source) = self.source.upgrade() else {
            return;
        };
        let rx = source.update(cx, |source, cx| {
            source.mcp_request(
                self.scope.0,
                self.scope.2,
                self.scope.1.clone(),
                Action::NoteColumns(object.clone()),
                cx,
            )
        });
        self.column_task = Some(cx.spawn(async move |view, cx| {
            let result = rx.recv().await.unwrap_or_else(|_| Err("mcp.stale".into()));
            let _ = view.update(cx, |this, cx| {
                if this.column_object.as_ref() != Some(&object) {
                    return;
                }
                this.column_task = None;
                if !this.scope_current(cx) {
                    this.column_error = "mcp.stale".into();
                    this.column_cache = None;
                    cx.notify();
                    return;
                }
                match result {
                    Ok(Reply::Schema(mut schema)) => {
                        if schema.columns.len() > 4096 {
                            this.column_error = "browser.limit".into();
                            schema.columns.truncate(4096);
                        }
                        let _ = this.source.update(cx, |source, cx| {
                            source.remember_note_columns(
                                this.scope.0,
                                &this.scope.1,
                                &object,
                                &schema.columns,
                            );
                            cx.emit(crate::database::NoteColumnsReady(this.scope.0, None));
                            cx.notify();
                        });
                        this.column_cache = Some((object, schema.columns));
                    }
                    Err(error) => this.column_error = error,
                    _ => this.column_error = "mcp.failed".into(),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn filtered_objects(&self, cx: &App) -> Vec<Object> {
        let query = self.input.read(cx).value().trim().to_lowercase();
        self.objects
            .iter()
            .filter(|o| {
                (!self.sql_tables || self.include_views || o.kind == "table")
                    && format!("{} {}", o.schema, o.name)
                        .to_lowercase()
                        .contains(&query)
            })
            .cloned()
            .collect()
    }
    fn choose(
        &mut self,
        object: Object,
        action: PickAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.columns_mode && action == PickAction::Select {
            self.show_columns(object, window, cx);
            return;
        }
        cx.emit(Selected {
            scope: self.scope.clone(),
            object,
        });
    }
    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.sql_tables {
            return;
        }
        if event.keystroke.key != "escape"
            && !self.input.read(cx).focus_handle(cx).is_focused(window)
        {
            return;
        }
        if self.input.update(cx, |input, cx| {
            input.marked_text_range(window, cx).is_some()
        }) {
            return;
        }
        let objects = self.filtered_objects(cx);
        let columns = self.filtered_columns(cx);
        let count = if self.column_object.is_some() {
            columns.len()
        } else {
            objects.len()
        };
        match event.keystroke.key.as_str() {
            "escape" => cx.emit(DismissEvent),
            "enter" => {
                if self.column_object.is_some() {
                    if let Some(column) = columns.get(self.selected) {
                        self.choose_column(column.name.clone(), cx);
                    }
                } else if let Some(object) = objects.get(self.selected) {
                    self.choose(object.clone(), PickAction::Select, window, cx);
                }
            }
            "down" | "up" => {
                self.selected = if event.keystroke.key == "down" {
                    self.selected.saturating_add(1).min(count.saturating_sub(1))
                } else {
                    self.selected.saturating_sub(1)
                };
                self.scroll
                    .scroll_to_item(self.selected, ScrollStrategy::Top);
                cx.notify();
            }
            _ => return,
        }
        cx.stop_propagation();
    }
    fn load(&mut self, more: bool, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(source) = self.source.upgrade() else {
            return;
        };
        let offset = if more { self.offset } else { 0 };
        let query = self.query.clone();
        let action = if self.paged {
            Action::BrowseCatalog {
                category: "relations".into(),
                search: query.clone(),
                offset,
                limit: 200,
                include_comments: false,
            }
        } else {
            Action::Catalog
        };
        let rx = source.update(cx, |source, cx| {
            source.mcp_request(self.scope.0, self.scope.2, self.scope.1.clone(), action, cx)
        });
        self.busy = true;
        self.error.clear();
        cx.notify();
        cx.spawn(async move |view, cx| {
            let result = rx.recv().await.unwrap_or_else(|_| Err("mcp.stale".into()));
            let _ = view.update(cx, |this, cx| {
                this.busy = false;
                if !this.scope_current(cx) {
                    this.objects.clear();
                    this.error = "mcp.stale".into();
                    cx.notify();
                    return;
                }
                if query != this.query {
                    this.load(false, cx);
                    return;
                }
                this.loaded = true;
                let result = match result {
                    Ok(Reply::CatalogPage { objects, more }) => Ok((objects, more)),
                    Ok(Reply::Catalog { objects, .. }) => Ok((objects, false)),
                    Err(error) => Err(error),
                    _ => Err("mcp.failed".into()),
                };
                match result {
                    Ok((objects, more)) => {
                        this.more = more;
                        if offset == 0 {
                            this.objects.clear();
                        }
                        for object in objects
                            .into_iter()
                            .filter(|o| matches!(o.kind.as_str(), "table" | "view"))
                        {
                            let key = crate::object_browser::key(&object);
                            if !this
                                .objects
                                .iter()
                                .any(|o| crate::object_browser::key(o) == key)
                            {
                                this.objects.push(object);
                            }
                        }
                        this.offset = this.objects.len();
                        if this.objects.len() > 5000 {
                            this.objects.truncate(5000);
                            this.more = false;
                        }
                    }
                    Err(error) => this.error = error,
                }
                cx.notify();
            });
        })
        .detach();
    }
}
impl Picker {
    fn render_columns(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let columns = self.filtered_columns(cx);
        self.selected = self.selected.min(columns.len().saturating_sub(1));
        let empty = columns.is_empty();
        let object = self.column_object.clone().unwrap();
        div()
            .w(px(400.))
            .v_flex()
            .gap_2()
            .p_2()
            .capture_key_down(
                cx.listener(|this, event: &KeyDownEvent, window, cx| this.key(event, window, cx)),
            )
            .child(
                Button::new("sql-columns-back")
                    .ghost()
                    .small()
                    .icon(IconName::ChevronLeft)
                    .label(object.name)
                    .tooltip(t(cx, "ddl.select_table"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.column_object = None;
                        this.column_task = None;
                        this.selected = 0;
                        this.input.update(cx, |input, cx| {
                            input.set_placeholder(t(cx, "ddl.filter"), window, cx);
                            input.set_value(this.table_search.clone(), window, cx);
                            input.focus(window, cx);
                        });
                        cx.notify();
                    })),
            )
            .child(Input::new(&self.input).cleanable(true))
            .child(
                gpui::uniform_list(
                    "sql-column-results",
                    columns.len(),
                    cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                        range
                            .map(|index| {
                                let column = columns[index].clone();
                                let label = column.name.clone();
                                let tooltip = format!(
                                    "{} · {}{}",
                                    column.name,
                                    column.data_type,
                                    if column.comment.is_empty() {
                                        String::new()
                                    } else {
                                        format!(
                                            " · {}",
                                            column.comment.chars().take(2048).collect::<String>()
                                        )
                                    }
                                );
                                Button::new(SharedString::from(format!(
                                    "sql-column-result-{}",
                                    column.name
                                )))
                                .ghost()
                                .small()
                                .w_full()
                                .justify_start()
                                .selected(this.selected == index)
                                .child(
                                    div()
                                        .h_flex()
                                        .gap_2()
                                        .min_w_0()
                                        .child(div().truncate().child(label.clone()))
                                        .child(
                                            div()
                                                .text_xs()
                                                .flex_shrink_0()
                                                .child(column.data_type.clone()),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .min_w_0()
                                                .overflow_hidden()
                                                .text_ellipsis()
                                                .child(
                                                    column
                                                        .comment
                                                        .chars()
                                                        .take(240)
                                                        .collect::<String>(),
                                                ),
                                        ),
                                )
                                .tooltip(tooltip)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.choose_column(column.name.clone(), cx)
                                }))
                                .into_any_element()
                            })
                            .collect()
                    }),
                )
                .track_scroll(self.scroll.clone())
                .h(px(280.)),
            )
            .when(self.column_task.is_some(), |b| {
                b.child(t(cx, "sql_columns.loading"))
            })
            .when(
                empty && self.column_task.is_none() && self.column_error.is_empty(),
                |b| b.child(t(cx, "sql_tables.empty")),
            )
            .when(!self.column_error.is_empty(), |b| {
                b.child(crate::locale::message(cx, &self.column_error))
            })
            .child(
                Button::new("sql-columns-refresh")
                    .ghost()
                    .small()
                    .icon(crate::assets::refresh_icon())
                    .label(t(cx, "dbwork.refresh"))
                    .disabled(self.column_task.is_some())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.column_cache = None;
                        this.show_columns(this.column_object.clone().unwrap(), window, cx);
                    })),
            )
            .into_any_element()
    }
}

impl Render for Picker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.column_object.is_some() {
            return self.render_columns(cx);
        }
        let objects = self.filtered_objects(cx);
        self.selected = self.selected.min(objects.len().saturating_sub(1));
        if self.inline {
            let empty = objects.is_empty();
            let mut actions = div().v_flex().gap_1();
            if let Some(object) = self.action_object.clone() {
                actions = actions.child(div().text_xs().truncate().child(object.name.clone()));
                for (label, action) in [
                    (t(cx, "sql_tables.copy"), PickAction::Copy),
                    (t(cx, "sql_tables.select"), PickAction::SelectSql),
                    (t(cx, "designer.right"), PickAction::Structure),
                    (t(cx, "dbwork.data"), PickAction::Data),
                ] {
                    let object = object.clone();
                    actions = actions.child(
                        Button::new(SharedString::from(format!("sql-table-action-{}", label)))
                            .ghost()
                            .small()
                            .justify_start()
                            .label(label)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.choose(object.clone(), action, window, cx)
                            })),
                    );
                }
            }
            return div()
                .w(px(400.))
                .v_flex()
                .gap_2()
                .p_2()
                .capture_key_down(
                    cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        this.key(event, window, cx)
                    }),
                )
                .when(self.sql_tables, |b| {
                    b.child(div().text_xs().child(self.scope_label.clone()))
                })
                .child(Input::new(&self.input).cleanable(true))
                .when(self.sql_tables, |b| {
                    b.child(
                        Button::new("sql-tables-views")
                            .ghost()
                            .small()
                            .selected(self.include_views)
                            .label(t(cx, "sql_tables.views"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.include_views = !this.include_views;
                                this.selected = 0;
                                this.scroll.scroll_to_item(0, ScrollStrategy::Top);
                                cx.notify();
                            })),
                    )
                })
                .when(empty && !self.busy && self.error.is_empty(), |b| {
                    b.child(t(cx, "sql_tables.empty"))
                })
                .child(
                    gpui::uniform_list(
                        "ddl-picker-results",
                        objects.len(),
                        cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                            range
                                .map(|index| {
                                    let object = objects[index].clone();
                                    let label = if object.schema.is_empty() {
                                        object.name.clone()
                                    } else {
                                        format!("{}.{}", object.schema, object.name)
                                    };
                                    let selected = this.sql_tables && this.selected == index;
                                    let choice = object.clone();
                                    let row = div()
                                        .h_flex()
                                        .h(px(32.))
                                        .min_w_0()
                                        .child(
                                            Button::new(SharedString::from(format!(
                                                "ddl-picker-result-{}",
                                                crate::object_browser::key(&object)
                                            )))
                                            .ghost()
                                            .small()
                                            .flex_1()
                                            .min_w_0()
                                            .justify_start()
                                            .selected(selected)
                                            .child(div().truncate().child(label.clone()))
                                            .tooltip(label)
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.choose(
                                                    choice.clone(),
                                                    PickAction::Select,
                                                    window,
                                                    cx,
                                                )
                                            })),
                                        )
                                        .when(this.sql_tables && !this.columns_mode, |row| {
                                            row.child(
                                                Button::new(SharedString::from(format!(
                                                    "sql-table-more-{}",
                                                    crate::object_browser::key(&object)
                                                )))
                                                .ghost()
                                                .small()
                                                .icon(IconName::Ellipsis)
                                                .tooltip(t(cx, "sql_tables.actions"))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.action_object =
                                                        if this.action_object.as_ref()
                                                            == Some(&object)
                                                        {
                                                            None
                                                        } else {
                                                            Some(object.clone())
                                                        };
                                                    cx.notify();
                                                })),
                                            )
                                        });
                                    row.into_any_element()
                                })
                                .collect::<Vec<_>>()
                        }),
                    )
                    .track_scroll(self.scroll.clone())
                    .h(px(280.)),
                )
                .when(self.action_object.is_some(), |b| b.child(actions))
                .when(self.busy || self.timer.is_some(), |b| {
                    b.child(t(cx, "ddl.loading"))
                })
                .when(!self.error.is_empty(), |b| {
                    b.child(crate::locale::message(cx, &self.error))
                })
                .when(self.more, |b| {
                    b.child(
                        Button::new("ddl-picker-more")
                            .ghost()
                            .label(t(cx, "browser.more"))
                            .disabled(self.busy || self.offset >= 5000)
                            .on_click(cx.listener(|this, _, _, cx| this.load(true, cx))),
                    )
                })
                .when(self.offset >= 5000, |b| b.child(t(cx, "browser.limit")))
                .when(!self.paged, |b| b.child(t(cx, "browser.local_only")))
                .child(
                    Button::new("ddl-picker-retry")
                        .ghost()
                        .icon(crate::assets::refresh_icon())
                        .label(t(cx, "dbwork.refresh"))
                        .disabled(self.busy)
                        .on_click(cx.listener(|this, _, _, cx| this.load(false, cx))),
                )
                .into_any_element();
        }
        let scope = self.scope.clone();
        let owner = cx.entity().downgrade();
        div()
            .v_flex()
            .size_full()
            .p_3()
            .gap_2()
            .min_w_0()
            .child(div().text_sm().child(self.scope.1.clone()))
            .child(Input::new(&self.input).cleanable(true))
            .child(
                Button::new("ddl-table-picker")
                    .label(t(cx, "ddl.select_table"))
                    .dropdown_caret(true)
                    .dropdown_menu(move |mut menu, _, _| {
                        for object in &objects {
                            let object = object.clone();
                            let owner = owner.clone();
                            let scope = scope.clone();
                            let label = if object.schema.is_empty() {
                                object.name.clone()
                            } else {
                                format!("{}.{}", object.schema, object.name)
                            };
                            menu =
                                menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                                    let _ = owner.update(cx, |_, cx| {
                                        cx.emit(Selected {
                                            scope: scope.clone(),
                                            object: object.clone(),
                                        })
                                    });
                                }));
                        }
                        menu.scrollable(true).max_h(px(360.))
                    }),
            )
            .when(self.busy || self.timer.is_some(), |b| {
                b.child(t(cx, "ddl.loading"))
            })
            .when(!self.error.is_empty(), |b| {
                b.child(crate::locale::message(cx, &self.error))
            })
            .when(self.more, |b| {
                b.child(
                    Button::new("ddl-more")
                        .ghost()
                        .icon(IconName::ChevronDown)
                        .label(t(cx, "browser.more"))
                        .disabled(self.busy || self.offset >= 5000)
                        .on_click(cx.listener(|this, _, _, cx| this.load(true, cx))),
                )
            })
            .child(
                Button::new("ddl-picker-refresh")
                    .ghost()
                    .icon(crate::assets::refresh_icon())
                    .tooltip(t(cx, "dbwork.refresh"))
                    .disabled(self.busy)
                    .on_click(cx.listener(|this, _, _, cx| this.load(false, cx))),
            )
            .when(!self.paged, |b| b.child(t(cx, "browser.local_only")))
            .into_any_element()
    }
}

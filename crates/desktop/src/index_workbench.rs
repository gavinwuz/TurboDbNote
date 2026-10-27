#[path = "index_editor.rs"]
mod editor;
use crate::{
    database::{DatabaseView, OpenTable},
    feedback,
    locale::{t, tr},
    query_grid::{self, QueryGrid},
};
use gpui::{prelude::*, *};
use gpui_component::{
    Disableable, IconName, Selectable, StyledExt, WindowExt,
    button::{Button, ButtonVariants},
    input::{Input, InputState},
    menu::{DropdownMenu, PopupMenuItem},
    table::TableState,
};
use turbodbn_services::{
    DatabaseKind,
    database::{
        index_design::{self, Draft},
        workbench::{Action, Object, Reply, Rows, Schema},
    },
};
pub struct Indexes {
    source: WeakEntity<DatabaseView>,
    target: OpenTable,
    filter: Entity<InputState>,
    grid: Option<Entity<TableState<QueryGrid>>>,
    objects: Vec<Object>,
    schema: Option<Schema>,
    table: Option<Object>,
    drafts: Vec<(Object, Schema, Draft)>,
    busy: bool,
    error: String,
    epoch: u64,
    scope_filter: bool,
    focus_index: Option<String>,
    initial_action: Option<&'static str>,
    detail_epoch: u64,
    detail_loading: bool,
    detail_grid: Option<Entity<TableState<QueryGrid>>>,
    inspected: Option<(Object, Schema)>,
    _subscription: Subscription,
    grid_subscription: Option<Subscription>,
    exact_index: Option<String>,
}
impl EventEmitter<crate::table_view::TableDraftChanged> for Indexes {}
impl Indexes {
    pub(crate) fn request(
        &mut self,
        kind: &str,
        name: &str,
        parent: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if kind == "index" {
            self.exact_index = Some(name.into());
            self.filter.update(cx, |filter, cx| {
                filter.set_value(name.to_owned(), window, cx)
            });
            self.refresh_grid(window, cx);
        }
        let action = match kind {
            "index-create" => Some("create"),
            "index-drop" => Some("drop"),
            _ => None,
        };
        if self.busy {
            self.focus_index = Some(name.into());
            self.initial_action = action;
            return;
        }
        if action == Some("create") {
            self.target.object.name = parent.into();
            self.scope_filter = true;
            if let Some(grid) = &self.grid {
                grid.update(cx, |grid, _| grid.delegate_mut().selected = None);
            }
            self.edit(None, false, window, cx);
        } else if let Some(object) = self
            .objects
            .iter()
            .find(|object| object.name == name && object.parent == self.target.object.name)
            .cloned()
        {
            if action == Some("drop") {
                self.edit(Some(object), true, window, cx);
            } else {
                self.inspect(object, window, cx);
            }
        }
    }
    pub(crate) fn pending(&self) -> bool {
        !self.drafts.is_empty()
    }
    pub(crate) fn discard(&mut self, cx: &mut Context<Self>) {
        self.drafts.clear();
        cx.emit(crate::table_view::TableDraftChanged);
        cx.notify();
    }
    pub fn new(
        source: WeakEntity<DatabaseView>,
        mut target: OpenTable,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let initial_action = match target.object.kind.as_str() {
            "index-create" => Some("create"),
            "index-drop" => Some("drop"),
            _ => None,
        };
        let focus_index = target
            .object
            .kind
            .starts_with("index")
            .then(|| target.object.name.clone());
        if focus_index.is_some() {
            target.object.name = target.object.parent.clone();
            target.object.kind = "table".into();
        }
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder(t(cx, "indexes.search")));
        if initial_action.is_none()
            && let Some(name) = &focus_index
        {
            filter.update(cx, |filter, cx| filter.set_value(name.clone(), window, cx));
        }
        let exact_index = if initial_action.is_none() {
            focus_index.clone()
        } else {
            None
        };
        let subscription = cx.subscribe_in(&filter, window, |this, _, event, window, cx| {
            if matches!(event, gpui_component::input::InputEvent::Change) {
                this.refresh_grid(window, cx);
            }
        });
        cx.defer_in(window, |this, window, cx| this.load(false, window, cx));
        Self {
            grid_subscription: None,
            exact_index,
            focus_index,
            initial_action,
            detail_epoch: 0,
            detail_loading: false,
            detail_grid: None,
            inspected: None,
            source,
            target,
            filter,
            grid: None,
            objects: vec![],
            schema: None,
            table: None,
            drafts: vec![],
            busy: false,
            error: String::new(),
            epoch: 0,
            scope_filter: true,
            _subscription: subscription,
        }
    }
    fn current(&self, cx: &App) -> bool {
        self.source.upgrade().is_some_and(|source| {
            source.read(cx).note_generation(self.target.connection) == self.target.generation
                && source
                    .read(cx)
                    .note_connection_state(self.target.connection)
                    == Some(true)
        })
    }
    fn load(&mut self, more: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || !self.current(cx) {
            return;
        }
        if !more {
            self.objects.clear();
        }
        if self.objects.len() >= 20_000 {
            self.error = "indexes.limit".into();
            return;
        }
        self.busy = true;
        self.error.clear();
        self.epoch += 1;
        let epoch = self.epoch;
        let target = self.target.clone();
        let offset = self.objects.len();
        let receiver = self.source.upgrade().unwrap().update(cx, |source, cx| {
            source.mcp_request(
                target.connection,
                target.generation,
                target.database.clone(),
                Action::BrowseCatalog {
                    category: "index".into(),
                    search: String::new(),
                    offset,
                    limit: 200,
                    include_comments: false,
                },
                cx,
            )
        });
        cx.spawn_in(window, async move |view, cx| {
            let result = receiver
                .recv()
                .await
                .unwrap_or_else(|_| Err("mcp.stale".into()));
            let _ = view.update_in(cx, |this, window, cx| {
                this.busy = false;
                if this.epoch != epoch || !this.current(cx) {
                    return;
                }
                match result {
                    Ok(Reply::CatalogPage { objects, more }) => {
                        this.objects.extend(objects);
                        this.refresh_grid(window, cx);
                        if (!more || this.initial_action.is_none())
                            && let Some(name) = this.focus_index.clone()
                            && let Some(object) = this
                                .objects
                                .iter()
                                .find(|object| {
                                    object.name == name && object.parent == this.target.object.name
                                })
                                .cloned()
                        {
                            this.focus_index = None;
                            match this.initial_action.take() {
                                Some("drop") => this.edit(Some(object), true, window, cx),
                                Some("create") => this.edit(None, false, window, cx),
                                _ => this.inspect(object, window, cx),
                            }
                        }
                        if more {
                            cx.defer_in(window, |this, window, cx| this.load(true, window, cx));
                        }
                    }
                    Err(error) => this.error = error,
                    _ => this.error = "mcp.failed".into(),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn refresh_grid(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let previous = self
            .selected(cx)
            .or_else(|| self.inspected.as_ref().map(|(object, _)| object.clone()));
        let query = self.filter.read(cx).value().to_lowercase();
        let scoped = self.scope_filter && !self.target.object.name.is_empty();
        let rows = Rows {
            columns: vec![
                t(cx, "compare.table").to_string(),
                t(cx, "indexes.name").to_string(),
                t(cx, "indexes.definition").to_string(),
            ],
            values: self
                .objects
                .iter()
                .filter(|object| {
                    (!scoped || object.parent == self.target.object.name)
                        && self.exact_index.as_ref().is_none_or(|exact| {
                            query != exact.to_lowercase() || object.name == *exact
                        })
                        && format!("{} {} {}", object.parent, object.name, object.comment)
                            .to_lowercase()
                            .contains(&query)
                })
                .map(|object| {
                    vec![
                        Some(object.parent.clone()),
                        Some(object.name.clone()),
                        Some(object.comment.clone()),
                    ]
                })
                .collect(),
        };
        let selected = previous.as_ref().and_then(|object| {
            rows.values.iter().position(|row| {
                row.first().and_then(Option::as_deref) == Some(object.parent.as_str())
                    && row.get(1).and_then(Option::as_deref) == Some(object.name.as_str())
            })
        });
        if selected.is_none() {
            self.detail_epoch += 1;
            self.detail_loading = false;
            self.inspected = None;
            self.detail_grid = None;
        }
        let grid = cx.new(|cx| {
            TableState::new(
                QueryGrid::new(rows, 0).with_column_widths(&[180., 220., 500.]),
                window,
                cx,
            )
        });
        grid.update(cx, |grid, _| {
            grid.delegate_mut().selected = selected.map(|row| (row, 0))
        });
        self.grid_subscription =
            Some(
                cx.subscribe_in(&grid, window, |this, grid, event, window, cx| {
                    if let gpui_component::table::TableEvent::SelectRow(row) = event {
                        grid.update(cx, |grid, _| grid.delegate_mut().selected = Some((*row, 0)));
                        if let Some(object) = this.selected(cx) {
                            this.inspect(object, window, cx);
                        }
                    }
                }),
            );
        self.grid = Some(grid);
        cx.notify();
    }
    fn selected(&self, cx: &App) -> Option<Object> {
        let grid = self.grid.as_ref()?.read(cx).delegate();
        let index = grid.selected?.0;
        let row = grid.rows.values.get(index)?;
        let name = row.get(1)?.as_deref()?;
        let table = row.first()?.as_deref()?;
        self.objects
            .iter()
            .find(|object| object.name == name && object.parent == table)
            .cloned()
    }
    fn inspect(&mut self, object: Object, window: &mut Window, cx: &mut Context<Self>) {
        if !self.current(cx) || object.parent.is_empty() {
            return;
        }
        self.inspected = None;
        self.detail_grid = None;
        self.detail_loading = true;
        if let Some(grid) = &self.grid {
            grid.update(cx, |grid, _| {
                let row = grid.delegate().rows.values.iter().position(|row| {
                    row.first().and_then(Option::as_deref) == Some(object.parent.as_str())
                        && row.get(1).and_then(Option::as_deref) == Some(object.name.as_str())
                });
                grid.delegate_mut().selected = row.map(|row| (row, 0));
            });
        }
        cx.notify();
        let mut table = object.clone();
        table.kind = "table".into();
        table.name = object.parent.clone();
        self.detail_epoch += 1;
        let detail_epoch = self.detail_epoch;
        let receiver = self.source.upgrade().unwrap().update(cx, |source, cx| {
            source.mcp_request(
                self.target.connection,
                self.target.generation,
                self.target.database.clone(),
                Action::Describe(table.clone()),
                cx,
            )
        });
        cx.spawn_in(window, async move |view, cx| {
            let result = receiver
                .recv()
                .await
                .unwrap_or_else(|_| Err("mcp.stale".into()));
            let _ = view.update_in(cx, |this, window, cx| {
                if this.detail_epoch != detail_epoch || !this.current(cx) {
                    return;
                }
                this.detail_loading = false;
                match result {
                    Ok(Reply::Schema(schema)) => {
                        let rows = Rows {
                            columns: vec![
                                t(cx, "indexes.position").to_string(),
                                t(cx, "ddl.column.name").to_string(),
                                t(cx, "grid.structure.type").to_string(),
                                t(cx, "indexes.direction").to_string(),
                                t(cx, "indexes.nullable").to_string(),
                                t(cx, "indexes.prefix").to_string(),
                            ],
                            values: schema
                                .index_columns
                                .get(&object.name)
                                .map(|columns| {
                                    columns
                                        .iter()
                                        .map(|column| {
                                            vec![
                                                Some(column.position.to_string()),
                                                Some(column.name.clone()),
                                                schema
                                                    .columns
                                                    .iter()
                                                    .find(|field| field.name == column.name)
                                                    .map(|field| field.data_type.clone()),
                                                column.descending.map(|desc| {
                                                    if desc { "DESC" } else { "ASC" }.into()
                                                }),
                                                column.nullable.map(|nullable| {
                                                    if nullable { "✓" } else { "—" }.into()
                                                }),
                                                column.prefix.map(|prefix| prefix.to_string()),
                                            ]
                                        })
                                        .collect()
                                })
                                .unwrap_or_default(),
                        };
                        this.detail_grid = Some(cx.new(|cx| {
                            TableState::new(
                                QueryGrid::new(rows, 0)
                                    .without_row_numbers()
                                    .fixed_columns(2)
                                    .with_column_widths(&[70., 220., 180., 90., 90., 90.]),
                                window,
                                cx,
                            )
                        }));
                        this.inspected = Some((object, schema));
                    }
                    Err(error) => feedback::error(crate::locale::message(cx, &error), cx),
                    _ => {}
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn edit(
        &mut self,
        existing: Option<Object>,
        drop_only: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.target.writable {
            feedback::error(t(cx, "indexes.read_only"), cx);
            return;
        }
        if !matches!(
            self.target.kind,
            DatabaseKind::MySql | DatabaseKind::PostgreSql | DatabaseKind::Sqlite
        ) {
            feedback::error(t(cx, "indexes.unsupported"), cx);
            return;
        }
        if self.busy || !self.current(cx) {
            return;
        }
        let selected = self.selected(cx);
        let table = existing
            .as_ref()
            .or(selected.as_ref())
            .map(|object| object.parent.clone())
            .filter(|name| !name.is_empty())
            .or_else(|| {
                (!self.target.object.name.is_empty()).then(|| self.target.object.name.clone())
            });
        let Some(name) = table else {
            self.choose_table(window, cx);
            return;
        };
        let table = Object {
            schema: existing
                .as_ref()
                .map(|object| object.schema.clone())
                .unwrap_or_else(|| self.target.object.schema.clone()),
            name,
            kind: "table".into(),
            comment: String::new(),
            parent: String::new(),
        };
        let receiver = self.source.upgrade().unwrap().update(cx, |source, cx| {
            source.mcp_request(
                self.target.connection,
                self.target.generation,
                self.target.database.clone(),
                Action::Describe(table.clone()),
                cx,
            )
        });
        self.busy = true;
        cx.spawn_in(window, async move |view, cx| {
            let result = receiver
                .recv()
                .await
                .unwrap_or_else(|_| Err("mcp.stale".into()));
            let _ = view.update_in(cx, |this, window, cx| {
                this.busy = false;
                if !this.current(cx) {
                    return;
                }
                match result {
                    Ok(Reply::Schema(schema)) => {
                        if existing
                            .as_ref()
                            .is_some_and(|object| index_design::protected(&schema, &object.name))
                        {
                            feedback::error(t(cx, "indexes.protected"), cx);
                            return;
                        }
                        this.schema = Some(schema.clone());
                        this.table = Some(table.clone());
                        if drop_only {
                            if let Some(existing) = existing {
                                let draft = Draft {
                                    name: existing.name.clone(),
                                    previous: Some(existing.name),
                                    columns: vec![],
                                    prefixes: Default::default(),
                                    unique: false,
                                    drop_only: true,
                                };
                                this.stage(table, schema, draft, cx);
                            }
                        } else {
                            this.editor(table, schema, existing, window, cx);
                        }
                    }
                    Err(error) => feedback::error(crate::locale::message(cx, &error), cx),
                    _ => {}
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn stage(&mut self, table: Object, schema: Schema, draft: Draft, cx: &mut Context<Self>) {
        match index_design::sql(
            self.target.kind,
            &self.target.database,
            &table,
            &schema,
            &draft,
        ) {
            Ok(_) => {
                if let Some(index) = self.drafts.iter().position(|(old, _, change)| {
                    old == &table
                        && (change.name == draft.name
                            || change.previous.is_some() && change.previous == draft.previous)
                }) {
                    self.drafts.remove(index);
                }
                self.drafts.push((table, schema, draft));
                cx.emit(crate::table_view::TableDraftChanged);
                feedback::success(t(cx, "indexes.staged"), cx);
            }
            Err(error) => feedback::error(crate::locale::message(cx, &error), cx),
        }
        cx.notify();
    }
    fn choose_table(&self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(t(cx, "compare.table")));
        let owner = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let input = input.clone();
            let apply = input.clone();
            let owner = owner.clone();
            dialog
                .title(t(cx, "indexes.choose_table_title"))
                .w(px(420.))
                .child(Input::new(&input).cleanable(true))
                .footer(move |_, _, _, cx| {
                    let apply = apply.clone();
                    let owner = owner.clone();
                    vec![
                        feedback::footer_actions()
                            .child(feedback::close_button(cx))
                            .child(
                                Button::new("index-table-apply")
                                    .primary()
                                    .label(t(cx, "grid.apply"))
                                    .on_click(move |_, window, cx| {
                                        let name = apply.read(cx).value().trim().to_string();
                                        if name.is_empty()
                                            || name.len() > 256
                                            || name.chars().any(char::is_control)
                                        {
                                            feedback::error(t(cx, "indexes.invalid"), cx);
                                            return;
                                        }
                                        window.close_dialog(cx);
                                        let _ = owner.update(cx, |this, cx| {
                                            this.target.object.name = name;
                                            this.scope_filter = true;
                                            this.edit(None, false, window, cx);
                                        });
                                    }),
                            ),
                    ]
                })
        });
    }
    fn editor(
        &self,
        table: Object,
        schema: Schema,
        existing: Option<Object>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let owner = cx.entity().downgrade();
        let target = self.target.clone();
        let editor =
            cx.new(|cx| editor::Editor::new(owner, target, table, schema, existing, window, cx));
        window.open_dialog(cx, move |dialog, _, cx| {
            dialog
                .title(t(cx, "indexes.design"))
                .w(px(740.))
                .on_ok(|_, _, _| false)
                .child(editor.clone())
        });
    }
    fn preview(&self, window: &mut Window, cx: &mut Context<Self>) {
        let mut sql = String::new();
        for (table, schema, draft) in &self.drafts {
            match index_design::sql_with_options(
                self.target.kind,
                &self.target.database,
                table,
                schema,
                draft,
                false,
            ) {
                Ok(statement) => sql.push_str(&statement),
                Err(error) => {
                    feedback::error(crate::locale::message(cx, &error), cx);
                    return;
                }
            }
        }
        if sql.is_empty() {
            return;
        }
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .default_value(sql.clone())
        });
        let sql = std::sync::Arc::new(sql);
        let include_database = std::rc::Rc::new(std::cell::Cell::new(false));
        let drafts = self.drafts.clone();
        let kind = self.target.kind;
        let database = self.target.database.clone();
        let output = self
            .source
            .upgrade()
            .and_then(|source| source.read(cx).output.clone());
        let connection = self.target.connection;
        window.open_dialog(cx, move |dialog, window, cx| {
            let value=drafts.iter().map(|(table,schema,draft)|index_design::sql_with_options(kind,&database,table,schema,draft,include_database.get())).collect::<Result<Vec<_>,_>>().map(|parts|parts.join("")).unwrap_or_else(|_|sql.to_string());
            if input.read(cx).value().as_str()!=value { input.update(cx,|input,cx|input.set_value(value.clone(),window,cx)); }
            let sql=std::sync::Arc::new(value);let output=output.clone();let include=include_database.clone();
            dialog
                .title(t(cx, "indexes.preview"))
                .w(px(760.))
                .on_ok(|_, _, _| false)
                .child(
                    div()
                        .v_flex()
                        .gap_2()
                        .child(t(cx, "indexes.preview_hint"))
                        .child(tr(cx,"indexes.preview_target",&[("database",database.as_str())]))
                        .child(gpui_component::checkbox::Checkbox::new("index-include-database").checked(include_database.get()).label(t(cx,"indexes.include_database")).on_click(move|value,window,_|{include.set(*value);window.refresh();}))
                        .child(Input::new(&input).disabled(true).h(px(360.))),
                )
                .footer(move |_, _, _, cx| {
                    let copy = sql.clone();
                    let save = sql.clone();let output=output.clone();
                    vec![
                        feedback::footer_actions()
                            .child(feedback::close_button(cx))
                            .child(
                                Button::new("copy-index-sql")
                                    .icon(IconName::Copy)
                                    .label(t(cx, "dbwork.copy"))
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            copy.to_string(),
                                        ));
                                        feedback::success(t(cx, "feedback.copied"), cx);
                                    }),
                            )
                            .child(
                                Button::new("save-index-sql")
                                    .primary()
                                    .icon(IconName::FolderOpen)
                                    .label(t(cx, "compare.save_sql"))
                                    .on_click(move |_, _, cx| {
                                        let pick = cx.prompt_for_new_path(
                                            &dirs::document_dir().unwrap_or_default(),
                                            Some("index-changes.sql"),
                                        );
                                        let sql = save.clone();let output=output.clone();
                                        cx.spawn(async move |cx| {
                                            if let Ok(Ok(Some(mut path))) = pick.await {
                                                if path.extension().is_none(){path.set_extension("sql");}
                                                let task=uuid::Uuid::new_v4();let started=std::time::Instant::now();
                                                let _=cx.update(|cx|{if let Some(output)=&output{let mut record=crate::operation_log::Record::new(task,crate::operation_log::Kind::Export,crate::operation_log::Phase::Started);record.source=Some(connection);output.update(cx,|output,cx|output.operation(record,cx));}});
                                                let result = cx
                                                    .background_executor()
                                                    .spawn(async move {
                                                        use std::io::Write;
                                                        let mut file =
                                                            tempfile::NamedTempFile::new_in(
                                                                path.parent()
                                                                    .ok_or("compare.save_failed")?,
                                                            )
                                                            .map_err(|_| "compare.save_failed")?;
                                                        file.write_all(sql.as_bytes())
                                                            .map_err(|_| "compare.save_failed")?;
                                                        file.as_file().sync_all().map_err(|_|"compare.save_failed")?;
                                                        file.persist_noclobber(path)
                                                            .map_err(|_| "compare.save_failed")?;
                                                        Ok::<_, &str>(())
                                                    })
                                                    .await;
                                                let _=cx.update(|cx|{
                                                    if let Some(output)=&output{
                                                        let mut record=crate::operation_log::Record::new(task,crate::operation_log::Kind::Export,if result.is_ok(){crate::operation_log::Phase::Succeeded}else{crate::operation_log::Phase::Failed});record.source=Some(connection);record.elapsed_ms=started.elapsed().as_millis() as u64;
                                                        if result.is_err(){record.failure="disk";}
                                                        output.update(cx,|output,cx|output.operation(record,cx));
                                                    }
                                                    match result{Ok(())=>feedback::success(t(cx,"feedback.saved"),cx),Err(_)=>feedback::error(t(cx,"compare.save_failed"),cx)}
                                                });
                                            }
                                        })
                                        .detach();
                                    }),
                            ),
                    ]
                })
        });
    }
}
impl Render for Indexes {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected(cx);
        let writable = self.target.writable
            && matches!(
                self.target.kind,
                DatabaseKind::MySql | DatabaseKind::PostgreSql | DatabaseKind::Sqlite
            )
            && self.current(cx);
        div()
            .v_flex()
            .size_full()
            .min_h_0()
            .min_w_0()
            .p_2()
            .gap_2()
            .child(
                div()
                    .h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        div()
                            .w(px(240.))
                            .child(Input::new(&self.filter).cleanable(true)),
                    )
                    .child(
                        Button::new("index-refresh")
                            .ghost()
                            .icon(crate::assets::refresh_icon())
                            .disabled(self.busy)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.load(false, window, cx)),
                            ),
                    )
                    .when(!self.target.object.name.is_empty(), |bar| {
                        bar.child(
                            Button::new("index-scope")
                                .ghost()
                                .label(t(cx, "indexes.table_only"))
                                .selected(self.scope_filter)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.scope_filter = !this.scope_filter;
                                    this.refresh_grid(window, cx);
                                })),
                        )
                    })
                    .child(
                        Button::new("index-new")
                            .ghost()
                            .icon(IconName::Plus)
                            .label(t(cx, "indexes.new"))
                            .disabled(!writable || self.busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.edit(None, false, window, cx)
                            })),
                    )
                    .child(
                        Button::new("index-view")
                            .ghost()
                            .label(t(cx, "indexes.view"))
                            .disabled(selected.is_none())
                            .on_click(cx.listener(|this, _, window, cx| {
                                if let Some(object) = this.selected(cx) {
                                    this.inspect(object, window, cx);
                                }
                            })),
                    )
                    .child(
                        Button::new("index-edit")
                            .ghost()
                            .label(t(cx, "indexes.edit"))
                            .disabled(!writable || self.busy || selected.is_none())
                            .on_click(cx.listener(|this, _, window, cx| {
                                if let Some(selected) = this.selected(cx) {
                                    this.edit(Some(selected), false, window, cx);
                                }
                            })),
                    )
                    .child(
                        Button::new("index-drop")
                            .ghost()
                            .icon(IconName::Delete)
                            .label(t(cx, "indexes.stage_drop"))
                            .disabled(!writable || self.busy || selected.is_none())
                            .on_click(cx.listener(|this, _, window, cx| {
                                if let Some(selected) = this.selected(cx) {
                                    this.edit(Some(selected), true, window, cx);
                                }
                            })),
                    ),
            )
            .when(!self.drafts.is_empty(), |body| {
                body.child(
                    div()
                        .h_flex()
                        .gap_2()
                        .child(
                            div().flex_1().text_xs().whitespace_normal().child(
                                self.drafts
                                    .iter()
                                    .map(|(table, _, draft)| {
                                        format!(
                                            "{} · {} · {}",
                                            table.name,
                                            draft.name,
                                            t(
                                                cx,
                                                if draft.drop_only {
                                                    "indexes.pending_drop"
                                                } else if draft.previous.is_some() {
                                                    "indexes.pending_rebuild"
                                                } else {
                                                    "indexes.pending_add"
                                                }
                                            )
                                        )
                                    })
                                    .collect::<Vec<_>>()
                                    .join("; "),
                            ),
                        )
                        .child(
                            Button::new("discard-index-drafts")
                                .ghost()
                                .label(t(cx, "accounts.grants.cancel"))
                                .on_click(cx.listener(|this, _, _, cx| this.discard(cx))),
                        ),
                )
            })
            .when(!self.error.is_empty(), |body| {
                body.child(crate::locale::message(cx, &self.error))
            })
            .when(self.busy, |body| body.child(t(cx, "ddl.loading")))
            .when(self.grid.is_some(), |body| {
                body.child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .child(query_grid::view(self.grid.as_ref().unwrap(), cx)),
                )
            })
            .when(self.detail_loading, |body| body.child(t(cx, "ddl.loading")))
            .when(self.inspected.is_some(), |body| {
                let (object, schema) = self.inspected.as_ref().unwrap();
                let row = schema
                    .indexes
                    .values
                    .iter()
                    .find(|row| row.first().and_then(Option::as_deref) == Some(&object.name));
                let value = |name: &str| {
                    schema
                        .indexes
                        .columns
                        .iter()
                        .position(|column| column.eq_ignore_ascii_case(name))
                        .and_then(|index| row.and_then(|row| row.get(index)))
                        .and_then(Option::as_deref)
                        .unwrap_or("")
                        .to_owned()
                };
                let index_type = value("Type");
                let comment = value("Comment");
                let unique = match (value("Non_unique").as_str(), value("unique").as_str()) {
                    ("0", _) | (_, "1") => "indexes.unique",
                    ("1", _) | (_, "0") => "indexes.non_unique",
                    _ => "indexes.unknown_unique",
                };
                let heading = tr(
                    cx,
                    "indexes.properties",
                    &[
                        ("table", &object.parent),
                        ("index", &object.name),
                        (
                            "type",
                            if index_type.is_empty() {
                                "—"
                            } else {
                                &index_type
                            },
                        ),
                        ("unique", &t(cx, unique)),
                    ],
                );
                body.child(div().text_sm().whitespace_normal().child(heading))
                    .when(!comment.is_empty(), |body| {
                        body.child(div().text_xs().whitespace_normal().child(comment))
                    })
                    .child(
                        div()
                            .h(px(190.))
                            .min_h_0()
                            .child(query_grid::view(self.detail_grid.as_ref().unwrap(), cx)),
                    )
            })
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(div().flex_1().text_xs().child(tr(
                        cx,
                        "indexes.count",
                        &[
                            ("count", &self.objects.len().to_string()),
                            ("drafts", &self.drafts.len().to_string()),
                        ],
                    )))
                    .child(
                        Button::new("index-preview")
                            .primary()
                            .label(t(cx, "compare.generate_sql"))
                            .disabled(self.drafts.is_empty())
                            .on_click(cx.listener(|this, _, window, cx| this.preview(window, cx))),
                    ),
            )
    }
}

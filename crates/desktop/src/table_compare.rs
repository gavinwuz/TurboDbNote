use crate::{
    database::{DatabaseView, OpenTable},
    feedback,
    locale::{t, tr},
    query_grid::QueryGrid,
};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Disableable, IconName, Selectable, StyledExt, WindowExt,
    button::{Button, ButtonVariants},
    input::{Input, InputState},
    menu::DropdownMenu,
    popover::Popover,
    select::{SearchableVec, Select, SelectEvent, SelectState},
    table::TableState,
};
use turbodbn_services::{
    DatabaseKind,
    database::{
        comparison,
        workbench::{Action, PageOptions, Reply, Rows, Schema},
    },
};
struct Side {
    database_picker: Entity<SelectState<SearchableVec<String>>>,
    table_picker: Option<Entity<crate::ddl_picker::Picker>>,
    table_subscription: Option<Subscription>,
    picker_open: bool,
    target: OpenTable,
    database: Entity<InputState>,
    schema: Entity<InputState>,
    table: Entity<InputState>,
}
pub struct Comparison {
    source: WeakEntity<DatabaseView>,
    sides: [Side; 2],
    structure: bool,
    busy: bool,
    revision: u64,
    locked: std::rc::Rc<std::cell::Cell<bool>>,
    scroll_subscriptions: Vec<Subscription>,
    notice: String,
    grids: [Option<Entity<TableState<QueryGrid>>>; 2],
    ddl: [String; 2],
    table_mode: bool,
    schemas: Option<[Schema; 2]>,
    compared_targets: Option<[OpenTable; 2]>,
    _subscriptions: Vec<Subscription>,
}
impl EventEmitter<crate::table_view::TableDraftChanged> for Comparison {}
impl Comparison {
    pub(crate) fn snapshot(&self, cx: &App) -> [OpenTable; 2] {
        std::array::from_fn(|i| {
            let mut target = self.sides[i].target.clone();
            target.database = self.sides[i].database.read(cx).value().to_string();
            target.object.schema = self.sides[i].schema.read(cx).value().to_string();
            target.object.name = self.sides[i].table.read(cx).value().to_string();
            target
        })
    }
    pub(crate) fn restore_sources(
        &mut self,
        targets: [OpenTable; 2],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for (side, target) in self.sides.iter_mut().zip(targets) {
            side.database.update(cx, |input, cx| {
                input.set_value(target.database.clone(), window, cx)
            });
            side.schema.update(cx, |input, cx| {
                input.set_value(target.object.schema.clone(), window, cx)
            });
            side.table.update(cx, |input, cx| {
                input.set_value(target.object.name.clone(), window, cx)
            });
            side.target = target;
            let mut names = self
                .source
                .upgrade()
                .map(|db| db.read(cx).note_databases(side.target.connection))
                .unwrap_or_default();
            if side.target.kind == DatabaseKind::PostgreSql {
                names.retain(|n| n == &side.target.database);
            }
            side.database_picker.update(cx, |picker, cx| {
                picker.set_items(SearchableVec::new(names), window, cx);
                picker.set_selected_index(None, window, cx);
            });
        }
        cx.notify();
    }

    pub fn new(
        source: WeakEntity<DatabaseView>,
        target: OpenTable,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let structure = target.mode == crate::table_view::Mode::StructureCompare;
        let names = source
            .upgrade()
            .map(|db| db.read(cx).note_databases(target.connection))
            .unwrap_or_default();
        let sides = std::array::from_fn(|_| Side {
            database_picker: cx.new(|cx| {
                SelectState::new(SearchableVec::new(names.clone()), None, window, cx)
                    .searchable(true)
            }),
            table_picker: None,
            table_subscription: None,
            picker_open: false,
            database: cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(target.database.clone())
                    .placeholder(t(cx, "compare.database"))
            }),
            schema: cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(target.object.schema.clone())
                    .placeholder(t(cx, "compare.schema"))
            }),
            table: cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(target.object.name.clone())
                    .placeholder(t(cx, "compare.table"))
            }),
            target: target.clone(),
        });
        let mut subscriptions = vec![];
        for (i, side) in sides.iter().enumerate() {
            for input in [&side.database, &side.schema, &side.table] {
                let mut previous = input.read(cx).value();
                subscriptions.push(cx.subscribe(input, move |this, input, event, cx| {
                    if matches!(event, gpui_component::input::InputEvent::Change) {
                        let value = input.read(cx).value();
                        if value == previous {
                            return;
                        }
                        previous = value;
                        this.invalidate();
                        cx.emit(crate::table_view::TableDraftChanged);
                        cx.notify();
                    }
                }));
            }
            subscriptions.push(cx.subscribe_in(
                &side.database_picker,
                window,
                move |this, _, event: &SelectEvent<SearchableVec<String>>, window, cx| {
                    if i == 0 || this.busy {
                        return;
                    }
                    if let SelectEvent::Confirm(Some(value)) = event {
                        this.sides[i]
                            .database
                            .update(cx, |input, cx| input.set_value(value.clone(), window, cx));
                        this.sides[i].table_picker = None;
                        this.sides[i].table_subscription = None;
                        this.invalidate();
                        cx.emit(crate::table_view::TableDraftChanged);
                        cx.notify();
                    }
                },
            ));
        }
        Self {
            source,
            sides,
            structure,
            busy: false,
            revision: 0,
            locked: std::rc::Rc::new(std::cell::Cell::new(true)),
            scroll_subscriptions: vec![],
            notice: String::new(),
            grids: [None, None],
            ddl: [String::new(), String::new()],
            table_mode: true,
            schemas: None,
            compared_targets: None,
            _subscriptions: subscriptions,
        }
    }
    fn invalidate(&mut self) {
        self.revision += 1;
        self.grids = [None, None];
        self.ddl = [String::new(), String::new()];
        self.schemas = None;
        self.compared_targets = None;
        self.notice.clear();
        self.scroll_subscriptions.clear();
        for side in &mut self.sides {
            side.table_picker = None;
            side.table_subscription = None;
            side.picker_open = false;
        }
    }
    fn open_table_picker(
        &mut self,
        i: usize,
        open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if i == 0 || self.busy {
            return;
        }
        self.sides[i].picker_open = open;
        if open && let Some(source) = self.source.upgrade() {
            let context = (
                self.sides[i].target.connection,
                self.sides[i].database.read(cx).value().trim().to_string(),
                source
                    .read(cx)
                    .note_generation(self.sides[i].target.connection),
            );
            if self.sides[i]
                .table_picker
                .as_ref()
                .is_some_and(|p| !p.read(cx).scope_matches(&context))
            {
                self.sides[i].table_picker = None;
                self.sides[i].table_subscription = None;
            }
        }
        if open && self.sides[i].table_picker.is_none() {
            let db = self.sides[i].database.read(cx).value().trim().to_string();
            let id = self.sides[i].target.connection;
            let Some(source) = self.source.upgrade() else {
                return;
            };
            if db.is_empty() || source.read(cx).note_connection_state(id) != Some(true) {
                feedback::error(t(cx, "compare.connect_first"), cx);
                self.sides[i].picker_open = false;
                return;
            }
            let generation = source.read(cx).note_generation(id);
            let paged = source.read(cx).paged_catalog_supported(id);
            let picker = cx.new(|cx| {
                crate::ddl_picker::Picker::new(
                    self.source.clone(),
                    id,
                    db,
                    generation,
                    paged,
                    window,
                    cx,
                )
                .inline()
            });
            self.sides[i].table_subscription = Some(cx.subscribe_in(
                &picker,
                window,
                move |this, _, event: &crate::ddl_picker::Selected, window, cx| {
                    let side = &this.sides[i];
                    if this.busy
                        || event.scope.0 != side.target.connection
                        || event.scope.1 != side.database.read(cx).value().as_ref()
                        || this.source.upgrade().is_none_or(|db| {
                            db.read(cx).note_generation(event.scope.0) != event.scope.2
                        })
                    {
                        return;
                    }
                    this.sides[i].target.object = event.object.clone();
                    this.sides[i].table.update(cx, |input, cx| {
                        input.set_value(event.object.name.clone(), window, cx)
                    });
                    this.sides[i].schema.update(cx, |input, cx| {
                        input.set_value(event.object.schema.clone(), window, cx)
                    });
                    this.invalidate();
                    cx.emit(crate::table_view::TableDraftChanged);
                    cx.notify();
                },
            ));
            self.sides[i].table_picker = Some(picker);
        }
        if open && let Some(picker) = self.sides[i].table_picker.clone() {
            cx.defer_in(window, move |_, window, cx| {
                picker.update(cx, |picker, cx| picker.focus_search(window, cx))
            });
        }
        cx.notify();
    }
    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(source) = self.source.upgrade() else {
            return;
        };
        let mut targets = vec![];
        for side in &self.sides {
            let mut target = side.target.clone();
            target.database = side.database.read(cx).value().trim().to_owned();
            target.object.schema = side.schema.read(cx).value().trim().to_owned();
            target.object.name = side.table.read(cx).value().trim().to_owned();
            if target.database.is_empty()
                || target.object.name.is_empty()
                || target.object.name.len() > 256
                || target.database.len() > 256
                || target.object.schema.len() > 256
            {
                feedback::error(t(cx, "compare.required"), cx);
                return;
            }
            let db = source.read(cx);
            let Some(profile) = db.note_profile(target.connection) else {
                return;
            };
            if !matches!(
                profile.kind,
                DatabaseKind::MySql
                    | DatabaseKind::PostgreSql
                    | DatabaseKind::Sqlite
                    | DatabaseKind::ClickHouse
            ) {
                feedback::error(t(cx, "compare.unsupported"), cx);
                return;
            }
            if profile.kind == DatabaseKind::PostgreSql && target.database != profile.database {
                feedback::error(t(cx, "compare.postgres_database"), cx);
                return;
            }
            if db.note_connection_state(target.connection) != Some(true) {
                feedback::error(t(cx, "compare.connect_first"), cx);
                return;
            }
            target.generation = db.note_generation(target.connection);
            target.kind = profile.kind;
            target.mysql_raw_gbk = profile.mysql_raw_gbk;
            targets.push(target);
        }
        self.busy = true;
        self.notice.clear();
        self.grids = [None, None];
        self.ddl = [String::new(), String::new()];
        self.schemas = None;
        self.compared_targets = None;
        cx.notify();
        let structure = self.structure;
        let revision = self.revision;
        cx.spawn_in(window, async move |view, cx| {
            let mut replies = vec![];
            let mut failure = None;
            for target in &targets {
                let action = if structure {
                    Action::Describe(target.object.clone())
                } else {
                    Action::GridPage {
                        object: target.object.clone(),
                        options: PageOptions {
                            page_size: 200,
                            ..Default::default()
                        },
                    }
                };
                let rx = cx.update(|_, cx| {
                    source.update(cx, |source, cx| {
                        source.mcp_request(
                            target.connection,
                            target.generation,
                            target.database.clone(),
                            action,
                            cx,
                        )
                    })
                });
                let result = match rx {
                    Ok(rx) => rx.recv().await.unwrap_or_else(|_| Err("mcp.stale".into())),
                    Err(_) => return,
                };
                match result {
                    Ok(reply) => replies.push(reply),
                    Err(error) => {
                        failure = Some(error);
                        break;
                    }
                }
            }
            let _ = cx.update(|window, cx| {
                view.update(cx, |this, cx| {
                    this.busy = false;
                    if this.revision != revision {
                        cx.notify();
                        return;
                    }
                    if targets.iter().any(|target| {
                        source.read(cx).note_generation(target.connection) != target.generation
                            || source.read(cx).note_connection_state(target.connection)
                                != Some(true)
                    }) {
                        this.notice = t(cx, "dbwork.context.changed").to_string();
                        cx.notify();
                        return;
                    }
                    if let Some(error) = failure {
                        this.notice = crate::locale::message(cx, &error).to_string();
                        feedback::error(this.notice.clone(), cx);
                        cx.notify();
                        return;
                    }
                    let mut values: Vec<(Rows, Schema, bool)> = vec![];
                    for reply in replies {
                        match reply {
                            Reply::Schema(s) => values.push((Rows::default(), s, false)),
                            Reply::GridPage {
                                rows, schema, more, ..
                            } => values.push((rows, schema, more)),
                            _ => {
                                this.notice = t(cx, "mcp.failed").to_string();
                                cx.notify();
                                return;
                            }
                        }
                    }
                    if values.len() != 2 {
                        return;
                    }
                    this.ddl = [values[0].1.ddl.clone(), values[1].1.ddl.clone()];
                    this.schemas = Some([values[0].1.clone(), values[1].1.clone()]);
                    this.compared_targets = Some([targets[0].clone(), targets[1].clone()]);
                    let result = if structure && this.table_mode {
                        comparison::fields(&values[0].1, &values[1].1)
                    } else if structure {
                        comparison::ddl(&this.ddl[0], &this.ddl[1])
                    } else {
                        Ok(comparison::data(
                            &values[0].0,
                            &values[0].1,
                            &values[1].0,
                            &values[1].1,
                        ))
                    };
                    match result {
                        Ok(compared) => {
                            this.notice = if structure {
                                tr(
                                    cx,
                                    "compare.ddl_summary",
                                    &[("different", &compared.different.to_string())],
                                )
                            } else {
                                tr(
                                    cx,
                                    if compared.keyed {
                                        "compare.key_summary"
                                    } else {
                                        "compare.row_summary"
                                    },
                                    &[
                                        ("left", &values[0].0.values.len().to_string()),
                                        ("right", &values[1].0.values.len().to_string()),
                                        ("different", &compared.different.to_string()),
                                    ],
                                )
                            }
                            .to_string();
                            if !structure && (values[0].2 || values[1].2) {
                                this.notice
                                    .push_str(&format!(" · {}", t(cx, "compare.more")));
                            }
                            this.set_grids(compared, window, cx);
                        }
                        Err(error) => this.notice = crate::locale::message(cx, &error).to_string(),
                    }
                    cx.notify();
                })
            });
        })
        .detach();
    }
    fn set_grids(
        &mut self,
        compared: comparison::Compared,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.structure {
            self.notice = tr(
                cx,
                "compare.ddl_summary",
                &[("different", &compared.different.to_string())],
            )
            .to_string();
        }
        let width = compared
            .left
            .values
            .iter()
            .chain(&compared.right.values)
            .filter_map(|r| r.get(1).and_then(Option::as_ref))
            .map(|s| unicode_width::UnicodeWidthStr::width(s.as_str()))
            .max()
            .unwrap_or(80)
            .saturating_mul(8)
            .clamp(800, 32700) as f32;
        let mut side_index = 0;
        let grids = [compared.left, compared.right].map(|mut rows| {
            if self.structure && self.table_mode {
                for (name, key) in rows.columns.iter_mut().skip(1).zip([
                    "ddl.column.name",
                    "grid.structure.type",
                    "ddl.column.not_null",
                    "ddl.column.auto_increment",
                    "ddl.column.primary",
                    "ddl.column.extra",
                    "ddl.column.comment",
                ]) {
                    *name = t(cx, key).to_string();
                }
            }
            let side = side_index;
            side_index += 1;
            cx.new(|cx| {
                TableState::new(
                    if self.structure && !self.table_mode {
                        QueryGrid::new(rows, 0)
                            .with_column_widths(&[52., width])
                            .with_differences(side)
                    } else if self.structure {
                        QueryGrid::new(rows, 0)
                            .with_column_widths(&[52., 160., 160., 70., 70., 70., 220., 280.])
                            .with_differences(side)
                    } else {
                        QueryGrid::new(rows, 0).with_differences(side)
                    },
                    window,
                    cx,
                )
            })
        });
        let link = std::rc::Rc::new(crate::query_grid::ScrollLink::new(
            &grids,
            self.locked.clone(),
            cx,
        ));
        self.scroll_subscriptions.clear();
        for (i, grid) in grids.iter().enumerate() {
            grid.update(cx, |grid, _| {
                grid.delegate_mut().scroll_link = Some((link.clone(), i))
            });
            let link = link.clone();
            self.scroll_subscriptions
                .push(cx.observe(grid, move |_, _, cx| {
                    if link.synchronize(i) {
                        cx.notify();
                    }
                }));
        }
        self.grids = grids.map(Some);
    }
    fn toggle_structure(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.table_mode = !self.table_mode;
        if let Some(schemas) = &self.schemas {
            let result = if self.table_mode {
                comparison::fields(&schemas[0], &schemas[1])
            } else {
                comparison::ddl(&self.ddl[0], &self.ddl[1])
            };
            match result {
                Ok(result) => self.set_grids(result, window, cx),
                Err(error) => feedback::error(crate::locale::message(cx, &error), cx),
            }
        }
        cx.notify();
    }
    fn sync_preview(&self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(schemas), Some(targets)) = (&self.schemas, &self.compared_targets) else {
            return;
        };
        if targets[0].kind != targets[1].kind {
            feedback::error(t(cx, "compare.sync_cross_driver"), cx);
            return;
        }
        let draft = match comparison::sync_ddl(
            targets[0].kind,
            &targets[0].database,
            &targets[0].object,
            &schemas[0],
            &schemas[1],
        ) {
            Ok(draft) => draft,
            Err(error) => {
                feedback::error(crate::locale::message(cx, &error), cx);
                return;
            }
        };
        let issues = draft
            .issues
            .iter()
            .map(|item| {
                if matches!(item.key, "compare.issue.column" | "compare.issue.comment") {
                    tr(cx, item.key, &[("column", item.column.as_str())])
                } else {
                    t(cx, item.key)
                }
            })
            .collect::<Vec<_>>();
        let labels = targets.each_ref().map(|target| {
            let connection = self
                .source
                .upgrade()
                .and_then(|db| {
                    db.read(cx)
                        .note_profile(target.connection)
                        .map(|p| p.name.clone())
                })
                .unwrap_or_else(|| target.connection.to_string());
            format!(
                "{} / {}",
                connection,
                turbodbn_services::database::workbench::qualified(
                    target.kind,
                    &target.database,
                    &target.object
                )
            )
        });
        let direction = format!("{}  ←  {}", labels[0], labels[1]);
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .code_editor("sql")
                .default_value(draft.sql.clone())
        });
        let sql = draft.sql;
        window.open_dialog(cx, move |dialog, _, cx| {
            let sql = sql.clone();
            dialog
                .title(t(cx, "compare.sync_title"))
                .w(px(820.))
                .overlay_closable(false)
                .child(
                    div()
                        .v_flex()
                        .gap_3()
                        .child(direction.clone())
                        .child(t(cx, "compare.sync_hint"))
                        .when(draft.incomplete, |el| {
                            el.child(
                                div()
                                    .text_color(cx.theme().warning)
                                    .child(t(cx, "compare.sync_incomplete")),
                            )
                        })
                        .when(!issues.is_empty(), |el| {
                            el.child(
                                div()
                                    .id("sync-manual-review")
                                    .v_flex()
                                    .gap_1()
                                    .max_h(px(120.))
                                    .overflow_y_scroll()
                                    .children(issues.iter().map(|message| {
                                        div().text_sm().whitespace_normal().child(message.clone())
                                    })),
                            )
                        })
                        .child(Input::new(&input).disabled(true).h(px(360.)))
                        .child(
                            feedback::actions().child(feedback::close_button(cx)).child(
                                Button::new("copy-sync-ddl")
                                    .icon(IconName::Copy)
                                    .label(t(cx, "dbwork.copy.ddl"))
                                    .primary()
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            sql.clone(),
                                        ));
                                        feedback::success(t(cx, "feedback.copied"), cx);
                                    }),
                            ),
                        ),
                )
        });
    }
    fn side(&self, i: usize, cx: &Context<Self>) -> AnyElement {
        let side = &self.sides[i];
        let source = self.source.upgrade();
        let profile = source
            .as_ref()
            .and_then(|s| s.read(cx).note_profile(side.target.connection))
            .cloned();
        let profiles = source
            .as_ref()
            .map(|s| {
                s.read(cx)
                    .note_profiles()
                    .into_iter()
                    .filter(|p| {
                        s.read(cx).note_connection_state(p.id) == Some(true)
                            && matches!(
                                p.kind,
                                DatabaseKind::MySql
                                    | DatabaseKind::PostgreSql
                                    | DatabaseKind::Sqlite
                                    | DatabaseKind::ClickHouse
                            )
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let owner = cx.entity().downgrade();
        let busy = self.busy;
        let picker = Button::new(("compare-connection", i))
            .ghost()
            .justify_start()
            .w_full()
            .disabled(busy || i == 0)
            .dropdown_caret(true)
            .when_some(profile.clone(), |b, p| {
                b.child(crate::connection_choice::identity(&p))
            })
            .dropdown_menu(move |mut menu, _, _| {
                for profile in &profiles {
                    let profile = profile.clone();
                    let owner = owner.clone();
                    menu = menu.item(crate::connection_choice::item(&profile).on_click(
                        move |_, window, cx| {
                            let _ = owner.update(cx, |this, cx| {
                                if this.busy || i == 0 {
                                    return;
                                }
                                let side = &mut this.sides[i];
                                side.target.connection = profile.id;
                                side.target.kind = profile.kind;
                                side.database.update(cx, |input, cx| {
                                    input.set_value(
                                        if profile.kind == DatabaseKind::Sqlite
                                            && profile.database.is_empty()
                                        {
                                            "main".into()
                                        } else {
                                            profile.database.clone()
                                        },
                                        window,
                                        cx,
                                    )
                                });
                                side.schema.update(cx, |input, cx| {
                                    input.set_value(
                                        if profile.kind == DatabaseKind::PostgreSql {
                                            "public"
                                        } else {
                                            ""
                                        },
                                        window,
                                        cx,
                                    )
                                });
                                let mut names = this
                                    .source
                                    .upgrade()
                                    .map(|db| db.read(cx).note_databases(profile.id))
                                    .unwrap_or_default();
                                if profile.kind == DatabaseKind::PostgreSql {
                                    names.retain(|n| n == &profile.database);
                                }
                                this.sides[i].database_picker.update(cx, |picker, cx| {
                                    picker.set_items(SearchableVec::new(names), window, cx)
                                });
                                this.invalidate();
                                cx.emit(crate::table_view::TableDraftChanged);
                                cx.notify();
                            });
                        },
                    ));
                }
                menu.scrollable(true).max_h(px(320.))
            });
        let mut body =
            div()
                .v_flex()
                .flex_1()
                .h_full()
                .min_w_0()
                .min_h_0()
                .gap_2()
                .child(
                    div()
                        .h(px(24.))
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(t(
                            cx,
                            if i == 0 {
                                "compare.current"
                            } else {
                                "compare.target"
                            },
                        )),
                )
                .child(picker)
                .child(
                    div()
                        .h_flex()
                        .w_full()
                        .min_w_0()
                        .gap_1()
                        .child(div().flex_1().min_w_0().child(
                            Input::new(&side.database).cleanable(true).disabled(
                                busy || i == 0
                                    || profile.as_ref().is_some_and(|p| {
                                        matches!(
                                            p.kind,
                                            DatabaseKind::PostgreSql | DatabaseKind::Sqlite
                                        )
                                    }),
                            ),
                        ))
                        .child(
                            div().w(px(32.)).flex_shrink_0().child(
                                Select::new(&side.database_picker)
                                    .w_full()
                                    .menu_width(px(320.))
                                    .placeholder("")
                                    .search_placeholder(t(cx, "dbwork.database.filter"))
                                    .disabled(
                                        busy || i == 0
                                            || profile.as_ref().is_some_and(|p| {
                                                matches!(
                                                    p.kind,
                                                    DatabaseKind::PostgreSql | DatabaseKind::Sqlite
                                                )
                                            }),
                                    ),
                            ),
                        ),
                )
                .when(
                    self.sides
                        .iter()
                        .any(|s| s.target.kind == DatabaseKind::PostgreSql),
                    |b| {
                        b.child(Input::new(&side.schema).cleanable(true).disabled(
                            busy || i == 0 || side.target.kind != DatabaseKind::PostgreSql,
                        ))
                    },
                )
                .child(
                    div()
                        .h_flex()
                        .w_full()
                        .min_w_0()
                        .gap_1()
                        .child(
                            div().flex_1().min_w_0().child(
                                Input::new(&side.table)
                                    .cleanable(true)
                                    .disabled(busy || i == 0),
                            ),
                        )
                        .child(
                            Popover::new(("compare-table-picker", i))
                                .open(side.picker_open)
                                .trigger(
                                    Button::new(("compare-table-arrow", i))
                                        .ghost()
                                        .icon(IconName::ChevronDown)
                                        .tooltip(t(cx, "ddl.select_table"))
                                        .disabled(busy || i == 0),
                                )
                                .on_open_change({
                                    let owner = cx.entity().downgrade();
                                    move |open, window, cx| {
                                        let _ = owner.update(cx, |this, cx| {
                                            this.open_table_picker(i, *open, window, cx)
                                        });
                                    }
                                })
                                .content({
                                    let owner = cx.entity().downgrade();
                                    move |_, _, cx| {
                                        owner
                                            .upgrade()
                                            .and_then(|owner| {
                                                owner.read(cx).sides[i].table_picker.clone()
                                            })
                                            .map(|picker| div().child(picker))
                                            .unwrap_or_else(div)
                                    }
                                }),
                        ),
                );
        if self.structure {
            body = body.child(
                Button::new(("compare-copy-ddl", i))
                    .ghost()
                    .icon(IconName::Copy)
                    .label(t(cx, "dbwork.copy.ddl"))
                    .disabled(self.ddl[i].is_empty())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(this.ddl[i].clone()));
                        feedback::success(t(cx, "feedback.copied"), cx);
                    })),
            );
        }
        if let Some(grid) = &self.grids[i] {
            body = body.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(crate::query_grid::view(grid, cx)),
            );
        }
        body.into_any_element()
    }
}
impl Render for Comparison {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .v_flex()
            .min_w_0()
            .gap_2()
            .p_3()
            .child(
                div()
                    .h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new("compare-start")
                            .icon(IconName::ArrowRight)
                            .label(t(cx, "compare.run"))
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, w, cx| this.start(w, cx))),
                    )
                    .child(
                        Button::new("compare-sync")
                            .ghost()
                            .icon(IconName::Replace)
                            .label(t(cx, "compare.sync_scroll"))
                            .selected(self.locked.get())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.locked.set(!this.locked.get());
                                if this.locked.get()
                                    && let Some(grid) = &this.grids[0]
                                    && let Some((link, _)) = &grid.read(cx).delegate().scroll_link
                                {
                                    link.synchronize(0);
                                }
                                cx.notify();
                            })),
                    )
                    .when(self.structure, |el| {
                        el.child(
                            Button::new("compare-display")
                                .ghost()
                                .label(t(
                                    cx,
                                    if self.table_mode {
                                        "compare.show_ddl"
                                    } else {
                                        "compare.show_fields"
                                    },
                                ))
                                .disabled(self.busy)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.toggle_structure(window, cx)
                                })),
                        )
                        .child(
                            Button::new("compare-sync-preview")
                                .ghost()
                                .icon(IconName::Copy)
                                .label(t(cx, "compare.sync_title"))
                                .disabled(self.busy || self.schemas.is_none())
                                .on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.sync_preview(window, cx)
                                    }),
                                ),
                        )
                    })
                    .child(
                        div()
                            .text_sm()
                            .flex_1()
                            .min_w_0()
                            .whitespace_normal()
                            .text_color(cx.theme().muted_foreground)
                            .child(t(
                                cx,
                                if self.structure && self.table_mode {
                                    "compare.fields_hint"
                                } else if self.structure {
                                    "compare.ddl_hint"
                                } else {
                                    "compare.data_hint"
                                },
                            )),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(t(cx, "compare.diff_legend")),
            )
            .when(self.busy, |b| b.child(t(cx, "ddl.loading")))
            .when(!self.notice.is_empty(), |b| {
                b.child(
                    div()
                        .text_sm()
                        .whitespace_normal()
                        .child(self.notice.clone()),
                )
            })
            .child(
                div()
                    .h_flex()
                    .items_start()
                    .gap_3()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(self.side(0, cx))
                    .child(self.side(1, cx)),
            )
    }
}

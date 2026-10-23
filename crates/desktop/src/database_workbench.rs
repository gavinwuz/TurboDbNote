use super::*;
use crate::locale::t;
use gpui_component::WindowExt;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct OpenTable {
    pub connection: Uuid,
    pub database: String,
    pub object: Object,
    pub mode: crate::table_view::Mode,
    #[serde(skip)]
    pub generation: u64,
    pub kind: DatabaseKind,
    #[serde(skip)]
    pub writable: bool,
    #[serde(skip)]
    pub mysql_raw_gbk: bool,
}
pub(super) const TABLE_MENU_ACTIONS: &[(&str, &str, Option<crate::table_view::Mode>)] = &[
    ("designer.title", "", Some(crate::table_view::Mode::Design)),
    ("designer.right", "structure", None),
    ("dbwork.data", "", Some(crate::table_view::Mode::Grid)),
    ("dbwork.note", "", Some(crate::table_view::Mode::Note)),
    ("dbwork.er", "", Some(crate::table_view::Mode::Er)),
    ("", "", None),
    ("dbwork.rename", "rename", None),
    ("dbwork.copy.name", "name", None),
    ("dbwork.copy.ddl", "ddl", None),
    ("dbwork.compare", "", Some(crate::table_view::Mode::Compare)),
    (
        "compare.structure",
        "",
        Some(crate::table_view::Mode::StructureCompare),
    ),
    ("dbwork.import", "import", None),
    ("dbwork.export", "export", None),
    ("", "", None),
    ("dbwork.delete", "delete", None),
];
impl EventEmitter<OpenTable> for DatabaseView {}
impl DatabaseView {
    pub(crate) fn mongo_context_current(&self, id: Uuid, generation: u64) -> bool {
        self.generation.get(&id).copied() == Some(generation)
            && self.states.get(&id).is_some_and(|s| s == "已连接")
    }
    pub(crate) fn mongo_write_allowed(&self, id: Uuid, generation: u64) -> bool {
        self.generation.get(&id).copied() == Some(generation)
            && self.states.get(&id).is_some_and(|s| s == "已连接")
            && self
                .profiles
                .iter()
                .any(|p| p.id == id && p.driver_id == "mongodb" && !p.mongo_read_only)
    }
    pub(crate) fn mongo_context_label(&self, id: Uuid) -> String {
        self.profiles
            .iter()
            .find(|p| p.id == id)
            .map(|p| format!("{} · {}", p.name, p.environment))
            .unwrap_or_default()
    }

    pub(super) fn cancel_table_request(&mut self, cx: &mut Context<Self>) {
        if let Some(sender) = self.mcp_target.take() {
            let _ = sender.try_send(Err("dbwork.session.closed".into()));
        }
        self.defer_mcp(cx);
        if let Some(view) = self.workbench_target.take() {
            let _ = view.update(cx, |this, cx| {
                this.receive(
                    Err("操作已取消 / 连接已断开；写入结果请重连确认".into()),
                    cx,
                )
            });
        }
        self.pending_object_action = None;
    }

    pub(super) fn object_key(object: &Object) -> String {
        crate::object_browser::key(object)
    }
    pub(crate) fn table_request(
        &mut self,
        id: Uuid,
        generation: u64,
        request: workbench::Request,
        target: WeakEntity<crate::table_view::TableView>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.generation.get(&id).copied().unwrap_or_default() != generation {
            return Err("连接已重新建立，请关闭并重新打开此数据页".into());
        }
        if matches!(request.action, Action::MongoWrite(_))
            && !self.mongo_write_allowed(id, generation)
        {
            return Err("mongo.document.denied".into());
        }
        if matches!(request.action, Action::Commit { .. })
            && self
                .profiles
                .iter()
                .any(|p| p.id == id && p.kind == DatabaseKind::Sqlite && p.sqlite_read_only)
        {
            return Err("当前连接为只读".into());
        }
        let rx = self.mcp_request(id, generation, request.database, request.action, cx);
        cx.spawn(async move |_, cx| {
            let result = rx.recv().await.unwrap_or_else(|_| Err("mcp.stale".into()));
            let _ = target.update(cx, |table, cx| table.receive(result, cx));
        })
        .detach();
        Ok(())
    }
    pub(super) fn describe_object(&mut self, object: Object, cx: &mut Context<Self>) {
        if let Some((id, database)) = self.selected_database.clone() {
            self.request_operation(
                id,
                OperationKind::Workbench(Box::new(workbench::Request {
                    database,
                    action: Action::Describe(object),
                })),
                cx,
            );
        }
    }
    fn open_table(
        &mut self,
        object: Object,
        mode: crate::table_view::Mode,
        cx: &mut Context<Self>,
    ) {
        if let Some((id, database)) = self.selected_database.clone()
            && let Some(p) = self.profiles.iter().find(|p| p.id == id)
        {
            cx.emit(OpenTable {
                connection: id,
                database,
                object,
                mode,
                generation: self.generation.get(&id).copied().unwrap_or_default(),
                kind: p.kind,
                writable: p.supports_mutation(),
                mysql_raw_gbk: p.mysql_raw_gbk,
            });
        }
    }
    pub(super) fn render_object_tree(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some((object, action)) = self.pending_object_action.clone()
            && self.browser.full.contains(&Self::object_key(&object))
        {
            self.pending_object_action = None;
            cx.defer_in(window, move |this, window, cx| {
                this.object_action(object, &action, window, cx)
            });
        }
        if let Some((object, column)) = self.browser.pending_edit.clone()
            && self.browser.full.contains(&Self::object_key(&object))
            && let Some(schema) = self.schemas.get(&Self::object_key(&object))
        {
            let ddl = schema.ddl.clone();
            self.browser.pending_edit = None;
            let context = self.selected_database.clone();
            let generation = context.as_ref().map(|(id, _)| self.note_generation(*id));
            cx.defer_in(window, move |this, window, cx| {
                if this.selected_database != context
                    || context.as_ref().map(|(id, _)| this.note_generation(*id)) != generation
                {
                    return;
                }
                this.edit_column(object, column, ddl, window, cx)
            });
        }
        let busy = self
            .active_operation
            .as_ref()
            .is_some_and(|(_, _, kind)| !super::browser_ui::is_request(kind));
        let source = cx.entity().downgrade();
        let selected = self.selected_database.clone();
        let profile = selected
            .as_ref()
            .and_then(|(id, _)| self.profiles.iter().find(|p| p.id == *id))
            .cloned();
        let categories = profile
            .as_ref()
            .map(|p| {
                let version = self
                    .info
                    .get(&p.id)
                    .map(|i| i.version.as_str())
                    .unwrap_or_default();
                turbodbn_services::database::workbench::categories(p.kind, version)
            })
            .unwrap_or_default();
        let create_categories = categories.clone();
        let create_database = profile
            .as_ref()
            .filter(|p| p.kind.can_create_database() && p.supports_mutation())
            .map(|p| p.id);
        let can_copy = profile.as_ref().is_some_and(|p| {
            p.supports_mutation()
                && matches!(
                    p.kind,
                    DatabaseKind::MySql | DatabaseKind::PostgreSql | DatabaseKind::Sqlite
                )
        });
        let connected = selected
            .as_ref()
            .is_some_and(|(id, _)| self.states.get(id).is_some_and(|state| state == "已连接"));
        let writable = profile.as_ref().is_some_and(|p| p.supports_mutation());
        let relational = profile
            .as_ref()
            .is_some_and(|p| !matches!(p.kind, DatabaseKind::Redis | DatabaseKind::External));
        let copy_source = selected.clone();
        let mut body = div()
            .v_flex()
            .size_full()
            .min_h_0()
            .border_t_1()
            .border_color(cx.theme().border)
            .p_2()
            .gap_2()
            .child(
                div()
                    .h_flex()
                    .gap_1()
                    .child(div().flex_1().child(t(cx, "dbwork.objects")))
                    .child(
                        Button::new("objects-copy-database")
                            .ghost()
                            .icon(IconName::Copy)
                            .tooltip(t(cx, "database.copy_name"))
                            .disabled(selected.is_none())
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some((_, db)) = &this.selected_database {
                                    cx.write_to_clipboard(ClipboardItem::new_string(db.clone()));
                                    feedback::success(t(cx, "feedback.copied"), cx);
                                }
                            })),
                    )
                    .child(
                        Button::new("objects-refresh")
                            .ghost()
                            .icon(crate::assets::refresh_icon())
                            .tooltip(t(cx, "dbwork.refresh"))
                            .disabled(busy || !connected)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some((id, db)) = this.selected_database.clone() {
                                    let _ = db;
                                    this.request_operation(id, OperationKind::Databases, cx);
                                }
                            })),
                    )
                    .child(
                        Button::new("objects-import")
                            .ghost()
                            .icon(gpui_component::Icon::default().path("icons/import.svg"))
                            .tooltip(t(cx, "dbwork.import"))
                            .disabled(!connected || busy || !relational || !writable)
                            .dropdown_menu({
                                let view = cx.entity().downgrade();
                                move |mut menu, _, cx| {
                                    for (key, mode) in [
                                        ("transfer.from_file", crate::transfer::Mode::ImportFile),
                                        (
                                            "transfer.from_connection",
                                            crate::transfer::Mode::ImportConnection,
                                        ),
                                    ] {
                                        let view = view.clone();
                                        menu = menu.item(PopupMenuItem::new(t(cx, key)).on_click(
                                            move |_, window, cx| {
                                                let _ = view.update(cx, |this, cx| {
                                                    this.transfer_dialog(mode, window, cx)
                                                });
                                            },
                                        ));
                                    }
                                    menu
                                }
                            }),
                    )
                    .child(
                        Button::new("objects-export")
                            .ghost()
                            .icon(gpui_component::Icon::default().path("icons/export.svg"))
                            .tooltip(t(cx, "transfer.export"))
                            .disabled(!connected || busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.transfer_dialog(crate::transfer::Mode::Export, window, cx)
                            })),
                    )
                    .child(
                        Button::new("objects-new")
                            .ghost()
                            .icon(IconName::Plus)
                            .tooltip(t(cx, "dbwork.new"))
                            .disabled(
                                !connected
                                    || busy
                                    || !(can_copy
                                        || create_database.is_some()
                                        || (relational && writable)),
                            )
                            .dropdown_menu(move |mut menu, _, cx| {
                                if let Some(id) = create_database {
                                    let view = source.clone();
                                    menu = menu
                                        .item(
                                            PopupMenuItem::new(t(cx, "dbwork.database.new"))
                                                .icon(
                                                    gpui_component::Icon::default()
                                                        .path("icons/database.svg"),
                                                )
                                                .on_click(move |_, window, cx| {
                                                    let _ = view.update(cx, |this, cx| {
                                                        this.create_database_dialog(id, window, cx)
                                                    });
                                                }),
                                        )
                                        .separator();
                                }
                                if can_copy && let Some((id, database)) = copy_source.clone() {
                                    let view = source.clone();
                                    menu = menu
                                        .item(
                                            PopupMenuItem::new(t(cx, "dbwork.database.copy"))
                                                .icon(IconName::Copy)
                                                .on_click(move |_, window, cx| {
                                                    let _ = view.update(cx, |this, cx| {
                                                        let _ = (&id, &database);
                                                        this.transfer_dialog(
                                                            crate::transfer::Mode::Copy,
                                                            window,
                                                            cx,
                                                        )
                                                    });
                                                }),
                                        )
                                        .separator();
                                }
                                for kind in create_categories.clone() {
                                    let v = source.clone();
                                    menu = menu.item(
                                        PopupMenuItem::new(t(cx, &format!("dbwork.{kind}")))
                                            .disabled(!writable)
                                            .on_click(move |_, window, cx| {
                                                let _ = v.update(cx, |this, cx| {
                                                    this.new_object(kind, window, cx)
                                                });
                                            }),
                                    );
                                }
                                menu
                            }),
                    ),
            );
        if let Some(p) = profile {
            let id = p.id;
            let names = self.catalog(&p);
            let database = selected
                .as_ref()
                .map(|(_, db)| db.clone())
                .filter(|db| !db.is_empty())
                .unwrap_or_else(|| t(cx, "dbwork.summary.unspecified").to_string());
            let status = if !connected {
                t(
                    cx,
                    if self.states.get(&id).is_some_and(|s| s.ends_with("中…")) {
                        "dbwork.summary.connecting"
                    } else {
                        "dbwork.summary.disconnected"
                    },
                )
                .to_string()
            } else if busy {
                t(cx, "dbwork.summary.loading").to_string()
            } else {
                String::new()
            };
            let summary = if status.is_empty() {
                database.clone().into()
            } else {
                crate::locale::tr(
                    cx,
                    "dbwork.summary",
                    &[("database", &database), ("status", &status)],
                )
            };
            if connected {
                let context = (id, database.clone(), names.clone());
                if self.object_database_context.as_ref() != Some(&context) {
                    let picker = cx.new(|cx| {
                        let mut state =
                            SelectState::new(SearchableVec::new(names), None, window, cx)
                                .searchable(true);
                        state.set_selected_value(&database, window, cx);
                        state
                    });
                    self.object_database_subscription = Some(cx.subscribe(
                        &picker,
                        move |this, _, event: &SelectEvent<SearchableVec<String>>, cx| {
                            if let SelectEvent::Confirm(Some(value)) = event
                                && this
                                    .selected_database
                                    .as_ref()
                                    .is_some_and(|(current, db)| *current == id && db != value)
                                && this.sessions.contains_key(&id)
                                && (this.active_operation.is_none()
                                    || this
                                        .active_operation
                                        .as_ref()
                                        .is_some_and(|(_, _, k)| super::browser_ui::is_request(k)))
                                && this
                                    .profiles
                                    .iter()
                                    .find(|p| p.id == id)
                                    .is_some_and(|p| this.catalog(p).contains(value))
                            {
                                this.select_database(id, value.clone(), cx);
                            }
                        },
                    ));
                    self.object_database_picker = Some(picker);
                    self.object_database_context = Some(context);
                }
                if let Some(picker) = &self.object_database_picker {
                    body = body.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .min_w_0()
                            .child(
                                gpui_component::Icon::default()
                                    .path("icons/database.svg")
                                    .size_4(),
                            )
                            .child(
                                div().flex_1().min_w_0().child(
                                    Select::new(picker)
                                        .appearance(false)
                                        .w_full()
                                        .placeholder(summary)
                                        .search_placeholder(t(cx, "dbwork.database.filter"))
                                        .menu_width(px(300.))
                                        .disabled(
                                            busy && !self.active_operation.as_ref().is_some_and(
                                                |(_, _, k)| super::browser_ui::is_request(k),
                                            ),
                                        ),
                                ),
                            ),
                    );
                }
            } else {
                self.object_database_picker = None;
                self.object_database_context = None;
                self.object_database_subscription = None;
                body = body.child(
                    Button::new("objects-database")
                        .ghost()
                        .icon(gpui_component::Icon::default().path("icons/database.svg"))
                        .w_full()
                        .min_w_0()
                        .overflow_hidden()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(summary.clone()),
                        )
                        .tooltip(summary)
                        .justify_start(),
                );
            }
            if !connected {
                return body.into_any_element();
            }
            if p.kind == DatabaseKind::Redis {
                return body
                    .child(self.render_redis_tree(id, database.clone(), cx))
                    .into_any_element();
            }
        }
        let all_categories = self.browser.all_categories;
        let comments = self.browser.include_comments;
        let source = cx.entity().downgrade();
        body = body.child(
            div()
                .h_flex()
                .gap_1()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Input::new(&self.object_filter).cleanable(true)),
                )
                .child(
                    Button::new("browser-search-options")
                        .ghost()
                        .icon(IconName::Ellipsis)
                        .tooltip(t(cx, "browser.options"))
                        .disabled(!self.browser.supported || !connected)
                        .dropdown_menu(move |menu, _, cx| {
                            let v = source.clone();
                            let v2 = source.clone();
                            menu.item(
                                PopupMenuItem::new(t(cx, "browser.all_categories"))
                                    .checked(all_categories)
                                    .on_click(move |_, _, cx| {
                                        let _ = v.update(cx, |this, cx| {
                                            this.browser.all_categories =
                                                !this.browser.all_categories;
                                            this.restart_browser_search(cx);
                                        });
                                    }),
                            )
                            .item(
                                PopupMenuItem::new(t(cx, "browser.include_comments"))
                                    .checked(comments)
                                    .on_click(move |_, _, cx| {
                                        let _ = v2.update(cx, |this, cx| {
                                            this.browser.include_comments =
                                                !this.browser.include_comments;
                                            this.browser.pages.clear();
                                            this.restart_browser_search(cx);
                                        });
                                    }),
                            )
                        }),
                ),
        );

        self.render_browser_list(body, &categories, window, cx)
    }
    pub(super) fn render_browser_object(
        &mut self,
        object: Object,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let category = object.kind.as_str();
        let busy = self.active_operation.is_some();
        let writable = self
            .selected_database
            .as_ref()
            .and_then(|(id, _)| self.profiles.iter().find(|p| p.id == *id))
            .is_some_and(|p| p.supports_mutation());
        let mut root = div().v_flex().w_full();
        let key = Self::object_key(&object);
        let expanded = self.expanded_objects.contains(&key);
        let target = object.clone();
        let open = object.clone();
        let menu_object = object.clone();
        let view = cx.entity().downgrade();
        let name = if !object.parent.is_empty() {
            format!("{} / {}", object.parent, object.name)
        } else if object.schema.is_empty() {
            object.name.clone()
        } else {
            format!("{}.{}", object.schema, object.name)
        };
        let table = matches!(category, "table" | "view");
        root = root.child(
            div()
                .id(SharedString::from(format!("object-{key}")))
                .h_flex()
                .pl(px(20.))
                .gap_1()
                .h(px(30.))
                .cursor_pointer()
                .child(
                    Button::new(SharedString::from(format!("expand-object-{key}")))
                        .ghost()
                        .icon(if expanded {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .disabled(!table)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let key = Self::object_key(&target);
                            if !this.expanded_objects.remove(&key) {
                                this.expanded_objects.insert(key.clone());
                                this.browser_columns(target.clone(), cx);
                            }
                            cx.notify();
                        })),
                )
                .child(if category == "index" {
                    gpui_component::Icon::default().path("icons/index.svg")
                } else {
                    gpui_component::Icon::new(IconName::GalleryVerticalEnd)
                })
                .child(
                    div()
                        .id(SharedString::from(format!("table-name-scope-{key}")))
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .child(
                            div()
                                .h_full()
                                .w_full()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(name)
                                .map(|label| {
                                    if table {
                                        crate::context_menu::scoped(
                                            SharedString::from(format!("table-menu-{key}")),
                                            label,
                                            move |mut menu, _, cx| {
                                                for &(label, action, mode) in TABLE_MENU_ACTIONS {
                                                    if label.is_empty() {
                                                        menu = menu.separator();
                                                        continue;
                                                    }
                                                    let v = view.clone();
                                                    let o = menu_object.clone();
                                                    menu = menu.item(
                                                        PopupMenuItem::new(t(cx, label))
                                                            .disabled(
                                                                (!table || busy)
                                                                    && action != "name"
                                                                    || !writable
                                                                        && matches!(
                                                                            action,
                                                                            "rename"
                                                                                | "delete"
                                                                                | "import"
                                                                        ),
                                                            )
                                                            .on_click(move |_, window, cx| {
                                                                let _ = v.update(cx, |this, cx| {
                                                                    if let Some(mode) = mode {
                                                                        this.open_table(
                                                                            o.clone(),
                                                                            mode,
                                                                            cx,
                                                                        );
                                                                    } else {
                                                                        this.object_action(
                                                                            o.clone(),
                                                                            action,
                                                                            window,
                                                                            cx,
                                                                        );
                                                                    }
                                                                });
                                                            }),
                                                    );
                                                }
                                                menu
                                            },
                                        )
                                        .into_any_element()
                                    } else {
                                        label.into_any_element()
                                    }
                                }),
                        ),
                )
                .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                    if event.click_count() == 2 && table {
                        this.open_table(open.clone(), crate::table_view::Mode::Grid, cx);
                    }
                })),
        );
        if expanded && let Some(schema) = self.schemas.get(&key) {
            let column_key = format!("{key}:columns-folded");
            let columns_folded = self.expanded_objects.contains(&column_key);
            root = root.child(self.object_section_toggle(
                column_key,
                t(cx, "dbwork.columns").to_string(),
                schema.columns.len(),
                columns_folded,
                cx,
            ));
            if !columns_folded {
                for (index, column) in schema.columns.iter().enumerate() {
                    let o = object.clone();
                    let col = column.clone();
                    let sql = schema.ddl.clone();
                    root = root.child(
                        div()
                            .id(SharedString::from(format!("column-{key}-{index}")))
                            .h_flex()
                            .pl(px(60.))
                            .gap_1()
                            .h(px(28.))
                            .text_sm()
                            .cursor_pointer()
                            .child(
                                div()
                                    .w(px(28.))
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(crate::table_view::column_icon(Some(column), cx)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .child(format!("{} ({})", column.name, column.data_type)),
                            )
                            .child(
                                div()
                                    .max_w(px(110.))
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(column.comment.clone()),
                            )
                            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                                if event.click_count() == 2 && writable {
                                    this.edit_column(
                                        o.clone(),
                                        col.clone(),
                                        sql.clone(),
                                        window,
                                        cx,
                                    );
                                }
                            })),
                    );
                }
            }
            for (label, rows) in [
                ("dbwork.index", &schema.indexes),
                ("dbwork.foreign", &schema.foreign_keys),
                ("dbwork.trigger", &schema.triggers),
            ] {
                let section_key = format!("{key}:{label}");
                let folded = !self.expanded_objects.contains(&section_key);
                root = root.child(self.object_section_toggle(
                    section_key,
                    t(cx, label).to_string(),
                    rows.values.len(),
                    folded,
                    cx,
                ));
                if !folded
                    && self.browser.supported
                    && !self.browser.sections.contains(&format!("{key}:{label}"))
                {
                    root = root.child(self.browser_state_row(
                        Action::BrowseSection {
                            object: object.clone(),
                            section: label.into(),
                        },
                        cx,
                    ));
                } else if !folded {
                    let detail_icon = if label == "dbwork.index" {
                        gpui_component::Icon::default().path("icons/index.svg")
                    } else if label == "dbwork.foreign" {
                        gpui_component::Icon::new(IconName::ExternalLink)
                    } else {
                        gpui_component::Icon::new(IconName::Redo)
                    };
                    for (index, row) in rows.values.iter().enumerate() {
                        let label = row.first().cloned().flatten().unwrap_or_default();
                        let detail = row
                            .iter()
                            .filter_map(|v| v.as_deref())
                            .collect::<Vec<_>>()
                            .join(" · ");
                        root = root.child(
                            div()
                                .id(SharedString::from(format!("detail-{key}-{label}-{index}")))
                                .h_flex()
                                .gap_1()
                                .pl(px(60.))
                                .h(px(28.))
                                .child(detail_icon.clone())
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .child(label),
                                )
                                .tooltip(move |window, cx| {
                                    gpui_component::tooltip::Tooltip::new(detail.clone())
                                        .build(window, cx)
                                }),
                        );
                    }
                }
            }
        }

        if expanded && !self.browser.columns.contains(&key) && self.browser.supported {
            root = root.child(self.browser_state_row(Action::BrowseColumns(object.clone()), cx));
        }
        root.into_any_element()
    }
    fn object_section_toggle(
        &self,
        key: String,
        label: String,
        count: usize,
        folded: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let loaded = !self.browser.supported
            || key.ends_with(":columns-folded")
            || self.browser.sections.contains(&key);
        let count_label = if loaded {
            count.to_string()
        } else {
            t(cx, "browser.not_loaded").to_string()
        };
        let icon = if key.ends_with(":columns-folded") {
            gpui_component::Icon::new(IconName::GalleryVerticalEnd)
        } else if key.ends_with("dbwork.index") {
            gpui_component::Icon::default().path("icons/index.svg")
        } else if key.ends_with("dbwork.foreign") {
            gpui_component::Icon::new(IconName::ExternalLink)
        } else {
            gpui_component::Icon::new(IconName::Redo)
        };
        div()
            .pl(px(36.))
            .child(
                Button::new(SharedString::from(format!("section-{key}")))
                    .ghost()
                    .icon(if folded {
                        IconName::ChevronRight
                    } else {
                        IconName::ChevronDown
                    })
                    .child(icon)
                    .child(format!("{label} ({count_label})"))
                    .justify_start()
                    .w_full()
                    .h(px(28.))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.expanded_objects.remove(&key) {
                            this.expanded_objects.insert(key.clone());
                            if this.browser.supported {
                                for section in ["dbwork.index", "dbwork.foreign", "dbwork.trigger"]
                                {
                                    if let Some(object_key) =
                                        key.strip_suffix(&format!(":{section}"))
                                        && let Some(object) = this
                                            .catalog_objects
                                            .iter()
                                            .find(|o| Self::object_key(o) == object_key)
                                            .cloned()
                                    {
                                        this.browser_section(object, section.into(), cx);
                                        break;
                                    }
                                }
                            }
                        }
                        cx.notify();
                    })),
            )
            .into_any_element()
    }
    pub(super) fn object_action(
        &mut self,
        object: Object,
        action: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if action == "name" {
            cx.write_to_clipboard(ClipboardItem::new_string(object.name));
            feedback::success(crate::locale::t(cx, "feedback.copied"), cx);
            return;
        }
        if action == "import" {
            self.open_table(object, crate::table_view::Mode::Import, cx);
            return;
        }
        if action == "export" {
            self.open_table(object, crate::table_view::Mode::Export, cx);
            return;
        }
        if action == "structure" {
            feedback::ddl_context(
                feedback::DdlContext {
                    note: None,
                    candidates: vec![],
                    origin: None,
                    scope: self.selected_database.clone(),
                    object: Some(object),
                    open: true,
                },
                cx,
            );
            return;
        }
        let key = Self::object_key(&object);
        if !self.browser.full.contains(&key) {
            self.pending_object_action = Some((object.clone(), action.into()));
            self.browser_full(object, cx);
            return;
        }
        let Some(schema) = self.schemas.get(&key).cloned() else {
            self.pending_object_action = Some((object.clone(), action.into()));
            self.describe_object(object, cx);
            return;
        };
        if action == "structure" {
            if let Some((_, database)) = &self.selected_database {
                feedback::details(
                    crate::table_view::structure_details(database, &object, &schema),
                    cx,
                );
            }
            return;
        }
        if action == "ddl" {
            cx.write_to_clipboard(ClipboardItem::new_string(schema.ddl));
            self.object_notice = "已复制 DDL".into();
            feedback::success(crate::locale::t(cx, "feedback.copied"), cx);
            cx.notify();
            return;
        }
        let Some((id, db)) = self.selected_database.clone() else {
            return;
        };
        let Some(p) = self.profiles.iter().find(|p| p.id == id) else {
            return;
        };
        let name = workbench::qualified(p.kind, &db, &object);
        let sql = if action == "delete" {
            format!(
                "DROP {} {name};",
                if object.kind == "view" {
                    "VIEW"
                } else {
                    "TABLE"
                }
            )
        } else {
            if p.kind == DatabaseKind::Sqlite && object.kind == "view" {
                self.object_notice = t(cx, "dbwork.view.rename.unsupported").to_string();
                cx.notify();
                return;
            }
            let new_name = workbench::quote(p.kind, &format!("{}_new", object.name));
            match p.kind {
                DatabaseKind::MySql | DatabaseKind::ClickHouse => format!(
                    "RENAME TABLE {name} TO {}.{new_name};",
                    workbench::quote(p.kind, &db)
                ),
                _ => format!(
                    "ALTER {} {name} RENAME TO {new_name};",
                    if object.kind == "view" {
                        "VIEW"
                    } else {
                        "TABLE"
                    }
                ),
            }
        };
        self.sql_dialog(sql, Some((object, schema.ddl)), window, cx);
    }
    fn new_object(&mut self, kind: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some((id, db)) = self.selected_database.clone() else {
            return;
        };
        let Some(p) = self.profiles.iter().find(|p| p.id == id) else {
            return;
        };
        let table = self
            .catalog_objects
            .iter()
            .find(|o| o.kind == "table")
            .cloned()
            .unwrap_or(Object {
                schema: "public".into(),
                name: "existing_table".into(),
                kind: "table".into(),
                comment: String::new(),
                parent: String::new(),
            });
        let sql = workbench::create_template(p.kind, &db, &table, kind);
        self.sql_dialog(sql, None, window, cx);
    }
    fn edit_column(
        &mut self,
        object: Object,
        column: workbench::Column,
        ddl: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ddl.is_empty() && self.browser.supported {
            self.browser.pending_edit = Some((object.clone(), column));
            self.browser_full(object, cx);
            return;
        }
        let Some((id, db)) = self.selected_database.clone() else {
            return;
        };
        let Some(p) = self.profiles.iter().find(|p| p.id == id) else {
            return;
        };
        if !p.supports_mutation() {
            self.object_notice = t(cx, "dbwork.readonly").to_string();
            cx.notify();
            return;
        }
        let kind = p.kind;
        let generation = self.generation.get(&id).copied().unwrap_or_default();
        let name = cx.new(|cx| InputState::new(window, cx).default_value(column.name.clone()));
        let data_type =
            cx.new(|cx| InputState::new(window, cx).default_value(column.data_type.clone()));
        let comment =
            cx.new(|cx| InputState::new(window, cx).default_value(column.comment.clone()));
        let view = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let v = view.clone();
            let fields = (name.clone(), data_type.clone(), comment.clone());
            let object = object.clone();
            let old = column.clone();
            let ddl = ddl.clone();
            let db = db.clone();
            dialog
                .title(t(cx, "dbwork.column.edit"))
                .w(px(560.))
                .overlay_closable(false)
                .child(
                    div()
                        .v_flex()
                        .gap_3()
                        .child(t(cx, "dbwork.column.name"))
                        .child(Input::new(&name))
                        .child(t(cx, "dbwork.column.type"))
                        .child(Input::new(&data_type).disabled(kind == DatabaseKind::Sqlite))
                        .child(t(cx, "dbwork.column.comment"))
                        .child(Input::new(&comment).disabled(kind == DatabaseKind::Sqlite))
                        .when(kind == DatabaseKind::Sqlite, |el| {
                            el.child(t(cx, "dbwork.column.sqlite"))
                        })
                        .child(
                            feedback::actions().child(feedback::close_button(cx)).child(
                                Button::new("column-preview")
                                    .icon(IconName::Check)
                                    .primary()
                                    .label(t(cx, "dbwork.column.preview"))
                                    .on_click(move |_, window, cx| {
                                        let result = workbench::change_column(
                                            kind,
                                            &db,
                                            &object,
                                            &old,
                                            &ddl,
                                            &fields.0.read(cx).value(),
                                            &fields.1.read(cx).value(),
                                            &fields.2.read(cx).value(),
                                        );
                                        match result {
                                            Ok(sql) => {
                                                window.close_dialog(cx);
                                                let _ = v.update(cx, |this, cx| {
                                                    if this.selected_database.as_ref()
                                                        != Some(&(id, db.clone()))
                                                        || this
                                                            .generation
                                                            .get(&id)
                                                            .copied()
                                                            .unwrap_or_default()
                                                            != generation
                                                    {
                                                        this.object_notice =
                                                            t(cx, "dbwork.context.changed")
                                                                .to_string();
                                                        cx.notify();
                                                        return;
                                                    }
                                                    this.sql_dialog(
                                                        sql,
                                                        Some((object.clone(), ddl.clone())),
                                                        window,
                                                        cx,
                                                    )
                                                });
                                            }
                                            Err(error) => feedback::error(error, cx),
                                        }
                                    }),
                            ),
                        ),
                )
        });
    }
    pub(super) fn sql_dialog(
        &mut self,
        sql: String,
        guard: Option<(Object, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((id, db)) = self.selected_database.clone() else {
            return;
        };
        let generation = self.generation.get(&id).copied().unwrap_or_default();
        let view = cx.entity().downgrade();
        let target = self
            .profiles
            .iter()
            .find(|p| p.id == id)
            .map(|p| {
                format!(
                    "{} · {} / {} · {}",
                    p.name,
                    p.kind.label(),
                    db,
                    p.environment
                )
            })
            .unwrap_or_default();
        feedback::sql_preview(sql, target, window, cx, move |sql, _, cx| {
            view.update(cx, |this, cx| {
                if this.generation.get(&id).copied().unwrap_or_default() != generation {
                    feedback::error(crate::locale::t(cx, "dbwork.context.changed"), cx);
                    return false;
                }
                this.request_operation(
                    id,
                    OperationKind::Workbench(Box::new(workbench::Request {
                        database: db.clone(),
                        action: Action::Execute {
                            sql,
                            guard: guard.clone(),
                        },
                    })),
                    cx,
                )
            })
            .unwrap_or(false)
        });
    }
}

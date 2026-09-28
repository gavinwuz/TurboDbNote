use super::*;
use crate::locale::t;
use gpui_component::WindowExt;

#[derive(Clone)]
pub struct OpenTable {
    pub connection: Uuid,
    pub database: String,
    pub object: Object,
    pub mode: crate::table_view::Mode,
    pub generation: u64,
    pub kind: DatabaseKind,
    pub writable: bool,
}
impl EventEmitter<OpenTable> for DatabaseView {}
impl DatabaseView {
    pub(super) fn cancel_table_request(&mut self, cx: &mut Context<Self>) {
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
        format!(
            "{}:{}:{}:{}",
            object.schema, object.kind, object.parent, object.name
        )
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
        if matches!(request.action, Action::Commit { .. })
            && self
                .profiles
                .iter()
                .any(|p| p.id == id && p.kind == DatabaseKind::Sqlite && p.sqlite_read_only)
        {
            return Err("当前连接为只读".into());
        }
        if self.request_operation(id, OperationKind::Workbench(Box::new(request)), cx) {
            self.workbench_target = Some(target);
            Ok(())
        } else {
            Err(self.object_notice.clone())
        }
    }
    fn describe_object(&mut self, object: Object, cx: &mut Context<Self>) {
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
                writable: p.kind != DatabaseKind::Sqlite || !p.sqlite_read_only,
            });
        }
    }
    pub(super) fn render_object_tree(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some((object, action)) = self.pending_object_action.clone()
            && self.schemas.contains_key(&Self::object_key(&object))
        {
            self.pending_object_action = None;
            cx.defer_in(window, move |this, window, cx| {
                this.object_action(object, &action, window, cx)
            });
        }
        let busy = self.active_operation.is_some();
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
            .filter(|p| p.kind.can_create_database())
            .map(|p| p.id);
        let can_copy = profile.as_ref().is_some_and(|p| {
            matches!(
                p.kind,
                DatabaseKind::MySql | DatabaseKind::PostgreSql | DatabaseKind::Sqlite
            )
        });
        let connected = selected
            .as_ref()
            .is_some_and(|(id, _)| self.states.get(id).is_some_and(|state| state == "已连接"));
        let writable = profile
            .as_ref()
            .is_some_and(|p| p.kind != DatabaseKind::Sqlite || !p.sqlite_read_only);
        let relational = profile
            .as_ref()
            .is_some_and(|p| p.kind != DatabaseKind::Redis);
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
                        Button::new("objects-refresh")
                            .ghost()
                            .icon(crate::assets::refresh_icon())
                            .tooltip(t(cx, "dbwork.refresh"))
                            .disabled(busy || !connected)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some((id, db)) = this.selected_database.clone() {
                                    this.select_database(id, db, cx);
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
                                && this.active_operation.is_none()
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
                                        .disabled(busy),
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
                    .child(self.render_objects(window, cx))
                    .into_any_element();
            }
        }
        body = body.child(Input::new(&self.object_filter).cleanable(true));
        let query = self.object_filter.read(cx).value().trim().to_lowercase();
        let mut roots = Vec::new();
        for category in categories {
            let members = self
                .catalog_objects
                .iter()
                .filter(|o| {
                    o.kind == category
                        && format!("{} {} {} {}", o.schema, o.name, o.comment, o.parent)
                            .to_lowercase()
                            .contains(&query)
                })
                .cloned()
                .collect::<Vec<_>>();
            if members.is_empty() && !query.is_empty() {
                continue;
            }
            let folded = self.expanded_objects.contains(category) && query.is_empty();
            let mut root = div().v_flex().child(
                Button::new(SharedString::from(format!("object-root-{category}")))
                    .ghost()
                    .icon(if folded {
                        IconName::ChevronRight
                    } else {
                        IconName::ChevronDown
                    })
                    .label(format!(
                        "{} ({})",
                        t(cx, &format!("dbwork.{category}")),
                        members.len()
                    ))
                    .justify_start()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.expanded_objects.remove(category) {
                            this.expanded_objects.insert(category.into());
                        }
                        cx.notify();
                    })),
            );
            if !folded {
                for object in members {
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
                                    .disabled(!table || busy)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let key = Self::object_key(&target);
                                        if !this.expanded_objects.remove(&key) {
                                            this.expanded_objects.insert(key.clone());
                                            if !this.schemas.contains_key(&key) {
                                                this.describe_object(target.clone(), cx);
                                            }
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
                                                if category == "table" {
                                                    crate::context_menu::scoped(
                                                        SharedString::from(format!(
                                                            "table-menu-{key}"
                                                        )),
                                                        label,
                                                        move |mut menu, _, cx| {
                                                            for (label, mode) in [
                                    ("dbwork.data", crate::table_view::Mode::Grid),
                                    ("dbwork.note", crate::table_view::Mode::Note),
                                    ("dbwork.er", crate::table_view::Mode::Er),
                                    ("dbwork.compare", crate::table_view::Mode::Compare),
                                ] {
                                    let v = view.clone();
                                    let o = menu_object.clone();
                                    menu = menu.item(
                                        PopupMenuItem::new(t(cx, label)).disabled(!table).on_click(
                                            move |_, _, cx| {
                                                let _ = v.update(cx, |this, cx| {
                                                    this.open_table(o.clone(), mode, cx)
                                                });
                                            },
                                        ),
                                    );
                                }
                                                            for (label, action) in [
                                                                ("dbwork.structure", "structure"),
                                                                ("dbwork.rename", "rename"),
                                                                ("dbwork.copy.name", "name"),
                                                                ("dbwork.copy.ddl", "ddl"),
                                                                ("dbwork.delete", "delete"),
                                                                ("dbwork.import", "import"),
                                                                ("dbwork.export", "export"),
                                                            ] {
                                                                let v = view.clone();
                                                                let o = menu_object.clone();
                                                                menu = menu.item(
                                        PopupMenuItem::new(t(cx, label))
                                            .disabled(
                                                (!table || busy) && action != "name"
                                                    || !writable
                                                        && matches!(
                                                            action,
                                                            "rename" | "delete" | "import"
                                                        ),
                                            )
                                            .on_click(move |_, window, cx| {
                                                let _ = v.update(cx, |this, cx| {
                                                    this.object_action(
                                                        o.clone(),
                                                        action,
                                                        window,
                                                        cx,
                                                    )
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
                                    this.open_table(
                                        open.clone(),
                                        crate::table_view::Mode::Grid,
                                        cx,
                                    );
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
                                                .child(crate::table_view::column_icon(
                                                    Some(column),
                                                    cx,
                                                )),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .overflow_hidden()
                                                .text_ellipsis()
                                                .whitespace_nowrap()
                                                .child(format!(
                                                    "{} ({})",
                                                    column.name, column.data_type
                                                )),
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
                                        .on_click(cx.listener(
                                            move |this, event: &ClickEvent, window, cx| {
                                                if event.click_count() == 2 {
                                                    this.edit_column(
                                                        o.clone(),
                                                        col.clone(),
                                                        sql.clone(),
                                                        window,
                                                        cx,
                                                    );
                                                }
                                            },
                                        )),
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
                            if !folded {
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
                                            .id(SharedString::from(format!(
                                                "detail-{key}-{label}-{index}"
                                            )))
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
                                                gpui_component::tooltip::Tooltip::new(
                                                    detail.clone(),
                                                )
                                                .build(window, cx)
                                            }),
                                    );
                                }
                            }
                        }
                    }
                }
            }
            roots.push(root);
        }
        body.child(
            div()
                .id("object-tree-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .children(roots),
        )
        .when(
            selected.is_none()
                || (!self.object_notice.is_empty()
                    && self.object_notice != format!("{} 个对象", self.catalog_objects.len())),
            |body| {
                body.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(if selected.is_none() {
                            t(cx, "dbwork.select").to_string()
                        } else {
                            self.object_notice.clone()
                        }),
                )
            },
        )
        .into_any_element()
    }
    fn object_section_toggle(
        &self,
        key: String,
        label: String,
        count: usize,
        folded: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
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
                    .child(format!("{label} ({count})"))
                    .justify_start()
                    .w_full()
                    .h(px(28.))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.expanded_objects.remove(&key) {
                            this.expanded_objects.insert(key.clone());
                        }
                        cx.notify();
                    })),
            )
            .into_any_element()
    }
    fn object_action(
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
        let key = Self::object_key(&object);
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
        let Some((id, db)) = self.selected_database.clone() else {
            return;
        };
        let Some(p) = self.profiles.iter().find(|p| p.id == id) else {
            return;
        };
        if p.kind == DatabaseKind::Sqlite && p.sqlite_read_only {
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

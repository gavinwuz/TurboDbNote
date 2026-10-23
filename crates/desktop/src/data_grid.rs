use super::*;
use gpui_component::{
    Sizable,
    table::{Column as GridColumn, TableDelegate},
    tooltip::Tooltip,
};
pub(super) const ROW_NUMBER_WIDTH: f32 = 58.;
pub(super) struct GridDelegate {
    mongo: bool,
    owner: Option<WeakEntity<TableView>>,
    columns: Vec<GridColumn>,
    rows: Rc<Rows>,
    schema: Option<Rc<Schema>>,
    pub(super) selection: Option<(usize, usize)>,
    pub(super) highlight: bool,
    pub(super) highlight_color: Option<Hsla>,
    sort: Option<(String, bool)>,
    offset: usize,
    target: String,
    deleted: Vec<usize>,
    pending: Vec<usize>,
    defaults: Vec<(usize, usize)>,
    busy: bool,
    query_controls: bool,
    previews: std::collections::HashMap<(usize, usize), SharedString>,
    preview_order: std::collections::VecDeque<(usize, usize)>,
}
impl GridDelegate {
    pub fn column_names(&self) -> Vec<&str> {
        self.columns.iter().map(|c| c.key.as_ref()).collect()
    }
    pub fn set_width(&mut self, field: &str, width: f32) {
        if let Some(column) = self
            .columns
            .iter_mut()
            .skip(1)
            .find(|c| c.name.as_ref() == field)
        {
            *column = column.clone().width(px(width));
        }
    }
    pub fn new() -> Self {
        Self {
            mongo: false,
            owner: None,
            columns: vec![],
            rows: Rc::new(Rows::default()),
            schema: None,
            selection: None,
            highlight: true,
            highlight_color: None,
            sort: None,
            offset: 0,
            target: String::new(),
            deleted: vec![],
            pending: vec![],
            defaults: vec![],
            busy: false,
            query_controls: true,
            previews: Default::default(),
            preview_order: Default::default(),
        }
    }
    pub fn snapshot(view: &TableView, owner: WeakEntity<TableView>) -> Self {
        let mut columns = vec![
            GridColumn::new("__row_number", "#")
                .p_0()
                .width(px(ROW_NUMBER_WIDTH))
                .resizable(false)
                .fixed_left(),
        ];
        columns.extend(view.rows.columns.iter().enumerate().map(|(i, name)| {
            GridColumn::new(format!("data-{i}"), name.clone())
                .p_0()
                .width(px(view
                    .widths
                    .widths
                    .get(name)
                    .copied()
                    .or_else(|| view.auto_widths.get(name).copied())
                    .unwrap_or(
                        if view.target.kind == turbodbn_services::DatabaseKind::Redis {
                            if view.rows.columns.len() == 1 {
                                720.
                            } else {
                                360.
                            }
                        } else {
                            220.
                        },
                    )))
        }));
        Self {
            mongo: view.mongo.is_some(),
            owner: Some(owner),
            columns,
            rows: Rc::new(view.display_rows()),
            schema: view.schema.clone().map(Rc::new),
            selection: view.selected,
            highlight: view.highlight,
            highlight_color: view.highlight_color,
            sort: view.options.sort.clone(),
            offset: view.offset,
            target: format!("{}.{}", view.target.database, view.target.object.name),
            deleted: view
                .changes
                .iter()
                .filter(|(_, c)| c.values.is_none())
                .map(|(r, _)| *r)
                .collect(),
            pending: view.changes.keys().copied().collect(),
            defaults: view
                .changes
                .iter()
                .filter(|(_, change)| change.original.is_none())
                .flat_map(|(row, change)| {
                    view.rows
                        .columns
                        .iter()
                        .enumerate()
                        .filter(|(_, name)| {
                            change
                                .values
                                .as_ref()
                                .is_none_or(|values| !values.contains_key(*name))
                        })
                        .map(move |(col, _)| (*row, col))
                })
                .collect(),
            busy: view.busy,
            query_controls: view.query_controls(),
            previews: Default::default(),
            preview_order: Default::default(),
        }
    }
}
pub(crate) fn column_icon(c: Option<&workbench::Column>, cx: &App) -> AnyElement {
    let color = |dark, light| {
        rgb(if cx.theme().mode.is_dark() {
            dark
        } else {
            light
        })
    };
    if c.is_some_and(|c| c.primary) {
        return gpui_component::Icon::default()
            .path("icons/key.svg")
            .text_color(color(0xfbbf24, 0xb45309))
            .into_any_element();
    }
    let ty = c.map(|c| c.data_type.to_lowercase()).unwrap_or_default();
    if ty.is_empty() {
        return gpui_component::Icon::new(IconName::Menu)
            .text_color(cx.theme().muted_foreground)
            .into_any_element();
    }
    if ["objectid", "array", "object", "mixed", "null", "boolean"].contains(&ty.as_str()) {
        return div()
            .text_xs()
            .text_color(color(0x2dd4bf, 0x0f766e))
            .child(match ty.as_str() {
                "array" => "[]",
                "object" => "{}",
                "objectid" => "OID",
                "boolean" => "T/F",
                "null" => "∅",
                _ => "±",
            })
            .into_any_element();
    }
    if ["date", "time"].iter().any(|t| ty.contains(t)) {
        return gpui_component::Icon::new(IconName::Calendar)
            .text_color(color(0xc084fc, 0x7e22ce))
            .into_any_element();
    }
    let numeric = [
        "int", "numeric", "decimal", "float", "double", "real", "serial",
    ]
    .iter()
    .any(|t| ty.contains(t));
    div()
        .text_xs()
        .text_color(if numeric {
            color(0x2dd4bf, 0x0f766e)
        } else {
            color(0x60a5fa, 0x2563eb)
        })
        .child(if numeric { "123" } else { "ABC" })
        .into_any_element()
}
impl TableDelegate for GridDelegate {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }
    fn rows_count(&self, _: &App) -> usize {
        self.rows.values.len()
    }
    fn column(&self, col: usize, _: &App) -> &GridColumn {
        &self.columns[col]
    }
    fn render_th(
        &mut self,
        index: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        if index == 0 {
            return div()
                .size_full()
                .h_flex()
                .px_2()
                .child("#")
                .into_any_element();
        }
        let col = index - 1;
        let name = self.rows.columns[col].clone();
        let meta = self
            .schema
            .as_ref()
            .and_then(|s| s.columns.iter().find(|c| c.name == name));
        let tip_name = name.clone();
        let tip_schema = self.schema.clone();
        let tip_target = self.target.clone();
        let copy = name.clone();
        let owner = self.owner.clone();
        let query_locked = self.busy || !self.pending.is_empty() || !self.query_controls;
        let resize_owner = owner.clone();
        let resize_field = name.clone();
        let resize_width = f32::from(self.columns[index].width);
        let sort_owner = owner.clone();
        let sort_name = name.clone();
        let direction = self
            .sort
            .as_ref()
            .filter(|(column, _)| column == &name)
            .map(|(_, desc)| *desc);
        let selected = self.highlight && self.selection.is_some_and(|(_, c)| c == col);
        div()
            .id(("column-head", index))
            .h_flex()
            .px_2()
            .h(gpui_component::Size::Medium.table_row_height() - px(1.))
            .flex_1()
            .min_w_0()
            .mr(-gpui_component::Size::Medium.table_cell_padding().right)
            .gap_1()
            .when(selected, |el| {
                el.bg(self
                    .highlight_color
                    .unwrap_or_else(|| default_highlight_color(cx))
                    .opacity(0.22))
            })
            .child(column_icon(meta, cx))
            .child(
                div()
                    .id(("column-tip", index))
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(name)
                    .tooltip(move |window, cx| {
                        let meta = tip_schema
                            .as_ref()
                            .and_then(|schema| schema.columns.iter().find(|c| c.name == tip_name));
                        let tip = crate::locale::tr(
                            cx,
                            "grid.column.tip",
                            &[
                                ("name", &tip_name),
                                ("type", meta.map(|c| c.data_type.as_str()).unwrap_or("—")),
                                (
                                    "comment",
                                    meta.map(|c| c.comment.as_str())
                                        .filter(|s| !s.is_empty())
                                        .unwrap_or("—"),
                                ),
                                ("table", &tip_target),
                            ],
                        );
                        Tooltip::new(tip).build(window, cx)
                    }),
            )
            .when(self.mongo, |el| {
                el.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(meta.map(|m| m.data_type.clone()).unwrap_or_default()),
                )
            })
            .child(
                Button::new(("sort-column", index))
                    .ghost()
                    .xsmall()
                    .icon(match direction {
                        Some(false) => IconName::ArrowUp,
                        Some(true) => IconName::ArrowDown,
                        None => IconName::ChevronsUpDown,
                    })
                    .when(direction.is_some(), |button| {
                        button.text_color(cx.theme().primary)
                    })
                    .disabled(self.busy || !self.pending.is_empty() || !self.query_controls)
                    .tooltip(t(
                        cx,
                        match direction {
                            None => "grid.sort.asc",
                            Some(false) => "grid.sort.desc",
                            Some(true) => "grid.sort.default",
                        },
                    ))
                    .on_click(move |_, _, cx| {
                        if let Some(owner) = &sort_owner {
                            let _ = owner.update(cx, |this, cx| {
                                if this.busy || !this.changes.is_empty() {
                                    return;
                                }
                                let current = this
                                    .options
                                    .sort
                                    .as_ref()
                                    .filter(|(column, _)| column == &sort_name)
                                    .map(|(_, desc)| *desc);
                                this.options.sort = match current {
                                    None => Some((sort_name.clone(), false)),
                                    Some(false) => Some((sort_name.clone(), true)),
                                    Some(true) => None,
                                };
                                this.change_query(cx);
                            });
                        }
                    }),
            )
            .child(
                Button::new(("column-menu", index))
                    .ghost()
                    .xsmall()
                    .icon(IconName::ChevronDown)
                    .dropdown_menu(move |mut menu, _, cx| {
                        let copy = copy.clone();
                        menu = menu.item(
                            PopupMenuItem::new(t(cx, "grid.column.copy"))
                                .icon(IconName::Copy)
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                                    crate::feedback::success(t(cx, "feedback.copied"), cx);
                                }),
                        );
                        for (key, action) in [
                            ("grid.width.set", "set"),
                            ("grid.width.fit", "fit"),
                            ("grid.width.reset", "reset"),
                        ] {
                            let owner = owner.clone();
                            menu = menu.item(PopupMenuItem::new(t(cx, key)).on_click(
                                move |_, window, cx| {
                                    if let Some(owner) = &owner {
                                        let _ = owner.update(cx, |this, cx| {
                                            this.width_action(col, action, window, cx)
                                        });
                                    }
                                },
                            ));
                        }
                        for (key, filter) in [("grid.filter", true), ("grid.structure", false)] {
                            let owner = owner.clone();
                            menu = menu.item(
                                PopupMenuItem::new(t(cx, key))
                                    .disabled(filter && query_locked)
                                    .icon(if filter {
                                        IconName::Search
                                    } else {
                                        IconName::LayoutDashboard
                                    })
                                    .on_click(move |_, window, cx| {
                                        if let Some(owner) = &owner {
                                            let _ = owner.update(cx, |this, cx| {
                                                if filter {
                                                    if !this.busy && this.changes.is_empty() {
                                                        this.filter_dialog(col, window, cx);
                                                    }
                                                } else {
                                                    this.show_details(None, false, cx);
                                                }
                                            });
                                        }
                                    }),
                            );
                        }
                        menu
                    }),
            )
            .when_some(resize_owner, |el, owner| {
                el.child(width_ui::handle(owner, resize_field, resize_width, cx))
            })
            .into_any_element()
    }
    fn render_td(
        &mut self,
        row: usize,
        index: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        if index == 0 {
            return div()
                .size_full()
                .h_flex()
                .px_2()
                .when(
                    self.highlight && self.selection.is_some_and(|(r, _)| r == row),
                    |el| {
                        el.bg(self
                            .highlight_color
                            .unwrap_or_else(|| default_highlight_color(cx))
                            .opacity(0.18))
                    },
                )
                .text_color(cx.theme().muted_foreground)
                .child(format!(
                    "{}{}",
                    self.offset + row + 1,
                    if self.deleted.contains(&row) {
                        " −"
                    } else if self.pending.contains(&row) {
                        " *"
                    } else {
                        ""
                    }
                ))
                .into_any_element();
        }
        let col = index - 1;
        let v = self
            .rows
            .values
            .get(row)
            .and_then(|r| r.get(col))
            .and_then(|value| value.as_deref());
        let missing = v.is_none();
        let key = (row, col);
        if !self.previews.contains_key(&key) {
            if self.previews.len() >= 4096
                && let Some(old) = self.preview_order.pop_front()
            {
                self.previews.remove(&old);
            }
            let text: SharedString = v
                .unwrap_or(if self.defaults.contains(&key) {
                    "DEFAULT"
                } else if self.mongo {
                    "∅"
                } else {
                    "NULL"
                })
                .chars()
                .take(256)
                .collect::<String>()
                .into();
            self.previews.insert(key, text);
            self.preview_order.push_back(key);
        }
        let text = self.previews[&key].clone();
        let owner = self.owner.clone();
        let menu_owner = owner.clone();
        let mongo = self.mongo;
        let active = self.selection == Some((row, col));
        let related = self.highlight && self.selection.is_some_and(|(r, c)| r == row || c == col);
        let cell = div()
            .id(("data-cell", row * 513 + index))
            .size_full()
            .h_flex()
            .px_2()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .when(related, |el| {
                el.bg(self
                    .highlight_color
                    .unwrap_or_else(|| default_highlight_color(cx))
                    .opacity(0.18))
            })
            .when(active, |el| {
                el.bg(if self.highlight {
                    self.highlight_color
                        .unwrap_or_else(|| default_highlight_color(cx))
                        .opacity(0.32)
                } else {
                    cx.theme().accent
                })
                .border_1()
                .border_color(cx.theme().primary)
            })
            .when(missing || self.deleted.contains(&row), |el| {
                el.text_color(cx.theme().muted_foreground)
            })
            .child(text)
            .on_click(move |event, window, cx| {
                if let Some(owner) = &owner {
                    let _ = owner.update(cx, |this, cx| {
                        this.select_cell(row, col, window, cx);
                        if event.click_count() == 2 {
                            this.edit_cell(row, col, window, cx);
                        }
                    });
                }
                cx.stop_propagation();
            });
        crate::context_menu::cell(
            SharedString::from(format!("cell-menu-{row}-{col}")),
            cell,
            move |mut menu, window, cx| {
                if mongo {
                    for (key, action) in [
                        ("mongo.copy.value", "cell"),
                        ("mongo.copy.document", "json"),
                        ("mongo.node.inspect", "inspect"),
                    ] {
                        let owner = menu_owner.clone();
                        menu =
                            menu.item(PopupMenuItem::new(t(cx, key)).on_click(move |_, _, cx| {
                                if let Some(owner) = &owner {
                                    let _ = owner.update(cx, |this, cx| {
                                        if action == "inspect" {
                                            this.open_mongo_node(row, col, cx);
                                        } else {
                                            this.copy(row, col, action, cx);
                                        }
                                    });
                                }
                            }));
                    }
                    return menu;
                }
                if menu_owner
                    .as_ref()
                    .and_then(|owner| owner.upgrade())
                    .is_none_or(|owner| owner.read(cx).busy)
                {
                    return menu;
                }
                let writable = menu_owner
                    .as_ref()
                    .and_then(|owner| owner.upgrade())
                    .is_some_and(|owner| owner.read(cx).can_delete_row(row));
                for (key, action) in [
                    ("grid.copy.cell", "cell"),
                    ("grid.copy.row", "txt"),
                    ("grid.delete.row", "delete"),
                    ("grid.inspect", "inspect"),
                ] {
                    let owner = menu_owner.clone();
                    menu = menu.item(
                        PopupMenuItem::new(t(cx, key))
                            .disabled(action == "delete" && !writable)
                            .icon(match action {
                                "delete" => IconName::Delete,
                                "inspect" => IconName::Inspector,
                                _ => IconName::Copy,
                            })
                            .on_click(move |_, window, cx| {
                                if let Some(owner) = &owner {
                                    let _ = owner.update(cx, |this, cx| {
                                        this.select_cell(row, col, window, cx);
                                        match action {
                                            "delete" => this.delete_row(row, window, cx),
                                            "inspect" => this.show_details(Some(row), false, cx),
                                            format => this.copy(row, col, format, cx),
                                        }
                                    });
                                }
                            }),
                    );
                }
                let structure_available = menu_owner
                    .as_ref()
                    .and_then(|owner| owner.upgrade())
                    .is_some_and(|owner| {
                        matches!(owner.read(cx).target.object.kind.as_str(), "table" | "view")
                    });
                let structure_owner = menu_owner.clone();
                menu = menu.item(
                    PopupMenuItem::new(t(cx, "grid.view_definition"))
                        .icon(IconName::File)
                        .disabled(!structure_available)
                        .on_click(move |_, _, cx| {
                            if let Some(owner) = &structure_owner {
                                let _ = owner.update(cx, |this, cx| {
                                    crate::feedback::ddl_context(
                                        crate::feedback::DdlContext {
                                            note: None,
                                            candidates: vec![],
                                            origin: None,
                                            scope: Some((
                                                this.target.connection,
                                                this.target.database.clone(),
                                            )),
                                            object: Some(this.target.object.clone()),
                                            open: true,
                                        },
                                        cx,
                                    );
                                });
                            }
                        }),
                );
                let document_owner = menu_owner.clone();
                menu = menu.item(
                    PopupMenuItem::new(t(cx, "result.full_value"))
                        .icon(IconName::File)
                        .on_click(move |_, window, cx| {
                            if let Some(owner) = &document_owner {
                                let _ = owner.update(cx, |this, cx| {
                                    this.show_document(Some((row, col)), window, cx)
                                });
                            }
                        }),
                );
                let owner = menu_owner.clone();
                menu.submenu(
                    t(cx, "grid.copy.advanced"),
                    window,
                    cx,
                    move |mut menu, _, cx| {
                        for (key, format) in [
                            ("grid.copy.csv", "csv"),
                            ("grid.copy.json", "json"),
                            ("grid.copy.insert", "insert"),
                            ("grid.copy.update", "update"),
                            ("grid.copy.md", "md"),
                            ("grid.copy.txt", "txt"),
                        ] {
                            let owner = owner.clone();
                            menu = menu.item(
                                PopupMenuItem::new(t(cx, key))
                                    .icon(IconName::Copy)
                                    .on_click(move |_, _, cx| {
                                        if let Some(owner) = &owner {
                                            let _ = owner.update(cx, |this, cx| {
                                                this.copy(row, col, format, cx)
                                            });
                                        }
                                    }),
                            );
                        }
                        menu
                    },
                )
            },
        )
        .into_any_element()
    }
}

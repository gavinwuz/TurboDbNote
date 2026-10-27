use super::*;
use gpui_component::{
    menu::PopupMenuItem,
    scroll::{Scrollbar, ScrollbarShow},
};
impl Comparison {
    pub(super) fn refresh_table_status(&mut self) {
        self.table_status = self
            .table_reasons
            .iter()
            .map(|(key, reasons)| {
                (
                    key.clone(),
                    reasons
                        .iter()
                        .any(|reason| self.structure == (reason.category != "compare.reason.data")),
                )
            })
            .collect();
    }

    pub(super) fn refresh_detail(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(schemas) = &self.schemas else {
            self.detail_grid = None;
            return;
        };
        if self.reasons_view {
            let kind = self.sides[0].target.kind;
            match comparison::structure_details(
                kind,
                &schemas[0],
                &schemas[1],
                comparison::CompareOptions {
                    ignore_comments: self.ignore_comments,
                    only_differences: self.only_differences,
                },
                self.ignore_column_order,
                &self.field_filter.read(cx).value(),
            ) {
                Ok(mut rows) => {
                    rows.values.retain(|row| {
                        let key = row.get(2).and_then(Option::as_deref).unwrap_or("");
                        match self.detail_class {
                            1 => {
                                key.starts_with("ddl.column.")
                                    || matches!(
                                        key,
                                        "grid.structure.type"
                                            | "compare.column_definition"
                                            | "compare.default_value"
                                            | "compare.reason.order"
                                            | "compare.reason.definitions"
                                    )
                            }
                            2 => key == "compare.reason.indexes",
                            3 => matches!(
                                key,
                                "compare.reason.constraints"
                                    | "compare.reason.foreign"
                                    | "compare.reason.triggers"
                            ),
                            4 => matches!(key, "compare.reason.options" | "compare.reason.ddl"),
                            _ => true,
                        }
                    });
                    let differences = rows
                        .values
                        .iter()
                        .filter(|row| row.first().and_then(Option::as_deref) != Some("="))
                        .count();
                    self.notice = if self.whole_database {
                        tr(
                            cx,
                            "compare.database_preview_summary",
                            &[
                                ("tables", &self.table_status.len().to_string()),
                                (
                                    "changed",
                                    &self
                                        .table_status
                                        .values()
                                        .filter(|different| **different)
                                        .count()
                                        .to_string(),
                                ),
                                ("table", self.selected_table.as_deref().unwrap_or("")),
                                ("rows", &differences.to_string()),
                                ("scope", &t(cx, "compare.scope_structure")),
                            ],
                        )
                        .to_string()
                    } else {
                        tr(
                            cx,
                            "compare.attribute_summary",
                            &[("different", &differences.to_string())],
                        )
                        .to_string()
                    };
                    rows.columns[0] = t(cx, "compare.status").to_string();
                    rows.columns[1] = "compare.object_name".into();
                    for key in rows.columns.iter_mut().skip(1) {
                        *key = t(cx, key).to_string();
                    }
                    for row in &mut rows.values {
                        if let Some(Some(key)) = row.get_mut(2) {
                            *key = t(cx, key).to_string();
                        }
                    }
                    self.detail_grid = Some(cx.new(|cx| {
                        TableState::new(
                            QueryGrid::new(rows, 0)
                                .without_row_numbers()
                                .fixed_columns(2)
                                .with_differences(0)
                                .with_status_labels()
                                .with_column_widths(&[90., 140., 120., 200., 200.]),
                            window,
                            cx,
                        )
                    }));
                }
                Err(error) => feedback::error(crate::locale::message(cx, &error), cx),
            }
            cx.notify();
            return;
        }
        if self.metadata_view {
            let query = self.field_filter.read(cx).value().to_lowercase();
            let mut rows = Rows {
                columns: vec![
                    "Δ".into(),
                    t(cx, "compare.attribute").to_string(),
                    t(cx, "compare.reference_value").to_string(),
                    t(cx, "compare.target_value").to_string(),
                ],
                values: vec![],
            };
            for (key, left, right) in [
                ("dbwork.index", &schemas[0].indexes, &schemas[1].indexes),
                (
                    "dbwork.foreign",
                    &schemas[0].foreign_keys,
                    &schemas[1].foreign_keys,
                ),
                ("dbwork.trigger", &schemas[0].triggers, &schemas[1].triggers),
            ] {
                let left = left
                    .values
                    .iter()
                    .map(|row| {
                        row.iter()
                            .filter_map(Option::as_deref)
                            .collect::<Vec<_>>()
                            .join(" · ")
                    })
                    .collect::<Vec<_>>();
                let right = right
                    .values
                    .iter()
                    .map(|row| {
                        row.iter()
                            .filter_map(Option::as_deref)
                            .collect::<Vec<_>>()
                            .join(" · ")
                    })
                    .collect::<Vec<_>>();
                for index in 0..left.len().max(right.len()) {
                    let a = left.get(index).cloned();
                    let b = right.get(index).cloned();
                    if self.only_differences && a == b {
                        continue;
                    }
                    if !query.is_empty()
                        && !format!(
                            "{} {}",
                            a.as_deref().unwrap_or(""),
                            b.as_deref().unwrap_or("")
                        )
                        .to_lowercase()
                        .contains(&query)
                    {
                        continue;
                    }
                    rows.values.push(vec![
                        Some(if a == b { "=" } else { "≠" }.into()),
                        Some(t(cx, key).to_string()),
                        a,
                        b,
                    ]);
                }
            }
            self.detail_grid = Some(cx.new(|cx| {
                TableState::new(
                    QueryGrid::new(rows, 0)
                        .without_row_numbers()
                        .fixed_columns(2)
                        .with_differences(0)
                        .with_column_widths(&[40., 130., 400., 400.]),
                    window,
                    cx,
                )
            }));
            cx.notify();
            return;
        }
        match comparison::field_details(
            &schemas[0],
            &schemas[1],
            comparison::CompareOptions {
                ignore_comments: self.ignore_comments,
                only_differences: self.only_differences,
            },
            &self.field_filter.read(cx).value(),
        ) {
            Ok(mut rows) => {
                if !self.only_differences {
                    let mut groups =
                        std::collections::BTreeMap::<String, Vec<Vec<Option<String>>>>::new();
                    for row in rows.values.drain(..) {
                        groups
                            .entry(row.get(1).cloned().flatten().unwrap_or_default())
                            .or_default()
                            .push(row);
                    }
                    for (name, mut attributes) in groups {
                        let key =
                            format!("{}\n{name}", self.selected_table.as_deref().unwrap_or(""));
                        if self.expanded_fields.contains(&key) {
                            rows.values.extend(attributes);
                        } else if !attributes.is_empty() {
                            let changed = attributes
                                .iter()
                                .any(|row| row.first().and_then(Option::as_deref) != Some("="));
                            let mut first = attributes.remove(0);
                            if changed {
                                first[0] = Some("≠".into());
                            }
                            rows.values.push(first);
                        }
                    }
                }
                for key in rows.columns.iter_mut().skip(1) {
                    *key = t(cx, key).to_string();
                }
                for row in &mut rows.values {
                    if let Some(Some(key)) = row.get_mut(2) {
                        *key = t(cx, key).to_string();
                    }
                }
                let grid = QueryGrid::new(rows, 0)
                    .without_row_numbers()
                    .fixed_columns(2)
                    .with_differences(0)
                    .with_column_widths(&[40., 160., 130., 260., 260.]);
                self.detail_grid = Some(cx.new(|cx| TableState::new(grid, window, cx)));
                self.field_position = 0;
            }
            Err(error) => feedback::error(crate::locale::message(cx, &error), cx),
        }
        cx.notify();
    }
    pub(super) fn visible_tables(&self, cx: &App) -> Vec<String> {
        let Some(snapshots) = &self.snapshots else {
            return vec![];
        };
        let query = self.table_filter.read(cx).value().to_lowercase();
        comparison::database_pairs(&snapshots[0], &snapshots[1])
            .into_iter()
            .filter(|key| {
                if !key.to_lowercase().contains(&query) {
                    return false;
                }
                let different = self.table_status.get(key).copied().unwrap_or(true);
                match self.table_class {
                    1 => comparison::database_find(&snapshots[1], key).is_none(),
                    2 => comparison::database_find(&snapshots[0], key).is_none(),
                    3 => !different,
                    4 => different,
                    _ => true,
                }
            })
            .collect()
    }
    fn directory(&self, cx: &mut Context<Self>) -> AnyElement {
        let keys = std::rc::Rc::new(self.visible_tables(cx));
        let total = keys.len();
        let source = cx.entity().downgrade();
        let select = keys.clone();
        let clear = source.clone();
        let filter_label = match self.table_class {
            1 => "compare.only_reference",
            2 => "compare.only_target",
            3 => "compare.same",
            4 => "compare.changed_tables",
            _ => "compare.all_tables",
        };
        let list_keys = keys.clone();
        let list = uniform_list(
            "compare-table-directory",
            total,
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                range
                    .map(|index| {
                        let key = list_keys[index].clone();
                        let open = key.clone();
                        let toggle = key.clone();
                        let changed = this.table_status.get(&key).copied().unwrap_or(true);
                        let marker = this
                            .snapshots
                            .as_ref()
                            .map(|snapshots| {
                                if this.busy {
                                    "…"
                                } else if comparison::database_find(&snapshots[0], &key).is_none() {
                                    "−"
                                } else if comparison::database_find(&snapshots[1], &key).is_none() {
                                    "+"
                                } else if changed {
                                    "≠"
                                } else {
                                    "="
                                }
                            })
                            .unwrap_or("…");
                        let selected = this.sync_tables.contains(&key);
                        div()
                            .h_flex()
                            .h(px(30.))
                            .min_w_0()
                            .gap_1()
                            .child(
                                gpui_component::checkbox::Checkbox::new(("sync-table", index))
                                    .checked(selected)
                                    .on_click(cx.listener(move |this, checked, _, cx| {
                                        if *checked {
                                            this.sync_tables.insert(toggle.clone());
                                        } else {
                                            this.sync_tables.remove(&toggle);
                                        }
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new(("compare-table", index))
                                    .ghost()
                                    .flex_1()
                                    .min_w_0()
                                    .justify_start()
                                    .selected(this.selected_table.as_ref() == Some(&key))
                                    .child(
                                        div()
                                            .h_flex()
                                            .w_full()
                                            .min_w_0()
                                            .gap_1()
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .overflow_hidden()
                                                    .text_ellipsis()
                                                    .whitespace_nowrap()
                                                    .child(format!("{marker} {key}")),
                                            )
                                            .when_some(
                                                this.table_reasons.get(&key).and_then(|reasons| {
                                                    reasons.iter().find(|reason| {
                                                        this.structure
                                                            == (reason.category
                                                                != "compare.reason.data")
                                                    })
                                                }),
                                                |row, reason| {
                                                    row.child(
                                                        div()
                                                            .text_xs()
                                                            .flex_shrink_0()
                                                            .child(t(cx, reason.category)),
                                                    )
                                                },
                                            ),
                                    )
                                    .tooltip(format!(
                                        "{} · {}",
                                        key,
                                        this.table_reasons
                                            .get(&key)
                                            .map(|reasons| reasons
                                                .iter()
                                                .filter(|reason| this.structure
                                                    == (reason.category != "compare.reason.data"))
                                                .map(|reason| t(cx, reason.category).to_string())
                                                .collect::<Vec<_>>()
                                                .join(" / "))
                                            .unwrap_or_default()
                                    ))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.show_database_table(&open, window, cx)
                                    })),
                            )
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(self.table_scroll.clone())
        .size_full();
        div()
            .v_flex()
            .w(px(250.))
            .min_w(px(180.))
            .h_full()
            .min_h_0()
            .gap_2()
            .child(Input::new(&self.table_filter).cleanable(true))
            .child(
                Button::new("table-filter")
                    .ghost()
                    .label(t(cx, filter_label))
                    .dropdown_menu(move |mut menu, _, cx| {
                        for (value, key) in [
                            (0, "compare.all_tables"),
                            (4, "compare.changed_tables"),
                            (1, "compare.only_reference"),
                            (2, "compare.only_target"),
                            (3, "compare.same"),
                        ] {
                            let source = source.clone();
                            menu = menu.item(PopupMenuItem::new(t(cx, key)).on_click(
                                move |_, window, cx| {
                                    let _ = source.update(cx, |this, cx| {
                                        this.table_class = value;
                                        this.saved_views.clear();
                                        if value == 3 {
                                            this.only_differences = false;
                                        } else if value == 4 {
                                            this.only_differences = true;
                                        }
                                        let visible = this.visible_tables(cx);
                                        let key = this
                                            .selected_table
                                            .clone()
                                            .filter(|key| visible.contains(key))
                                            .or_else(|| visible.first().cloned());
                                        if let Some(key) = key {
                                            this.show_database_table(&key, window, cx);
                                        } else {
                                            this.selected_table = None;
                                            this.detail_grid = None;
                                            this.grids = [None, None];
                                        }
                                        cx.notify();
                                    });
                                },
                            ));
                        }
                        menu.separator()
                            .item(
                                PopupMenuItem::new(t(cx, "compare.select_filtered")).on_click({
                                    let source = clear.clone();
                                    let select = select.clone();
                                    move |_, _, cx| {
                                        let _ = source.update(cx, |this, cx| {
                                            this.sync_tables.extend(select.iter().cloned());
                                            cx.notify();
                                        });
                                    }
                                }),
                            )
                            .item(
                                PopupMenuItem::new(t(cx, "compare.clear_selection")).on_click({
                                    let source = clear.clone();
                                    move |_, _, cx| {
                                        let _ = source.update(cx, |this, cx| {
                                            this.sync_tables.clear();
                                            cx.notify();
                                        });
                                    }
                                }),
                            )
                    }),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .pr(px(14.))
                    .child(list)
                    .child(
                        div()
                            .absolute()
                            .right_0()
                            .top_0()
                            .bottom_0()
                            .w(px(12.))
                            .child(
                                Scrollbar::vertical(&self.table_scroll)
                                    .scrollbar_show(ScrollbarShow::Always),
                            ),
                    ),
            )
            .into_any_element()
    }
}
impl Render for Comparison {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.structure && self.table_mode && self.detail_grid.is_none() && self.schemas.is_some()
        {
            self.refresh_detail(window, cx);
        }
        let owner = cx.entity().downgrade();
        let options = owner.clone();
        let ignored_comments = self.ignore_comments;
        let ignored_order = self.ignore_column_order;
        let more = owner.clone();
        let locked = self.locked.clone();
        let selected = self.sync_tables.len();
        let visible = self
            .visible_tables(cx)
            .into_iter()
            .collect::<std::collections::HashSet<_>>();
        let hidden = self
            .sync_tables
            .iter()
            .filter(|key| !visible.contains(*key))
            .count();
        let selected_label = tr(
            cx,
            "compare.selected_count",
            &[
                ("count", &selected.to_string()),
                ("hidden", &hidden.to_string()),
            ],
        );
        let mut result = div().v_flex().flex_1().h_full().min_h_0().min_w_0().gap_2();
        result = result.child(
            div()
                .h_flex()
                .flex_wrap()
                .gap_1()
                .when(self.structure && self.table_mode, |bar| {
                    bar.child(
                        div()
                            .w(px(220.))
                            .child(Input::new(&self.field_filter).cleanable(true)),
                    )
                })
                .when(
                    self.structure
                        && self.table_mode
                        && !self.only_differences
                        && !self.metadata_view
                        && !self.reasons_view,
                    |bar| {
                        bar.child(
                            Button::new("expand-field-properties")
                                .ghost()
                                .label(t(cx, "compare.expand_field"))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let name = this.detail_grid.as_ref().and_then(|grid| {
                                        let grid = grid.read(cx).delegate();
                                        grid.selected.and_then(|(index, _)| {
                                            grid.rows.values.get(index)?.get(1)?.clone()
                                        })
                                    });
                                    if let Some(name) = name {
                                        let key = format!(
                                            "{}\n{name}",
                                            this.selected_table.as_deref().unwrap_or("")
                                        );
                                        if !this.expanded_fields.remove(&key) {
                                            this.expanded_fields.insert(key);
                                        }
                                        this.refresh_detail(window, cx);
                                    }
                                })),
                        )
                    },
                )
                .when(self.structure, |bar| {
                    bar.child(
                        Button::new("only-differences")
                            .ghost()
                            .label(t(cx, "compare.changed_attributes"))
                            .selected(self.only_differences)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.only_differences = !this.only_differences;
                                this.saved_views.clear();
                                this.refresh_detail(window, cx);
                                if let Some(key) = this.selected_table.clone() {
                                    this.show_database_table(&key, window, cx);
                                }
                                cx.notify();
                            })),
                    )
                })
                .when(self.include_data, |bar| {
                    bar.children([true, false].map(|structure| {
                        Button::new(if structure {
                            "compare-structure-tab"
                        } else {
                            "compare-data-tab"
                        })
                        .ghost()
                        .label(t(
                            cx,
                            if structure {
                                "compare.scope_structure"
                            } else {
                                "compare.scope_sample"
                            },
                        ))
                        .selected(self.structure == structure)
                        .disabled(self.busy || self.schemas.is_none())
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.structure = structure;
                                this.saved_views.clear();
                                this.refresh_table_status();
                                let visible = this.visible_tables(cx);
                                let key = this
                                    .selected_table
                                    .clone()
                                    .filter(|key| visible.contains(key))
                                    .or_else(|| visible.first().cloned());
                                if let Some(key) = key {
                                    this.show_database_table(&key, window, cx);
                                } else {
                                    this.detail_grid = None;
                                    this.grids = [None, None];
                                }
                                cx.notify();
                            },
                        ))
                    }))
                })
                .when(self.structure, |bar| {
                    let mut bar = bar;
                    for (id, key, table_mode) in [
                        ("compare-table-view", "compare.difference_table", true),
                        ("compare-ddl-view", "compare.show_ddl", false),
                    ] {
                        bar = bar.child(
                            Button::new(id)
                                .ghost()
                                .label(t(cx, key))
                                .selected(self.table_mode == table_mode)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.saved_views.clear();
                                    this.reasons_view = true;
                                    this.metadata_view = false;
                                    if this.table_mode != table_mode {
                                        this.toggle_structure(window, cx);
                                    }
                                    this.refresh_detail(window, cx);
                                })),
                        );
                    }
                    bar.when(self.table_mode, |bar| {
                        bar.child(
                            Button::new("compare-object-filter")
                                .ghost()
                                .label(t(
                                    cx,
                                    match self.detail_class {
                                        1 => "compare.object_columns",
                                        2 => "compare.object_indexes",
                                        3 => "compare.object_constraints",
                                        4 => "compare.object_properties",
                                        _ => "compare.object_all",
                                    },
                                ))
                                .dropdown_menu({
                                    let owner = cx.entity().downgrade();
                                    move |mut menu, _, cx| {
                                        for (class, key) in [
                                            (0, "compare.object_all"),
                                            (1, "compare.object_columns"),
                                            (2, "compare.object_indexes"),
                                            (3, "compare.object_constraints"),
                                            (4, "compare.object_properties"),
                                        ] {
                                            let owner = owner.clone();
                                            menu =
                                                menu.item(PopupMenuItem::new(t(cx, key)).on_click(
                                                    move |_, window, cx| {
                                                        let _ = owner.update(cx, |this, cx| {
                                                            this.detail_class = class;
                                                            this.saved_views.clear();
                                                            this.refresh_detail(window, cx);
                                                        });
                                                    },
                                                ));
                                        }
                                        menu
                                    }
                                }),
                        )
                    })
                })
                .when(
                    self.structure && self.table_mode && self.detail_grid.is_some(),
                    |bar| {
                        bar.children([false, true].map(|next| {
                            Button::new(if next {
                                "next-field-difference"
                            } else {
                                "previous-field-difference"
                            })
                            .ghost()
                            .icon(if next {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronUp
                            })
                            .tooltip(t(
                                cx,
                                if next {
                                    "compare.next_difference"
                                } else {
                                    "compare.previous_difference"
                                },
                            ))
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if let Some(grid) = &this.detail_grid {
                                        let differences = grid
                                            .read(cx)
                                            .delegate()
                                            .rows
                                            .values
                                            .iter()
                                            .enumerate()
                                            .filter(|(_, row)| {
                                                row.first().and_then(Option::as_deref) != Some("=")
                                            })
                                            .map(|(index, _)| index)
                                            .collect::<Vec<_>>();
                                        if !differences.is_empty() {
                                            this.field_position = if next {
                                                (this.field_position + 1) % differences.len()
                                            } else {
                                                (this.field_position + differences.len() - 1)
                                                    % differences.len()
                                            };
                                            let row = differences[this.field_position];
                                            grid.update(cx, |grid, cx| grid.scroll_to_row(row, cx));
                                        }
                                    }
                                },
                            ))
                        }))
                    },
                )
                .when(!self.structure || !self.table_mode, |bar| {
                    bar.child(
                        Button::new("compare-more")
                            .ghost()
                            .icon(IconName::Ellipsis)
                            .dropdown_menu(move |menu, _, cx| {
                                let source = more.clone();
                                menu.item(
                                    PopupMenuItem::new(t(cx, "compare.sync_scroll"))
                                        .checked(locked.get())
                                        .on_click(move |_, _, cx| {
                                            let _ = source.update(cx, |this, cx| {
                                                this.locked.set(!this.locked.get());
                                                cx.notify();
                                            });
                                        }),
                                )
                            }),
                    )
                }),
        );
        if self.structure && self.table_mode {
            if let Some(grid) = &self.detail_grid {
                result = result.child(div().flex_1().min_h_0().min_w_0().child(
                    if grid.read(cx).delegate().rows.values.is_empty() {
                        div()
                            .p_4()
                            .child(t(
                                cx,
                                if self.detail_class == 0
                                    && self.field_filter.read(cx).value().is_empty()
                                {
                                    "compare.identical"
                                } else {
                                    "compare.no_match"
                                },
                            ))
                            .into_any_element()
                    } else {
                        crate::query_grid::view(grid, cx)
                    },
                ));
            }
        } else {
            result = result.child(
                div()
                    .h_flex()
                    .items_start()
                    .flex_1()
                    .h_full()
                    .min_h_0()
                    .min_w_0()
                    .gap_2()
                    .children(self.grids.iter().filter_map(|g| g.as_ref()).map(|grid| {
                        div()
                            .h_full()
                            .flex_1()
                            .min_h_0()
                            .min_w_0()
                            .child(crate::query_grid::view(grid, cx))
                    })),
            );
        }
        let content = div()
            .h_flex()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .gap_3()
            .when(self.whole_database, |body| body.child(self.directory(cx)))
            .child(result);
        let progress = tr(
            cx,
            "compare.progress",
            &[(
                "count",
                &self
                    .progress
                    .load(std::sync::atomic::Ordering::Relaxed)
                    .to_string(),
            )],
        );
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
                    .gap_2()
                    .flex_shrink_0()
                    .child(self.side(0, cx))
                    .child(
                        Button::new("swap")
                            .ghost()
                            .icon(IconName::Replace)
                            .tooltip(t(cx, "compare.swap"))
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                let mut targets = this.snapshot(cx);
                                targets.swap(0, 1);
                                this.restore_sources(targets, window, cx);
                                this.invalidate();
                            })),
                    )
                    .child(self.side(1, cx)),
            )
            .child(
                div()
                    .h_flex()
                    .flex_wrap()
                    .gap_2()
                    .flex_shrink_0()
                    .child(
                        Button::new("compare-scope")
                            .ghost()
                            .label(t(
                                cx,
                                if self.include_data {
                                    "compare.include_data"
                                } else {
                                    "compare.scope_structure"
                                },
                            ))
                            .icon(IconName::ChevronDown)
                            .disabled(self.busy)
                            .dropdown_menu(move |mut menu, _, cx| {
                                for (whole, data, key) in [
                                    (true, false, "compare.database_scope"),
                                    (true, true, "compare.include_data"),
                                    (false, false, "compare.single_table"),
                                ] {
                                    let source = owner.clone();
                                    menu = menu.item(PopupMenuItem::new(t(cx, key)).on_click(
                                        move |_, _, cx| {
                                            let _ = source.update(cx, |this, cx| {
                                                this.whole_database = whole;
                                                this.include_data = data;
                                                this.structure = !data;
                                                this.invalidate();
                                                cx.notify();
                                            });
                                        },
                                    ));
                                }
                                menu
                            }),
                    )
                    .child(
                        Button::new("compare-options")
                            .ghost()
                            .label(t(
                                cx,
                                if self.ignore_comments || !self.ignore_column_order {
                                    "compare.options_ignored"
                                } else {
                                    "compare.options"
                                },
                            ))
                            .icon(IconName::ChevronDown)
                            .disabled(self.busy)
                            .dropdown_menu(move |menu, _, cx| {
                                let source = options.clone();
                                let order = options.clone();
                                menu.item(
                                    PopupMenuItem::new(t(cx, "compare.ignore_comments"))
                                        .checked(ignored_comments)
                                        .on_click(move |_, _, cx| {
                                            let _ = source.update(cx, |this, cx| {
                                                this.ignore_comments = !this.ignore_comments;
                                                this.invalidate();
                                                cx.notify();
                                            });
                                        }),
                                )
                                .item(
                                    PopupMenuItem::new(t(cx, "compare.ignore_order"))
                                        .checked(ignored_order)
                                        .on_click(move |_, _, cx| {
                                            let _ = order.update(cx, |this, cx| {
                                                this.ignore_column_order =
                                                    !this.ignore_column_order;
                                                this.invalidate();
                                                cx.notify();
                                            });
                                        }),
                                )
                            }),
                    )
                    .child(div().flex_1())
                    .when(self.busy, |bar| {
                        bar.child(
                            Button::new("compare-cancel")
                                .ghost()
                                .label(t(cx, "compare.cancel"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(control) = &this.compare_control {
                                        control.cancel();
                                    }
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        Button::new("compare-start")
                            .primary()
                            .icon(IconName::ArrowRight)
                            .label(t(cx, "compare.run"))
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.start(window, cx))),
                    ),
            )
            .child(content)
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .flex_shrink_0()
                    .min_w_0()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(if self.busy {
                                progress.to_string()
                            } else {
                                self.notice.clone()
                            }),
                    )
                    .child(div().text_xs().child(selected_label))
                    .child(
                        Button::new("compare-generate-sql")
                            .icon(IconName::Copy)
                            .label(t(cx, "compare.generate_sql"))
                            .disabled(
                                self.busy
                                    || self.schemas.is_none()
                                    || self.whole_database && selected == 0,
                            )
                            .on_click(
                                cx.listener(|this, _, window, cx| this.sync_preview(window, cx)),
                            ),
                    ),
            )
    }
}

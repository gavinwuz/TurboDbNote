use super::*;
use gpui_component::tooltip::Tooltip;
use value_ui::ValueInput;
const OPERATORS: [(&str, &str); 9] = [
    ("eq", "grid.filter.eq"),
    ("ne", "grid.filter.ne"),
    ("gt", "grid.filter.gt"),
    ("ge", "grid.filter.ge"),
    ("lt", "grid.filter.lt"),
    ("le", "grid.filter.le"),
    ("contains", "grid.filter.contains"),
    ("null", "grid.filter.null"),
    ("not_null", "grid.filter.not_null"),
];
fn operator_label(op: &str, cx: &App) -> SharedString {
    match op {
        "eq" => "=".into(),
        "ne" => "≠".into(),
        "gt" => ">".into(),
        "ge" => "≥".into(),
        "lt" => "<".into(),
        "le" => "≤".into(),
        "null" => "IS NULL".into(),
        "not_null" => "IS NOT NULL".into(),
        _ => t(cx, "grid.filter.contains"),
    }
}
fn null_operator(op: &str) -> bool {
    matches!(op, "null" | "not_null")
}
struct ConditionDraft {
    id: usize,
    column: String,
    operator: String,
    value: Rc<ValueInput>,
}
pub(super) struct Draft {
    rows: Vec<ConditionDraft>,
    join: FilterJoin,
    next_id: usize,
}
impl Draft {
    fn filter(&self, cx: &App) -> Option<FilterSet> {
        (!self.rows.is_empty()).then(|| FilterSet::Group {
            join: self.join,
            conditions: self
                .rows
                .iter()
                .map(|row| Filter {
                    column: row.column.clone(),
                    operator: row.operator.clone(),
                    value: if null_operator(&row.operator) {
                        String::new()
                    } else {
                        row.value.input.read(cx).value().to_string()
                    },
                })
                .collect(),
        })
    }
}
impl TableView {
    fn filter_row(
        &self,
        id: usize,
        column: String,
        existing: Option<&Filter>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ConditionDraft {
        let ty = self
            .schema
            .as_ref()
            .and_then(|s| s.columns.iter().find(|c| c.name == column))
            .map(|c| c.data_type.as_str())
            .unwrap_or_default();
        let operator = existing.map(|f| f.operator.clone()).unwrap_or_else(|| {
            if ["char", "text", "string", "clob"]
                .iter()
                .any(|part| ty.to_lowercase().contains(part))
            {
                "contains".into()
            } else {
                "eq".into()
            }
        });
        let value = existing.map(|f| f.value.clone()).unwrap_or_default();
        ConditionDraft {
            id,
            column,
            operator,
            value: Rc::new(ValueInput::new(ty, value, false, true, window, cx)),
        }
    }
    pub(super) fn filter_summary(&self, cx: &App) -> String {
        self.last_options
            .filter
            .as_ref()
            .map(|group| {
                group
                    .conditions()
                    .iter()
                    .map(|f| {
                        let value = if null_operator(&f.operator) {
                            String::new()
                        } else {
                            format!(" {}", f.value)
                        };
                        format!("{} {}{}", f.column, operator_label(&f.operator, cx), value)
                    })
                    .collect::<Vec<_>>()
                    .join(&format!(" {} ", group.join().sql()))
            })
            .unwrap_or_else(|| t(cx, "grid.filter.none").to_string())
    }
    pub(super) fn filter_dialog(
        &mut self,
        col: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.query_controls() {
            return;
        }

        if self.busy {
            return;
        }
        if !self.changes.is_empty() {
            crate::feedback::error(t(cx, "grid.pending"), cx);
            return;
        }
        let Some(column) = self.rows.columns.get(col).cloned() else {
            return;
        };
        if self.filter_draft.is_none() {
            let group = self.last_options.filter.clone();
            let rows = group
                .as_ref()
                .map(|g| {
                    g.conditions()
                        .iter()
                        .enumerate()
                        .map(|(id, f)| self.filter_row(id, f.column.clone(), Some(f), window, cx))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            self.filter_draft = Some(Draft {
                next_id: rows.len(),
                rows,
                join: group.as_ref().map(FilterSet::join).unwrap_or_default(),
            });
        }
        if self
            .filter_draft
            .as_ref()
            .is_some_and(|draft| !draft.rows.iter().any(|row| row.column == column))
        {
            self.add_filter_row(column, window, cx);
        }
        cx.notify();
    }
    fn add_filter_row(&mut self, column: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = &self.filter_draft else {
            return;
        };
        if draft.rows.len() >= workbench::MAX_FILTER_CONDITIONS {
            crate::feedback::error(t(cx, "grid.filter.limit"), cx);
            return;
        }
        let id = draft.next_id;
        let row = self.filter_row(id, column, None, window, cx);
        if let Some(draft) = &mut self.filter_draft {
            draft.rows.push(row);
            draft.next_id += 1;
        }
        cx.notify();
    }
    pub(super) fn apply_filter(&mut self, cx: &mut Context<Self>) {
        if self.busy || !self.changes.is_empty() {
            return;
        }
        let Some(draft) = &self.filter_draft else {
            return;
        };
        self.options.filter = draft.filter(cx);
        if self.filter_only {
            if self.target.mysql_raw_gbk
                && self
                    .options
                    .filter
                    .as_ref()
                    .is_some_and(|g| g.conditions().iter().any(|f| !f.value.is_ascii()))
            {
                crate::feedback::error(t(cx, "query_note.gbk_filter"), cx);
                return;
            }
            let Some(schema) = &self.schema else {
                return;
            };
            match workbench::filter_select(
                self.target.kind,
                &self.target.database,
                &self.target.object,
                schema,
                self.options.filter.as_ref(),
            ) {
                Ok(sql) => {
                    self.last_options = self.options.clone();
                    cx.emit(FilterSqlReady(sql));
                    cx.notify();
                }
                Err(error) => crate::feedback::error(crate::locale::message(cx, &error), cx),
            }
            return;
        }

        self.change_query(cx);
    }
    pub(super) fn filter_bar(&self, cx: &Context<Self>) -> AnyElement {
        let locked = self.busy || !self.changes.is_empty();
        let summary = self.filter_summary(cx);
        let full_summary = summary.clone();
        let col = self
            .last_options
            .filter
            .as_ref()
            .and_then(|g| g.conditions().first())
            .and_then(|f| self.rows.columns.iter().position(|c| c == &f.column))
            .or(self.selected.map(|(_, c)| c))
            .unwrap_or(0);
        let count = self
            .last_options
            .filter
            .as_ref()
            .map(|g| g.conditions().len())
            .unwrap_or(0);
        let mut bar = div()
            .v_flex()
            .flex_shrink_0()
            .gap_1()
            .border_b_1()
            .border_color(cx.theme().border)
            .pb_1()
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .min_w_0()
                    .child(
                        Button::new("grid-filter-toggle")
                            .ghost()
                            .small()
                            .icon(IconName::Search)
                            .label(if count == 0 {
                                t(cx, "grid.filter")
                            } else {
                                crate::locale::tr(
                                    cx,
                                    "grid.filter.count",
                                    &[("count", &count.to_string())],
                                )
                            })
                            .selected(count > 0)
                            .disabled(locked || self.rows.columns.is_empty())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if this.filter_draft.is_some() {
                                    this.filter_draft = None;
                                    cx.notify();
                                } else {
                                    this.filter_dialog(col, window, cx);
                                }
                            })),
                    )
                    .child(
                        div()
                            .id("grid-filter-summary")
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(summary)
                            .tooltip(move |window, cx| {
                                Tooltip::new(full_summary.clone()).build(window, cx)
                            }),
                    )
                    .when(count > 0, |row| {
                        row.child(
                            Button::new("grid-filter-clear")
                                .ghost()
                                .small()
                                .icon(IconName::Close)
                                .tooltip(t(cx, "grid.filter.clear_only"))
                                .disabled(locked)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.options.filter = None;
                                    this.filter_draft = None;
                                    this.change_query(cx);
                                })),
                        )
                    })
                    .when_some(self.last_options.sort.as_ref(), |row, (name, desc)| {
                        row.child(
                            Button::new("grid-sort-clear")
                                .ghost()
                                .small()
                                .icon(if *desc {
                                    IconName::ArrowDown
                                } else {
                                    IconName::ArrowUp
                                })
                                .label(name.clone())
                                .max_w(px(160.))
                                .overflow_hidden()
                                .tooltip(t(cx, "grid.sort.default"))
                                .disabled(locked)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.options.sort = None;
                                    this.change_query(cx);
                                })),
                        )
                    })
                    .when(self.filter_draft.is_some(), |row| {
                        row.child(
                            Button::new("grid-filter-collapse")
                                .ghost()
                                .small()
                                .icon(IconName::ChevronUp)
                                .tooltip(t(cx, "grid.filter.collapse"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.filter_draft = None;
                                    cx.notify();
                                })),
                        )
                    }),
            );
        if let Some(draft) = &self.filter_draft {
            let dirty = self.last_options.filter != draft.filter(cx);
            let owner = cx.entity().downgrade();
            let default_column = self.rows.columns.first().cloned().unwrap_or_default();
            bar = bar.child(
                div()
                    .h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        Button::new("filter-join")
                            .small()
                            .icon(IconName::ChevronDown)
                            .label(draft.join.sql())
                            .disabled(locked)
                            .dropdown_menu(move |mut menu, _, cx| {
                                for (join, key) in [
                                    (FilterJoin::And, "grid.filter.and"),
                                    (FilterJoin::Or, "grid.filter.or"),
                                ] {
                                    let owner = owner.clone();
                                    menu = menu.item(PopupMenuItem::new(t(cx, key)).on_click(
                                        move |_, _, cx| {
                                            let _ = owner.update(cx, |this, cx| {
                                                if let Some(draft) = &mut this.filter_draft {
                                                    draft.join = join;
                                                }
                                                cx.notify();
                                            });
                                        },
                                    ));
                                }
                                menu
                            }),
                    )
                    .child(
                        Button::new("filter-add")
                            .small()
                            .ghost()
                            .icon(IconName::Plus)
                            .label(t(cx, "grid.filter.add"))
                            .disabled(
                                locked || draft.rows.len() >= workbench::MAX_FILTER_CONDITIONS,
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.add_filter_row(default_column.clone(), window, cx)
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_xs()
                            .text_color(if dirty {
                                cx.theme().warning
                            } else {
                                cx.theme().muted_foreground
                            })
                            .child(t(
                                cx,
                                if self.busy {
                                    "dbwork.loading"
                                } else if dirty {
                                    "grid.filter.draft"
                                } else {
                                    "grid.filter.applied"
                                },
                            )),
                    )
                    .child(
                        Button::new("grid-filter-apply")
                            .small()
                            .primary()
                            .icon(IconName::Check)
                            .label(t(cx, "grid.apply"))
                            .disabled(locked || !dirty)
                            .on_click(cx.listener(|this, _, _, cx| this.apply_filter(cx))),
                    ),
            );
            let mut rows = div()
                .id("filter-editor-rows")
                .v_flex()
                .gap_1()
                .max_h(px(260.))
                .overflow_y_scroll();
            for (index, row) in draft.rows.iter().enumerate() {
                if index > 0 {
                    rows = rows.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .pl_2()
                            .child(draft.join.sql()),
                    );
                }
                let id = row.id;
                let owner = cx.entity().downgrade();
                let op_owner = owner.clone();
                let names = self.rows.columns.clone();
                let active_operator = row.operator.clone();
                rows = rows.child(
                    div()
                        .id(("filter-row", id))
                        .h_flex()
                        .flex_wrap()
                        .gap_2()
                        .items_center()
                        .child(
                            Button::new(("filter-column", id))
                                .small()
                                .label(row.column.clone())
                                .tooltip(row.column.clone())
                                .max_w(px(160.))
                                .overflow_hidden()
                                .icon(IconName::ChevronDown)
                                .disabled(locked)
                                .dropdown_menu(move |mut menu, _, _| {
                                    for name in &names {
                                        let owner = owner.clone();
                                        let name = name.clone();
                                        menu =
                                            menu.item(PopupMenuItem::new(name.clone()).on_click(
                                                move |_, window, cx| {
                                                    let _ = owner.update(cx, |this, cx| {
                                                        let row = this.filter_row(
                                                            id,
                                                            name.clone(),
                                                            None,
                                                            window,
                                                            cx,
                                                        );
                                                        if let Some(draft) = &mut this.filter_draft
                                                            && let Some(existing) = draft
                                                                .rows
                                                                .iter_mut()
                                                                .find(|r| r.id == id)
                                                        {
                                                            *existing = row;
                                                        }
                                                        cx.notify();
                                                    });
                                                },
                                            ));
                                    }
                                    menu.scrollable(true).max_h(px(300.))
                                }),
                        )
                        .child(
                            Button::new(("filter-operator", id))
                                .small()
                                .label(operator_label(&row.operator, cx))
                                .icon(IconName::ChevronDown)
                                .disabled(locked)
                                .dropdown_menu(move |mut menu, _, cx| {
                                    for (op, key) in OPERATORS {
                                        let owner = op_owner.clone();
                                        menu = menu.item(
                                            PopupMenuItem::new(if op == "contains" {
                                                t(cx, key).to_string()
                                            } else {
                                                format!(
                                                    "{}  {}",
                                                    operator_label(op, cx),
                                                    t(cx, key)
                                                )
                                            })
                                            .checked(op == active_operator)
                                            .on_click(move |_, _, cx| {
                                                let _ = owner.update(cx, |this, cx| {
                                                    if let Some(draft) = &mut this.filter_draft
                                                        && let Some(row) = draft
                                                            .rows
                                                            .iter_mut()
                                                            .find(|r| r.id == id)
                                                    {
                                                        row.operator = op.into();
                                                    }
                                                    cx.notify();
                                                });
                                            }),
                                        );
                                    }
                                    menu
                                }),
                        )
                        .when(!null_operator(&row.operator), |el| {
                            el.child(
                                div()
                                    .flex_1()
                                    .min_w(px(180.))
                                    .child(row.value.render(locked, cx)),
                            )
                        })
                        .child(
                            Button::new(("filter-remove", id))
                                .ghost()
                                .small()
                                .icon(IconName::Delete)
                                .tooltip(t(cx, "grid.filter.remove"))
                                .disabled(locked)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(draft) = &mut this.filter_draft {
                                        draft.rows.retain(|row| row.id != id);
                                    }
                                    cx.notify();
                                })),
                        ),
                );
            }
            bar = bar.child(rows);
        }
        bar.into_any_element()
    }
}

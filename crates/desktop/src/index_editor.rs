use super::*;
use gpui_component::checkbox::Checkbox;

struct Field {
    name: String,
    desc: bool,
    prefix: Entity<InputState>,
}
pub(super) struct Editor {
    owner: WeakEntity<Indexes>,
    table: Object,
    schema: Schema,
    previous: Option<String>,
    kind: DatabaseKind,
    database: String,
    name: Entity<InputState>,
    search: Entity<InputState>,
    fields: Vec<Field>,
    unique: bool,
    naming: bool,
    generated_name: Option<String>,
    scroll: ScrollHandle,
}
impl Editor {
    pub(super) fn new(
        owner: WeakEntity<Indexes>,
        target: OpenTable,
        table: Object,
        schema: Schema,
        existing: Option<Object>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (kind, database) = (target.kind, target.database);
        let previous = existing.map(|object| object.name);
        let fields = previous
            .as_ref()
            .and_then(|name| schema.index_columns.get(name))
            .map(|columns| {
                columns
                    .iter()
                    .map(|column| Field {
                        name: column.name.clone(),
                        desc: column.descending.unwrap_or(false),
                        prefix: cx.new(|cx| {
                            InputState::new(window, cx).default_value(
                                column.prefix.map(|p| p.to_string()).unwrap_or_default(),
                            )
                        }),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let unique = previous
            .as_ref()
            .and_then(|name| {
                schema
                    .indexes
                    .values
                    .iter()
                    .find(|row| row.first().and_then(Option::as_deref) == Some(name.as_str()))
            })
            .is_some_and(|row| {
                schema
                    .indexes
                    .columns
                    .iter()
                    .position(|key| key.eq_ignore_ascii_case("Non_unique"))
                    .and_then(|i| row.get(i))
                    .and_then(Option::as_deref)
                    == Some("0")
            });
        Self {
            owner,
            table,
            schema,
            previous: previous.clone(),
            kind,
            database,
            name: cx
                .new(|cx| InputState::new(window, cx).default_value(previous.unwrap_or_default())),
            search: cx
                .new(|cx| InputState::new(window, cx).placeholder(t(cx, "compare.search_fields"))),
            fields,
            unique,
            naming: false,
            generated_name: None,
            scroll: ScrollHandle::new(),
        }
    }
    fn columns(&self) -> Vec<(String, bool)> {
        self.fields
            .iter()
            .map(|field| (field.name.clone(), field.desc))
            .collect()
    }
    fn generate_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name =
            index_design::suggest_name(self.kind, &self.columns(), self.unique, &self.schema);
        self.generated_name = Some(name.clone());
        self.name
            .update(cx, |input, cx| input.set_value(name, window, cx));
    }
    fn finish(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut prefixes = std::collections::BTreeMap::new();
        for field in &self.fields {
            let value = field.prefix.read(cx).value();
            if !value.trim().is_empty() {
                let Ok(length) = value.trim().parse::<u64>() else {
                    feedback::error(t(cx, "indexes.invalid"), cx);
                    return;
                };
                prefixes.insert(field.name.clone(), length);
            }
        }
        let draft = Draft {
            name: self.name.read(cx).value().trim().into(),
            columns: self.columns(),
            prefixes,
            unique: self.unique,
            previous: self.previous.clone(),
            drop_only: false,
        };
        if let Err(error) =
            index_design::sql(self.kind, &self.database, &self.table, &self.schema, &draft)
        {
            feedback::error(crate::locale::message(cx, &error), cx);
            return;
        }
        let result = self.owner.update(cx, |owner, cx| {
            if !owner.current(cx) {
                return false;
            }
            owner.stage(self.table.clone(), self.schema.clone(), draft, cx);
            true
        });
        if matches!(result, Ok(true)) {
            window.close_dialog(cx);
        } else {
            feedback::error(t(cx, "dbwork.context.changed"), cx);
        }
    }
}
impl Render for Editor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = div().v_flex().gap_3().min_w_0();
        if self.naming {
            body = body
                .child(
                    div()
                        .h_flex()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .child(Input::new(&self.name).cleanable(true).w_full()),
                        )
                        .child(
                            Button::new("generate-index-name")
                                .ghost()
                                .icon(crate::assets::refresh_icon())
                                .tooltip(t(cx, "indexes.generate_name"))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.generate_name(window, cx)
                                })),
                        ),
                )
                .child(format!(
                    "{} ({})",
                    self.table.name,
                    self.columns()
                        .iter()
                        .map(|(name, desc)| format!("{name}{}", if *desc { " DESC" } else { "" }))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
        } else {
            let available = self
                .schema
                .columns
                .iter()
                .map(|column| column.name.clone())
                .filter(|name| !self.fields.iter().any(|field| &field.name == name))
                .collect::<Vec<_>>();
            let query = self.search.clone();
            let owner = cx.entity().downgrade();
            body = body
                .child(
                    Checkbox::new("design-index-unique")
                        .label(t(cx, "indexes.unique"))
                        .checked(self.unique)
                        .on_click(cx.listener(|this, value, _, cx| {
                            this.unique = *value;
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .h_flex()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .child(Input::new(&self.search).cleanable(true).w_full()),
                        )
                        .child(
                            Button::new("add-index-field")
                                .icon(IconName::Plus)
                                .label(t(cx, "indexes.add_column"))
                                .dropdown_menu(move |mut menu, _, cx| {
                                    let term = query.read(cx).value().to_lowercase();
                                    for name in available
                                        .iter()
                                        .filter(|name| name.to_lowercase().contains(&term))
                                        .take(200)
                                    {
                                        let name = name.clone();
                                        let owner = owner.clone();
                                        menu =
                                            menu.item(PopupMenuItem::new(name.clone()).on_click(
                                                move |_, window, cx| {
                                                    let _ = owner.update(cx, |this, cx| {
                                                        if this.fields.len() < 32
                                                            && !this
                                                                .fields
                                                                .iter()
                                                                .any(|field| field.name == name)
                                                        {
                                                            this.fields.push(Field {
                                                                name: name.clone(),
                                                                desc: false,
                                                                prefix: cx.new(|cx| {
                                                                    InputState::new(window, cx)
                                                                }),
                                                            });
                                                            cx.notify();
                                                        }
                                                    });
                                                },
                                            ));
                                    }
                                    menu.scrollable(true).max_h(px(280.))
                                }),
                        ),
                );
            let prefix = self.kind == DatabaseKind::MySql;
            let header = div()
                .h_flex()
                .gap_2()
                .child(div().w(px(38.)).child(t(cx, "indexes.position")))
                .child(div().w(px(200.)).child(t(cx, "ddl.column.name")))
                .child(div().w(px(74.)).child(t(cx, "indexes.direction")))
                .when(prefix, |row| {
                    row.child(div().w(px(110.)).child(t(cx, "indexes.prefix")))
                });
            let mut rows = div()
                .v_flex()
                .gap_1()
                .min_w(px(if prefix { 590. } else { 480. }))
                .child(header);
            for (index, field) in self.fields.iter().enumerate() {
                let can_prefix = prefix
                    && self
                        .schema
                        .columns
                        .iter()
                        .find(|c| c.name == field.name)
                        .is_some_and(|c| {
                            ["char", "text", "binary", "blob"]
                                .iter()
                                .any(|ty| c.data_type.to_lowercase().contains(ty))
                        });
                rows = rows.child(
                    div()
                        .h_flex()
                        .gap_2()
                        .items_center()
                        .child(div().w(px(38.)).child((index + 1).to_string()))
                        .child(div().w(px(200.)).truncate().child(field.name.clone()))
                        .child(
                            Button::new(("field-order", index))
                                .ghost()
                                .label(if field.desc { "DESC" } else { "ASC" })
                                .w(px(74.))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(field) = this.fields.get_mut(index) {
                                        field.desc = !field.desc;
                                        cx.notify();
                                    }
                                })),
                        )
                        .when(prefix, |row| {
                            row.child(
                                div().w(px(110.)).child(
                                    Input::new(&field.prefix)
                                        .cleanable(true)
                                        .w_full()
                                        .disabled(!can_prefix),
                                ),
                            )
                        })
                        .child(
                            Button::new(("field-up", index))
                                .ghost()
                                .icon(IconName::ChevronUp)
                                .tooltip(t(cx, "indexes.move_up"))
                                .disabled(index == 0)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if index > 0 && index < this.fields.len() {
                                        this.fields.swap(index, index - 1);
                                        cx.notify();
                                    }
                                })),
                        )
                        .child(
                            Button::new(("field-down", index))
                                .ghost()
                                .icon(IconName::ChevronDown)
                                .tooltip(t(cx, "indexes.move_down"))
                                .disabled(index + 1 == self.fields.len())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if index + 1 < this.fields.len() {
                                        this.fields.swap(index, index + 1);
                                        cx.notify();
                                    }
                                })),
                        )
                        .child(
                            Button::new(("field-remove", index))
                                .ghost()
                                .icon(IconName::Close)
                                .tooltip(t(cx, "indexes.remove_field"))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if index < this.fields.len() {
                                        this.fields.remove(index);
                                        cx.notify();
                                    }
                                })),
                        ),
                );
            }
            body = body.child(div().h(px(280.)).child(crate::content_scroll::view(
                "index-field-design-scroll",
                &self.scroll,
                rows,
            )));
        }
        body.child(div().text_xs().child(t(cx, "indexes.rebuild_hint")))
            .child(
                feedback::actions()
                    .when(self.naming, |actions| {
                        actions.child(
                            Button::new("index-back")
                                .ghost()
                                .label(t(cx, "indexes.back"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.naming = false;
                                    cx.notify();
                                })),
                        )
                    })
                    .child(feedback::close_button(cx))
                    .child(
                        Button::new("index-design-next")
                            .primary()
                            .label(t(
                                cx,
                                if self.naming {
                                    "indexes.stage"
                                } else {
                                    "indexes.next_name"
                                },
                            ))
                            .disabled(self.fields.is_empty())
                            .on_click(cx.listener(|this, _, window, cx| {
                                if this.naming {
                                    this.finish(window, cx);
                                } else {
                                    if this.previous.is_none()
                                        && (this.name.read(cx).value().is_empty()
                                            || this.generated_name.as_deref()
                                                == Some(this.name.read(cx).value().as_str()))
                                    {
                                        this.generate_name(window, cx);
                                    }
                                    this.naming = true;
                                    cx.notify();
                                }
                            })),
                    ),
            )
    }
}

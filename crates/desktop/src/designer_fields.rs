use super::*;
use gpui_component::checkbox::Checkbox;
use std::{cell::Cell, rc::Rc};
use turbodbn_services::database::table_design::{self, ColumnDraft};
impl Designer {
    pub(super) fn edit_mysql(
        &self,
        row: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let model = self.model.read(cx);
        let Some(schema) = model.schema.clone() else {
            return;
        };
        let target = model.target.clone();
        let old = row.and_then(|r| schema.columns.get(r)).cloned();
        let extra = match &old {
            Some(old) => match table_design::mysql_extra(old, &schema.ddl) {
                Ok(extra) => extra,
                Err(error) => {
                    feedback::error(crate::locale::message(cx, &error), cx);
                    return;
                }
            },
            None => String::new(),
        };
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(old.as_ref().map(|c| c.name.clone()).unwrap_or_default())
        });
        let data_type = cx.new(|cx| {
            InputState::new(window, cx).default_value(
                old.as_ref()
                    .map(|c| c.data_type.clone())
                    .unwrap_or_else(|| "varchar(255)".into()),
            )
        });
        let extra = cx.new(|cx| InputState::new(window, cx).default_value(extra));
        let comment = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(old.as_ref().map(|c| c.comment.clone()).unwrap_or_default())
        });
        let flags = [
            old.as_ref().and_then(|c| c.not_null).unwrap_or(false),
            old.as_ref().and_then(|c| c.auto_increment).unwrap_or(false),
            old.as_ref().is_some_and(|c| c.primary),
        ]
        .map(|v| Rc::new(Cell::new(v)));
        let types = cx.new(|cx| TypeChoices::new(&data_type, target.kind, false, window, cx));
        let owner = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let fields = (
                name.clone(),
                data_type.clone(),
                extra.clone(),
                comment.clone(),
            );
            let flags = flags.clone();
            let submit = flags.clone();
            let owner = owner.clone();
            let old = old.clone();
            let target = target.clone();
            let schema = schema.clone();
            dialog
                .title(t(
                    cx,
                    if old.is_some() {
                        "designer.edit"
                    } else {
                        "designer.add"
                    },
                ))
                .w(px(620.))
                .overlay_closable(false)
                .child(
                    div()
                        .v_flex()
                        .gap_3()
                        .child(t(cx, "ddl.column.name"))
                        .child(Input::new(&name).cleanable(true))
                        .child(t(cx, "grid.structure.type"))
                        .child(
                            div()
                                .h_flex()
                                .w_full()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .child(Input::new(&data_type).cleanable(true).w_full()),
                                )
                                .child(div().w(px(180.)).flex_shrink_0().child(types.clone())),
                        )
                        .child(
                            div().h_flex().gap_3().children(
                                [
                                    "ddl.column.not_null",
                                    "ddl.column.auto_increment",
                                    "ddl.column.primary",
                                ]
                                .iter()
                                .enumerate()
                                .map(|(i, key)| {
                                    let value = flags[i].clone();
                                    Checkbox::new(("design-column-flag", i))
                                        .label(t(cx, key))
                                        .checked(value.get())
                                        .on_click(move |checked, _, cx| {
                                            value.set(*checked);
                                            cx.refresh_windows();
                                        })
                                }),
                            ),
                        )
                        .child(t(cx, "ddl.column.extra"))
                        .child(feedback::editable_multiline(&extra, false, cx).h(px(76.)))
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(t(cx, "designer.extra_hint")),
                        )
                        .child(t(cx, "ddl.column.comment"))
                        .child(Input::new(&comment).cleanable(true))
                        .child(
                            feedback::actions().child(feedback::close_button(cx)).child(
                                Button::new("design-column-save")
                                    .icon(IconName::Check)
                                    .label(t(cx, "dbwork.column.preview"))
                                    .primary()
                                    .on_click(move |_, window, cx| {
                                        let draft = ColumnDraft {
                                            name: fields.0.read(cx).value().to_string(),
                                            data_type: fields.1.read(cx).value().to_string(),
                                            extra: fields.2.read(cx).value().to_string(),
                                            comment: fields.3.read(cx).value().to_string(),
                                            not_null: submit[0].get(),
                                            auto_increment: submit[1].get(),
                                            primary: submit[2].get(),
                                        };
                                        match table_design::mysql_edit(
                                            &target.database,
                                            &target.object,
                                            &schema,
                                            old.as_ref(),
                                            &draft,
                                        ) {
                                            Ok(sql) => {
                                                window.close_dialog(cx);
                                                let _ = owner.update(cx, |this, cx| {
                                                    this.preview(sql.clone(), window, cx)
                                                });
                                            }
                                            Err(error) => feedback::error(
                                                crate::locale::message(cx, &error),
                                                cx,
                                            ),
                                        }
                                    }),
                            ),
                        ),
                )
        });
    }
}

/// Kept alive with the dialog so filtering survives dialog renders.
pub(super) struct TypeChoices {
    picker:
        Entity<gpui_component::select::SelectState<gpui_component::select::SearchableVec<String>>>,
    _subscription: Subscription,
}
impl TypeChoices {
    pub(super) fn new(
        input: &Entity<InputState>,
        kind: DatabaseKind,
        definition: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        use gpui_component::select::{SearchableVec, SelectEvent, SelectState};
        let picker = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(
                    table_design::type_presets(kind)
                        .iter()
                        .map(|v| (*v).to_string())
                        .collect::<Vec<_>>(),
                ),
                None,
                window,
                cx,
            )
            .searchable(true)
        });
        let input = input.clone();
        let subscription = cx.subscribe_in(
            &picker,
            window,
            move |_, _, event: &SelectEvent<SearchableVec<String>>, window, cx| {
                if let SelectEvent::Confirm(Some(preset)) = event {
                    let value = if definition {
                        table_design::replace_type(kind, &input.read(cx).value(), preset)
                    } else {
                        Ok(preset.clone())
                    };
                    match value {
                        Ok(value) => {
                            input.update(cx, |input, cx| input.set_value(value, window, cx))
                        }
                        Err(error) => feedback::error(crate::locale::message(cx, &error), cx),
                    }
                }
            },
        );
        Self {
            picker,
            _subscription: subscription,
        }
    }
}
impl Render for TypeChoices {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        gpui_component::select::Select::new(&self.picker)
            .w(px(180.))
            .flex_shrink_0()
            .menu_width(px(300.))
            .placeholder(t(cx, "designer.type_templates"))
            .search_placeholder(t(cx, "designer.type_search"))
    }
}

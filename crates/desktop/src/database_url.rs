use super::*;
use std::{cell::Cell, rc::Rc};
use turbodbn_services::connection_url;

struct PreviewRow {
    draft: connection_url::Draft,
    selected: Rc<Cell<bool>>,
    name: Entity<InputState>,
    project: Entity<InputState>,
    environment: Entity<InputState>,
    database: Entity<InputState>,
    user: Entity<InputState>,
}
impl DatabaseView {
    pub(super) fn paste_connection_urls(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx));
        input.update(cx, |input, cx| input.set_masked(true, window, cx));
        let shown = Rc::new(Cell::new(false));
        let help = Rc::new(Cell::new(false));
        let view = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let input = input.clone();
            let view = view.clone();
            let clipboard_view = view.clone();
            let keyboard_view = view.clone();
            let reveal = input.clone();
            let shown = shown.clone();
            let help = help.clone();
            dialog
                .title(crate::locale::t(cx, "url.import"))
                .w(px(720.))
                .overlay_closable(false)
                .child(crate::locale::t(cx, "url.paste_hint"))
                .child(
                    Button::new("url-format-help")
                        .ghost()
                        .icon(IconName::Info)
                        .label(crate::locale::t(cx, "url.format_help"))
                        .tooltip(crate::locale::t(cx, "url.format_tip"))
                        .on_click({
                            let help = help.clone();
                            move |_, _, cx| {
                                help.set(!help.get());
                                cx.refresh_windows();
                            }
                        }),
                )
                .when(help.get(), |dialog| {
                    dialog.child(
                        div()
                            .w(px(670.))
                            .v_flex()
                            .gap_2()
                            .max_h(px(230.))
                            .id("url-format-examples")
                            .overflow_y_scroll()
                            .child(
                                div()
                                    .whitespace_normal()
                                    .child(crate::locale::t(cx, "url.format_rules")),
                            )
                            .child(
                                div()
                                    .v_flex()
                                    .gap_1()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .whitespace_normal()
                                    .children(
                                        crate::locale::t(cx, "url.format_examples").lines().map(
                                            |line| div().whitespace_normal().child(line.to_owned()),
                                        ),
                                    ),
                            ),
                    )
                })
                .child(
                    Button::new("url-from-clipboard")
                        .icon(IconName::Copy)
                        .label(crate::locale::t(cx, "url.clipboard"))
                        .on_click(move |_, window, cx| {
                            let _ = clipboard_view
                                .update(cx, |this, cx| this.paste_url_clipboard(window, cx));
                        }),
                )
                .child(
                    div()
                        .id("url-input")
                        .capture_action(move |_: &gpui_component::input::Paste, window, cx| {
                            cx.stop_propagation();
                            let _ = keyboard_view
                                .update(cx, |this, cx| this.paste_url_clipboard(window, cx));
                        })
                        .child(Input::new(&input).cleanable(true)),
                )
                .child(
                    Button::new("url-reveal")
                        .ghost()
                        .icon(IconName::Eye)
                        .label(crate::locale::t(cx, "url.reveal"))
                        .on_click(move |_, window, cx| {
                            shown.set(!shown.get());
                            reveal.update(cx, |i, cx| i.set_masked(!shown.get(), window, cx));
                        }),
                )
                .footer(move |_, _, _, cx| {
                    let input = input.clone();
                    let view = view.clone();
                    vec![
                        feedback::footer_actions()
                            .child(feedback::close_button(cx))
                            .child(
                                Button::new("url-parse")
                                    .primary()
                                    .icon(IconName::Check)
                                    .label(crate::locale::t(cx, "url.preview"))
                                    .on_click(move |_, window, cx| {
                                        let text = input.read(cx).value().to_string();
                                        let _ = view.update(cx, |this, cx| {
                                            this.preview_url_text(&text, window, cx)
                                        });
                                    }),
                            ),
                    ]
                })
        });
    }
    fn paste_url_clipboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) else {
            feedback::error(crate::locale::t(cx, "url.error.empty"), cx);
            return;
        };
        self.preview_url_text(&text, window, cx);
    }
    fn preview_url_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        match connection_url::parse(text) {
            Ok(rows) if !rows.is_empty() => {
                window.close_dialog(cx);
                self.preview_connection_urls(rows, window, cx);
            }
            Ok(_) => feedback::error(crate::locale::t(cx, "url.error.empty"), cx),
            Err(key) => feedback::error(crate::locale::t(cx, key), cx),
        }
    }
    fn preview_connection_urls(
        &mut self,
        parsed: Vec<connection_url::Row>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut entries = Vec::new();
        let mut errors = Vec::new();
        let mut names = self.profiles.clone();
        names.extend(self.url_drafts.iter().map(|(p, _)| p.clone()));
        for row in parsed {
            match row.draft {
                Ok(mut draft) => {
                    draft.profile.name = generated_connection_name(&draft.profile, &names);
                    names.push(draft.profile.clone());
                    let field = |value: String, window: &mut Window, cx: &mut Context<Self>| {
                        cx.new(|cx| InputState::new(window, cx).default_value(value))
                    };
                    entries.push(PreviewRow {
                        name: field(draft.profile.name.clone(), window, cx),
                        project: field(String::new(), window, cx),
                        environment: field(draft.profile.environment.clone(), window, cx),
                        database: field(draft.profile.database.clone(), window, cx),
                        user: field(draft.profile.user.clone(), window, cx),
                        selected: Rc::new(Cell::new(draft.notices.is_empty())),
                        draft,
                    });
                }
                Err(key) => errors.push((row.line, key)),
            }
        }
        let rows = Rc::new(entries);
        let errors = Rc::new(errors);
        let view = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let mut list = div()
                .id("url-preview-list")
                .v_flex()
                .gap_3()
                .max_h(px(420.))
                .overflow_y_scroll();
            for (line, key) in errors.iter() {
                list = list.child(div().text_color(cx.theme().danger).child(crate::locale::tr(
                    cx,
                    "url.error.row",
                    &[
                        ("line", &line.to_string()),
                        ("reason", crate::locale::t(cx, key).as_ref()),
                    ],
                )));
            }
            for (index, row) in rows.iter().enumerate() {
                let draft = &row.draft;
                let selected = row.selected.clone();
                list = list.child(
                    div()
                        .v_flex()
                        .gap_2()
                        .child(
                            gpui_component::checkbox::Checkbox::new(("url-selected", index))
                                .checked(selected.get())
                                .label(format!(
                                    "{} · {}:{} · {}",
                                    draft.profile.driver_label(),
                                    draft.profile.host,
                                    draft.profile.port,
                                    draft.profile.user
                                ))
                                .on_click(move |value, _, cx| {
                                    selected.set(*value);
                                    cx.refresh_windows();
                                }),
                        )
                        .child(
                            div()
                                .h_flex()
                                .gap_3()
                                .child(
                                    div()
                                        .flex_1()
                                        .child(crate::locale::t(cx, "dbwork.connection.name"))
                                        .child(Input::new(&row.name).cleanable(true)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .child(crate::locale::t(cx, "url.project"))
                                        .child(Input::new(&row.project).cleanable(true)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .child(crate::locale::t(cx, "url.environment"))
                                        .child(Input::new(&row.environment).cleanable(true)),
                                ),
                        )
                        .child(crate::locale::t(cx, "dbwork.connections.database"))
                        .child(Input::new(&row.database).cleanable(true))
                        .child(crate::locale::t(
                            cx,
                            if matches!(
                                draft.profile.kind,
                                DatabaseKind::MySql | DatabaseKind::PostgreSql
                            ) {
                                "dbwork.import.user"
                            } else {
                                "url.user"
                            },
                        ))
                        .child(Input::new(&row.user).cleanable(true))
                        .children(draft.notices.iter().map(|key| {
                            div()
                                .text_sm()
                                .whitespace_normal()
                                .text_color(cx.theme().warning)
                                .child(crate::locale::t(cx, key))
                        }))
                        .when(draft.suggested_database, |el| {
                            el.child(crate::locale::t(cx, "url.database_suggestion"))
                        })
                        .when(draft.profile.driver_id == "mongodb", |el| {
                            el.child(format!("authSource: {}", draft.profile.mongo_auth_source))
                        }),
                );
            }
            let rows = rows.clone();
            let view = view.clone();
            dialog
                .title(crate::locale::t(cx, "url.preview"))
                .w(px(780.))
                .overlay_closable(false)
                .child(crate::locale::t(cx, "url.draft_hint"))
                .child(list)
                .footer(move |_, _, _, cx| {
                    let rows = rows.clone();
                    let view = view.clone();
                    vec![
                        feedback::footer_actions()
                            .child(feedback::close_button(cx))
                            .child(
                                Button::new("url-create-drafts")
                                    .primary()
                                    .icon(IconName::Plus)
                                    .label(crate::locale::t(cx, "url.create"))
                                    .disabled(!rows.iter().any(|r| r.selected.get()))
                                    .on_click(move |_, window, cx| {
                                        let mut drafts = Vec::new();
                                        for row in rows.iter().filter(|r| r.selected.get()) {
                                            let draft = &row.draft;
                                            let mut profile = draft.profile.clone();
                                            profile.name =
                                                row.name.read(cx).value().trim().to_string();
                                            profile.group =
                                                row.project.read(cx).value().trim().to_string();
                                            profile.environment =
                                                row.environment.read(cx).value().trim().to_string();
                                            profile.database =
                                                row.database.read(cx).value().trim().to_string();
                                            profile.user =
                                                row.user.read(cx).value().trim().to_string();
                                            if let Some(key) =
                                                required_connection_field(&profile, false)
                                            {
                                                feedback::error(crate::locale::t(cx, key), cx);
                                                return;
                                            }
                                            if profile.validate().is_err() {
                                                feedback::error(
                                                    crate::locale::t(cx, "url.error.invalid"),
                                                    cx,
                                                );
                                                return;
                                            }
                                            drafts.push((profile, draft.password.clone()));
                                        }
                                        let _ = view.update(cx, |this, cx| {
                                            if this.saving {
                                                return;
                                            }
                                            if this.url_drafts.len() + drafts.len() > 100 {
                                                feedback::error(
                                                    crate::locale::t(cx, "url.error.draft_limit"),
                                                    cx,
                                                );
                                                return;
                                            }
                                            this.keep_url_draft(cx);
                                            let first = drafts.first().map(|(p, _)| p.id);
                                            this.url_drafts.extend(drafts);
                                            window.close_dialog(cx);
                                            if let Some(id) = first {
                                                this.open_url_draft(id, window, cx);
                                            }
                                        });
                                    }),
                            ),
                    ]
                })
        });
    }
    fn keep_url_draft(&mut self, cx: &App) {
        if let Some(index) = self
            .url_drafts
            .iter()
            .position(|(p, _)| p.id == self.editing.id)
            && let Ok(profile) = self.raw_draft(cx)
        {
            self.url_drafts[index] = (
                profile,
                self.fields["password"].read(cx).value().to_string(),
            );
        }
    }
    fn open_url_draft(&mut self, id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        self.keep_url_draft(cx);
        if !self.url_drafts.iter().any(|(p, _)| p.id == self.editing.id)
            && let Ok(profile) = self.raw_draft(cx)
        {
            let key = profile.driver_spec().map(|d| d.id).unwrap_or("").to_owned();
            self.typed_drafts.insert(
                key,
                (
                    profile,
                    self.fields["password"].read(cx).value().to_string(),
                ),
            );
        }
        if let Some((profile, password)) = self.url_drafts.iter().find(|(p, _)| p.id == id).cloned()
        {
            self.fill(profile, window, cx);
            if !self.profiles.iter().any(|p| p.id == id) {
                self.baseline.name.clear();
            }
            self.fields["password"].update(cx, |i, cx| i.set_value(password, window, cx));
            self.tab = 0;
        }
    }
    pub(super) fn url_draft_picker(&self, cx: &Context<Self>) -> AnyElement {
        let drafts = self
            .url_drafts
            .iter()
            .map(|(p, _)| (p.id, p.name.clone()))
            .collect::<Vec<_>>();
        let view = cx.entity().downgrade();
        Button::new("url-draft-picker")
            .icon(IconName::ChevronDown)
            .label(crate::locale::tr(
                cx,
                "url.drafts",
                &[("count", &drafts.len().to_string())],
            ))
            .disabled(self.saving)
            .dropdown_menu(move |mut menu, _, _| {
                for (id, name) in &drafts {
                    let id = *id;
                    let view = view.clone();
                    menu = menu.item(PopupMenuItem::new(name.clone()).on_click(
                        move |_, window, cx| {
                            let _ = view.update(cx, |this, cx| {
                                if !this.saving {
                                    this.open_url_draft(id, window, cx);
                                }
                            });
                        },
                    ));
                }
                menu
            })
            .into_any_element()
    }
}

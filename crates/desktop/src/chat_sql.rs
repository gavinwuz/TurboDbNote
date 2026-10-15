use super::*;
use crate::{
    feedback::{self, SqlDraft},
    locale::t,
};
use gpui_component::WindowExt;

fn draft_text(request: &SqlDraft, cx: &App) -> String {
    let fence = "~".repeat(
        request
            .sql
            .split(|c| c != '~')
            .map(str::len)
            .max()
            .unwrap_or(0)
            .max(2)
            + 1,
    );
    crate::locale::tr(
        cx,
        "notes.ai_prompt",
        &[
            ("connection", &t(cx, "notes.ai_safe_scope")),
            ("database", &request.scope.database),
            ("kind", &request.scope.kind),
            ("sql", &request.sql),
            ("fence", &fence),
        ],
    )
    .to_string()
}
impl ChatView {
    pub(super) fn ready_sql_connection(&self, cx: &mut Context<Self>) {
        if let Some(id) = self.sql_scope.as_ref().and_then(|scope| scope.connection)
            && let Some(db) = self.databases.as_ref()
        {
            let _ = db.update(cx, |db, cx| db.ensure_note_connection(id, false, cx));
        }
    }
    pub(crate) fn accept_sql_draft(
        &mut self,
        request: SqlDraft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if request.sql.len() > 65536 {
            feedback::error(t(cx, "chat.error.input_limit"), cx);
            return;
        }
        let text = draft_text(&request, cx);
        if text.len() > 65536 {
            feedback::error(t(cx, "chat.error.input_limit"), cx);
            return;
        }
        let old = self.input.read(cx).value().to_string();
        if old.trim().is_empty() {
            self.install_sql_draft(request, text, window, cx);
            return;
        }
        let same_scope = self
            .sql_scope
            .as_ref()
            .is_none_or(|scope| scope == &request.scope);
        let engine = self.engine;
        let owner = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let owner = owner.clone();
            let request = request.clone();
            let text = text.clone();
            let old = old.clone();
            dialog
                .title(t(cx, "notes.ai_existing"))
                .child(t(
                    cx,
                    if same_scope {
                        "notes.ai_existing_body"
                    } else {
                        "notes.ai_different_scope"
                    },
                ))
                .footer(move |_, _, _, cx| {
                    let mut row = feedback::actions()
                        .child(feedback::close_button(cx).label(t(cx, "notes.ai_cancel")));
                    for append in [true, false] {
                        let owner = owner.clone();
                        let request = request.clone();
                        let text = text.clone();
                        let old = old.clone();
                        row = row.child(
                            Button::new(if append {
                                "append-sql-draft"
                            } else {
                                "replace-sql-draft"
                            })
                            .icon(if append {
                                IconName::Plus
                            } else {
                                IconName::Check
                            })
                            .label(t(
                                cx,
                                if append {
                                    "notes.ai_append"
                                } else {
                                    "notes.ai_replace"
                                },
                            ))
                            .disabled(append && !same_scope)
                            .when(!append, |button| button.primary())
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                let _ = owner.update(cx, |this, cx| {
                                    if this.engine != engine
                                        || this.input.read(cx).value().as_str() != old
                                    {
                                        feedback::error(t(cx, "notes.ai_draft_changed"), cx);
                                        return;
                                    }
                                    let text = if append {
                                        format!("{old}\n\n{text}")
                                    } else {
                                        text.clone()
                                    };
                                    if text.len() > 65536 {
                                        feedback::error(t(cx, "chat.error.input_limit"), cx);
                                        return;
                                    }
                                    this.install_sql_draft(request.clone(), text, window, cx);
                                });
                            }),
                        );
                    }
                    vec![row]
                })
        });
    }
    fn install_sql_draft(
        &mut self,
        request: SqlDraft,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sql_scope.as_ref() != Some(&request.scope) {
            self.reset_sql_access();
        }
        self.sql_scope = Some(request.scope);
        self.ready_sql_connection(cx);
        self.input.update(cx, |input, cx| {
            input.set_value(text, window, cx);
            input.focus(window, cx);
        });
        feedback::success(t(cx, "notes.ai_ready"), cx);
        cx.notify();
    }
}

impl ChatView {
    pub(super) fn sql_connection_menu(
        &self,
        cx: &Context<Self>,
    ) -> impl Fn(
        gpui_component::menu::PopupMenu,
        &mut Window,
        &mut Context<gpui_component::menu::PopupMenu>,
    ) -> gpui_component::menu::PopupMenu
    + 'static {
        let profiles = self
            .databases
            .as_ref()
            .and_then(|db| db.upgrade())
            .map(|db| db.read(cx).note_profiles())
            .unwrap_or_default();
        let owner = cx.entity().downgrade();
        move |mut menu, _, _| {
            for p in &profiles {
                let owner = owner.clone();
                let p = p.clone();
                menu = menu.item(
                    crate::connection_choice::item(&p).on_click(move |_, _, cx| {
                        let _ = owner.update(cx, |this, cx| {
                            if this.busy {
                                return;
                            }
                            this.reset_sql_access();
                            this.sql_scope = Some(crate::feedback::SqlScope {
                                connection: Some(p.id),
                                name: p.name.clone(),
                                database: if p.kind == turbodbn_services::DatabaseKind::Sqlite {
                                    std::path::Path::new(&p.sqlite_path)
                                        .file_name()
                                        .unwrap_or_default()
                                        .to_string_lossy()
                                        .into_owned()
                                } else {
                                    p.database.clone()
                                },
                                kind: p.driver_label().into(),
                            });
                            if let Some(db) = this.databases.as_ref() {
                                let _ = db
                                    .update(cx, |db, cx| db.ensure_note_connection(p.id, true, cx));
                            }
                            cx.notify();
                        });
                    }),
                );
            }
            menu.scrollable(true).min_w(px(360.)).max_h(px(320.))
        }
    }
    pub(super) fn sql_database_menu(
        &self,
        cx: &Context<Self>,
    ) -> impl Fn(
        gpui_component::menu::PopupMenu,
        &mut Window,
        &mut Context<gpui_component::menu::PopupMenu>,
    ) -> gpui_component::menu::PopupMenu
    + 'static {
        let scope = self.sql_scope.clone();
        let names = scope
            .as_ref()
            .and_then(|s| s.connection)
            .and_then(|id| {
                self.databases
                    .as_ref()
                    .and_then(|db| db.upgrade())
                    .map(|db| db.read(cx).note_databases(id))
            })
            .unwrap_or_default();
        let owner = cx.entity().downgrade();
        move |mut menu, _, _| {
            for name in &names {
                let name = name.clone();
                let owner = owner.clone();
                let scope = scope.clone();
                menu = menu.item(PopupMenuItem::new(name.clone()).on_click(move |_, _, cx| {
                    let _ = owner.update(cx, |this, cx| {
                        if this.busy || this.sql_scope != scope {
                            return;
                        }
                        this.reset_sql_access();
                        if let Some(scope) = &mut this.sql_scope {
                            scope.database = name.clone();
                        }
                        cx.notify();
                    });
                }));
            }
            menu.scrollable(true).max_h(px(320.))
        }
    }
}

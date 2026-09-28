#[path = "import_discovery.rs"]
mod discovery;
use super::*;
use std::rc::Rc;
use turbodbn_services::database::connection_import::{self, Source};
struct ImportRow {
    selected: Rc<std::cell::Cell<bool>>,
    profile: Profile,
    discovery: Option<Entity<discovery::Discovery>>,
    user: Entity<InputState>,
    database: Entity<InputState>,
}
impl DatabaseView {
    pub(super) fn import_connections(
        &mut self,
        navicat: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.saving || self.storage_error {
            return;
        }
        if navicat {
            let view = cx.entity().downgrade();
            window.open_dialog(cx, move |dialog, _, cx| {
                let mut body = div()
                    .v_flex()
                    .gap_2()
                    .child(crate::locale::t(cx, "dbwork.import.choose_driver"));
                for kind in DatabaseKind::ALL {
                    let view = view.clone();
                    body = body.child(
                        Button::new(kind.label())
                            .icon(navigation::type_icon(kind))
                            .label(kind.label())
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                let _ = view.update(cx, |this, cx| {
                                    this.pick_import(Source::Navicat(kind), window, cx)
                                });
                            }),
                    );
                }
                dialog
                    .title(crate::locale::t(cx, "dbwork.import.navicat"))
                    .child(body)
                    .footer(|_, _, _, cx| {
                        vec![feedback::footer_actions().child(feedback::close_button(cx))]
                    })
            });
        } else {
            self.pick_import(Source::DBeaver, window, cx);
        }
    }
    fn pick_import(&mut self, source: Source, window: &mut Window, cx: &mut Context<Self>) {
        let file = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(crate::locale::t(cx, "dbwork.import.select_file")),
        });
        cx.spawn_in(window, async move |view, cx| {
            let Ok(Ok(Some(paths))) = file.await else {
                return;
            };
            let Some(path) = paths.first().cloned() else {
                return;
            };
            let result = cx
                .background_executor()
                .spawn(async move { connection_import::read(&path, source) })
                .await;
            let _ = cx.update(|window, cx| {
                let _ = view.update(cx, |this, cx| match result {
                    Ok(report) => this.preview_import(report, window, cx),
                    Err(error) => feedback::error(error, cx),
                });
            });
        })
        .detach();
    }
    fn preview_import(
        &mut self,
        mut report: connection_import::Report,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if report.profiles.is_empty() {
            feedback::error(crate::locale::t(cx, "dbwork.import.empty"), cx);
            return;
        }
        connection_import::unique_names(&mut report.profiles, &self.profiles);
        let rows = Rc::new(
            report
                .profiles
                .into_iter()
                .map(|profile| {
                    let user = cx.new(|cx| {
                        InputState::new(window, cx)
                            .default_value(profile.user.clone())
                            .placeholder(crate::locale::t(cx, "dbwork.import.user"))
                    });
                    let database = cx.new(|cx| {
                        InputState::new(window, cx)
                            .default_value(if profile.kind == DatabaseKind::Sqlite {
                                profile.sqlite_path.clone()
                            } else {
                                profile.database.clone()
                            })
                            .placeholder(crate::locale::t(cx, "dbwork.connections.database"))
                    });
                    let discovery = if profile.kind == DatabaseKind::Sqlite {
                        None
                    } else {
                        let mut target = profile.clone();
                        if target.legacy_library.is_empty()
                            && let Some(path) = self.drivers.read(cx).path()
                        {
                            target.legacy_library = path.display().to_string();
                        }
                        Some(cx.new(|cx| {
                            discovery::Discovery::new(
                                target,
                                user.clone(),
                                database.clone(),
                                window,
                                cx,
                            )
                        }))
                    };
                    ImportRow {
                        selected: Rc::new(std::cell::Cell::new(true)),
                        profile,
                        user,
                        database,
                        discovery,
                    }
                })
                .collect::<Vec<_>>(),
        );
        let view = cx.entity().downgrade();
        let skipped = report.skipped;
        window.open_dialog(cx, move |dialog, _, cx| {
            let mut list = div()
                .id("connection-import-list")
                .v_flex()
                .gap_3()
                .max_h(px(360.))
                .overflow_y_scroll();
            for (index, row) in rows.iter().enumerate() {
                let selected = row.selected.clone();
                list = list.child(
                    div()
                        .v_flex()
                        .gap_1()
                        .child(
                            gpui_component::checkbox::Checkbox::new(("import-select", index))
                                .label(format!(
                                    "{} · {}",
                                    row.profile.name,
                                    row.profile.kind.label()
                                ))
                                .checked(selected.get())
                                .on_click(move |value, _, cx| {
                                    selected.set(*value);
                                    cx.refresh_windows();
                                }),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(if row.profile.kind == DatabaseKind::Sqlite {
                                    crate::locale::t(cx, "dbwork.import.sqlite_target").to_string()
                                } else {
                                    format!("{}:{}", row.profile.host, row.profile.port)
                                }),
                        )
                        .child(if let Some(discovery) = &row.discovery {
                            discovery.clone().into_any_element()
                        } else {
                            Input::new(&row.database).cleanable(true).into_any_element()
                        })
                        .when(
                            matches!(
                                row.profile.kind,
                                DatabaseKind::MySql | DatabaseKind::PostgreSql
                            ),
                            |el| el.child(Input::new(&row.user).cleanable(true)),
                        ),
                );
            }
            let rows = rows.clone();
            let view = view.clone();
            dialog
                .title(crate::locale::t(cx, "dbwork.connections.import"))
                .w(px(620.))
                .overlay_closable(false)
                .child(crate::locale::tr(
                    cx,
                    "dbwork.import.summary",
                    &[
                        ("count", &rows.len().to_string()),
                        ("skipped", &skipped.to_string()),
                    ],
                ))
                .child(crate::locale::t(cx, "dbwork.connections.import_notice"))
                .child({
                    let all = rows.clone();
                    let none = rows.clone();
                    div()
                        .h_flex()
                        .gap_2()
                        .child(
                            Button::new("import-select-all")
                                .label(crate::locale::t(cx, "settings2.select_all"))
                                .on_click(move |_, _, cx| {
                                    for row in all.iter() {
                                        row.selected.set(true);
                                    }
                                    cx.refresh_windows();
                                }),
                        )
                        .child(
                            Button::new("import-select-none")
                                .label(crate::locale::t(cx, "settings2.select_none"))
                                .on_click(move |_, _, cx| {
                                    for row in none.iter() {
                                        row.selected.set(false);
                                    }
                                    cx.refresh_windows();
                                }),
                        )
                })
                .child(list)
                .footer(move |_, _, _, cx| {
                    let rows = rows.clone();
                    let view = view.clone();
                    vec![
                        feedback::footer_actions()
                            .child(feedback::close_button(cx))
                            .child(
                                Button::new("save-imported-connections")
                                    .primary()
                                    .icon(gpui_component::Icon::default().path("icons/import.svg"))
                                    .label(crate::locale::t(cx, "dbwork.connections.import"))
                                    .disabled(!rows.iter().any(|row| row.selected.get()))
                                    .on_click(move |_, window, cx| {
                                        let mut profiles = rows
                                            .iter()
                                            .filter(|row| row.selected.get())
                                            .map(|row| {
                                                let mut p = row.profile.clone();
                                                p.user =
                                                    row.user.read(cx).value().trim().to_string();
                                                if p.kind == DatabaseKind::Sqlite {
                                                    p.sqlite_path = row
                                                        .database
                                                        .read(cx)
                                                        .value()
                                                        .trim()
                                                        .to_string();
                                                } else {
                                                    p.database = row
                                                        .database
                                                        .read(cx)
                                                        .value()
                                                        .trim()
                                                        .to_string();
                                                }
                                                if let Some(discovery) = &row.discovery {
                                                    discovery.read(cx).apply_tested_tls(&mut p);
                                                }
                                                p
                                            })
                                            .collect::<Vec<_>>();
                                        if profiles.is_empty() {
                                            return;
                                        }
                                        if let Some(view) = view.upgrade() {
                                            connection_import::unique_names(
                                                &mut profiles,
                                                &view.read(cx).profiles,
                                            );
                                        }
                                        for p in &profiles {
                                            if let Some(key) = required_connection_field(p, false) {
                                                feedback::error(crate::locale::t(cx, key), cx);
                                                return;
                                            }
                                            if let Err(error) = p.validate() {
                                                feedback::error(error, cx);
                                                return;
                                            }
                                        }
                                        let _ = view.update(cx, |this, cx| {
                                            if this.saving || this.storage_error {
                                                return;
                                            }
                                            this.saving = true;
                                            let file = this.file.clone();
                                            cx.spawn(async move |view, cx| {
                                                let result = cx
                                                    .background_executor()
                                                    .spawn(async move {
                                                        connection_store::import_profiles(
                                                            &file, &profiles,
                                                        )
                                                        .map(|history| (profiles, history))
                                                    })
                                                    .await;
                                                let _ = view.update(cx, |this, cx| {
                                                    this.saving = false;
                                                    match result {
                                                        Ok((profiles, history)) => {
                                                            let count = profiles.len();
                                                            this.profiles.extend(profiles);
                                                            this.history.extend(history);
                                                            feedback::success(
                                                                crate::locale::tr(
                                                                    cx,
                                                                    "dbwork.import.done",
                                                                    &[(
                                                                        "count",
                                                                        &count.to_string(),
                                                                    )],
                                                                ),
                                                                cx,
                                                            );
                                                        }
                                                        Err(error) => feedback::error(error, cx),
                                                    }
                                                    cx.notify();
                                                });
                                            })
                                            .detach();
                                        });
                                        window.close_dialog(cx);
                                    }),
                            ),
                    ]
                })
        });
    }
}

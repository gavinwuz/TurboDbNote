use super::*;
use gpui_component::{
    Sizable, WindowExt,
    menu::{ContextMenuExt, PopupMenu},
    select::{SearchableVec, Select},
};

#[derive(Clone, Copy)]
pub(crate) enum FileAction {
    Rename,
    SaveAs,
    Reveal,
    Project,
}

pub(super) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 240
        && !name.ends_with(['.', ' '])
        && !name
            .chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
        && !matches!(
            name.to_ascii_uppercase().split('.').next().unwrap_or(""),
            "CON"
                | "PRN"
                | "AUX"
                | "NUL"
                | "COM1"
                | "COM2"
                | "COM3"
                | "COM4"
                | "COM5"
                | "COM6"
                | "COM7"
                | "COM8"
                | "COM9"
                | "LPT1"
                | "LPT2"
                | "LPT3"
                | "LPT4"
                | "LPT5"
                | "LPT6"
                | "LPT7"
                | "LPT8"
                | "LPT9"
        )
}
pub(super) fn cell_types() -> [(usize, &'static str); 3] {
    [
        (0, "notes.add_sql"),
        (1, "notes.add_markdown"),
        (2, "notes.add_task"),
    ]
}
pub(super) fn cell_icon(kind: usize, cx: &App) -> AnyElement {
    if kind == 2 {
        crate::assets::task_icon(cx)
    } else {
        crate::assets::note_icon(kind, cx)
    }
}
impl NoteView {
    pub(super) fn select_script_target(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let owner = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, window, cx| {
            let scope = owner
                .update(cx, |note, cx| note.toolbar_scope(window, cx))
                .unwrap_or_else(|_| div().into_any_element());
            dialog
                .title(t(cx, "script.select_target"))
                .w(px(600.))
                .overlay_closable(false)
                .child(
                    div()
                        .v_flex()
                        .gap_3()
                        .child(t(cx, "script.error.database_mismatch"))
                        .child(scope)
                        .child(t(cx, "script.select_target_hint"))
                        .child(feedback::actions().child(feedback::close_button(cx))),
                )
        });
    }
    pub(super) fn sync_defaults(&mut self, cx: &mut Context<Self>) {
        let database = self.default_database.read(cx).value().trim().to_owned();
        for cell in &self.cells {
            cell.update(cx, |cell, cx| {
                cell.set_defaults(self.note.default_connection_id, database.clone(), cx)
            });
        }
    }
    fn defaults_locked(&self, cx: &App) -> bool {
        self.active_queries(cx) || self.cells.iter().any(|c| c.read(cx).result_pending(cx))
    }
    pub(super) fn add_cell(&mut self, kind: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.batch.is_some() || self.cells.len() >= note_store::MAX_CELLS {
            return;
        }
        let content = match kind {
            0 => CellContent::Sql {
                source: String::new(),
                connection_id: None,
                database: None,
            },
            1 => CellContent::Markdown {
                source: String::new(),
            },
            _ => CellContent::Task {
                description: String::new(),
                items: vec![],
                status: TaskStatus::Pending,
                due_at: None,
                remind_at: None,
                linked_cells: vec![],
                connection_id: None,
            },
        };
        let cell = self.push_cell(Cell::new("", content), window, cx);
        self.activate_cell(cell.entity_id(), window, cx);
        self.changed(cx);
    }
    pub(super) fn render_toolbar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .h_flex()
            .flex_wrap()
            .gap_1()
            .min_h(px(32.))
            .border_b_1()
            .border_color(cx.theme().border)
            .child(self.toolbar_actions(cx))
            .child(div().flex_1())
            .child(self.toolbar_scope(window, cx))
            .into_any_element()
    }
    fn toolbar_actions(&self, cx: &Context<Self>) -> AnyElement {
        let active = self.active_queries(cx);
        let view = cx.entity();
        div()
            .h_flex()
            .gap_1()
            .child(
                crate::assets::note_tool("note-new-cell-menu")
                    .w(px(32.))
                    .icon(
                        gpui_component::Icon::new(IconName::Plus)
                            .text_color(crate::assets::note_color(0, cx)),
                    )
                    .dropdown_caret(true)
                    .tooltip(t(cx, "notes.new_cell"))
                    .disabled(self.batch.is_some() || self.cells.len() >= note_store::MAX_CELLS)
                    .dropdown_menu(move |mut menu, _, _| {
                        for (kind, key) in cell_types() {
                            let view = view.clone();
                            menu = menu.item(
                                PopupMenuItem::element(move |_, cx| {
                                    div()
                                        .h_flex()
                                        .gap_2()
                                        .child(cell_icon(kind, cx))
                                        .child(t(cx, key))
                                })
                                .on_click(move |_, window, cx| {
                                    view.update(cx, |this, cx| this.add_cell(kind, window, cx))
                                }),
                            );
                        }
                        menu
                    }),
            )
            .child(
                crate::assets::note_tool("save-note")
                    .icon(
                        gpui_component::Icon::default()
                            .path("icons/note-save.svg")
                            .text_color(crate::assets::note_color(0, cx)),
                    )
                    .tooltip(t(cx, "notes.save"))
                    .disabled(!self.dirty() || self.saving)
                    .opacity(if !self.dirty() || self.saving {
                        0.4
                    } else {
                        1.
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.save_now(cx))),
            )
            .child(
                crate::assets::note_tool("run-all-note-cells")
                    .icon(
                        gpui_component::Icon::default()
                            .path("icons/note-run-all.svg")
                            .text_color(crate::assets::note_color(1, cx)),
                    )
                    .tooltip(self.batch_label(cx))
                    .disabled(active || !self.cells.iter().any(|c| c.read(cx).has_sql(cx)))
                    .on_click(cx.listener(|this, _, window, cx| this.run_all(window, cx))),
            )
            .child(
                crate::assets::note_tool("stop-all-note-cells")
                    .icon(
                        gpui_component::Icon::default()
                            .path("icons/stop.svg")
                            .text_color(crate::assets::note_color(3, cx)),
                    )
                    .tooltip(t(cx, "notes.stop_all"))
                    .disabled(!active)
                    .opacity(if active { 1. } else { 0.4 })
                    .on_click(cx.listener(|this, _, _, cx| this.stop_queries(cx))),
            )
            .child(
                crate::assets::note_tool("import-note-sql")
                    .icon(
                        gpui_component::Icon::default()
                            .path("icons/note-import-sql.svg")
                            .text_color(crate::assets::note_color(0, cx)),
                    )
                    .tooltip(t(cx, "script.import"))
                    .disabled(
                        active
                            || self.picking
                            || self.saving
                            || self.cells.len() >= note_store::MAX_CELLS,
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.import_sql(window, cx))),
            )
            .child(
                crate::assets::note_tool("collapse-all-note-cells")
                    .icon(gpui_component::Icon::default().path("icons/note-collapse.svg"))
                    .tooltip(t(cx, "notes.collapse_all"))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.collapse_all(true, window, cx)),
                    ),
            )
            .child(
                crate::assets::note_tool("expand-all-note-cells")
                    .icon(gpui_component::Icon::default().path("icons/note-expand.svg"))
                    .tooltip(t(cx, "notes.expand_all"))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.collapse_all(false, window, cx)),
                    ),
            )
            .into_any_element()
    }
    fn toolbar_scope(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let profiles = self.connections.read(cx).note_profiles();
        let selected = profiles
            .iter()
            .find(|p| Some(p.id) == self.note.default_connection_id);
        let sqlite = selected.is_some_and(|p| p.kind == turbodbn_services::DatabaseKind::Sqlite);
        let connection = selected
            .map(|p| p.name.clone())
            .unwrap_or_else(|| t(cx, "notes.default_connection").to_string());
        let sqlite_name = selected
            .map(|p| {
                std::path::Path::new(&p.sqlite_path)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            })
            .unwrap_or_default();
        let mut names = self
            .note
            .default_connection_id
            .map(|id| self.connections.read(cx).note_databases(id))
            .unwrap_or_default();
        let value = self.default_database.read(cx).value().to_string();
        if !value.is_empty() {
            names.push(value.clone());
        }
        names.sort();
        names.dedup();
        names.truncate(2000);
        let context = (
            self.note.default_connection_id,
            names.clone(),
            value.clone(),
        );
        if self.default_context.as_ref() != Some(&context) {
            self.default_context = Some(context);
            self.default_picker.update(cx, |picker, cx| {
                picker.set_items(
                    SearchableVec::new(
                        names
                            .into_iter()
                            .map(crate::database::DatabaseChoice)
                            .collect::<Vec<_>>(),
                    ),
                    window,
                    cx,
                );
                picker.set_selected_value(&value, window, cx);
            });
        }
        let binding = cx.entity();
        let clear_binding = binding.clone();
        div()
            .h_flex()
            .gap_1()
            .flex_wrap()
            .child(
                Button::new("note-default-connection")
                    .small()
                    .ghost()
                    .w(px(160.))
                    .justify_start()
                    .icon(gpui_component::Icon::default().path("icons/database.svg"))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(connection),
                    )
                    .tooltip(t(cx, "notes.default_hint"))
                    .dropdown_caret(true)
                    .disabled(self.defaults_locked(cx))
                    .dropdown_menu(move |mut menu, _, cx| {
                        let clear = clear_binding.clone();
                        menu = menu.item(PopupMenuItem::new(t(cx, "notes.no_default")).on_click(
                            move |_, window, cx| {
                                clear.update(cx, |this, cx| {
                                    if this.defaults_locked(cx) {
                                        return;
                                    }
                                    this.note.default_connection_id = None;
                                    this.default_database
                                        .update(cx, |input, cx| input.set_value("", window, cx));
                                    this.sync_defaults(cx);
                                    this.changed(cx);
                                })
                            },
                        ));
                        for profile in &profiles {
                            let owner = binding.clone();
                            let profile = profile.clone();
                            menu = menu.item(PopupMenuItem::new(profile.name.clone()).on_click(
                                move |_, window, cx| {
                                    owner.update(cx, |this, cx| {
                                        if this.defaults_locked(cx) {
                                            return;
                                        }
                                        this.note.default_connection_id = Some(profile.id);
                                        this.default_database.update(cx, |input, cx| {
                                            input.set_value(profile.database.clone(), window, cx)
                                        });
                                        this.sync_defaults(cx);
                                        this.changed(cx);
                                    })
                                },
                            ));
                        }
                        menu.scrollable(true).max_h(px(280.))
                    }),
            )
            .child(if sqlite {
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(sqlite_name)
                    .into_any_element()
            } else {
                Input::new(&self.default_database)
                    .small()
                    .w(px(280.))
                    .flex_shrink_0()
                    .cleanable(!self.defaults_locked(cx))
                    .disabled(self.defaults_locked(cx))
                    .suffix(
                        Select::new(&self.default_picker)
                            .small()
                            .appearance(false)
                            .menu_width(px(360.))
                            .w(px(28.))
                            .disabled(self.defaults_locked(cx)),
                    )
                    .into_any_element()
            })
            .into_any_element()
    }
    pub(crate) fn file_menu(mut menu: PopupMenu, view: WeakEntity<Self>, cx: &App) -> PopupMenu {
        let locked = view.upgrade().is_none_or(|view| {
            let note = view.read(cx);
            note.saving || note.picking || note.cells.iter().any(|cell| cell.read(cx).indexing(cx))
        });
        for (action, key, icon) in [
            (FileAction::Rename, "notes.rename", IconName::ALargeSmall),
            (FileAction::SaveAs, "notes.save_as", IconName::FolderOpen),
            (FileAction::Project, "notes.project", IconName::Folder),
            (FileAction::Reveal, "notes.reveal", IconName::ExternalLink),
        ] {
            let view = view.clone();
            menu = menu.item(
                PopupMenuItem::new(t(cx, key))
                    .icon(icon)
                    .disabled(locked && !matches!(action, FileAction::Reveal))
                    .on_click(move |_, window, cx| {
                        let _ = view.update(cx, |this, cx| this.file_action(action, window, cx));
                    }),
            );
        }
        menu
    }
    pub(crate) fn file_action(
        &mut self,
        action: FileAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.deleting {
            return;
        }
        match action {
            FileAction::Reveal => crate::feedback::reveal_parent(self.path.clone(), cx),
            FileAction::SaveAs => self.save_as(window, cx),
            FileAction::Rename | FileAction::Project => {
                self.metadata_dialog(matches!(action, FileAction::Project), window, cx)
            }
        }
    }
    fn metadata_dialog(&mut self, project: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving || self.picking || self.cells.iter().any(|cell| cell.read(cx).indexing(cx)) {
            return;
        }
        let value = if project {
            self.note.project.clone()
        } else {
            self.display_title(cx).to_string()
        };
        let input = cx.new(|cx| InputState::new(window, cx).default_value(value));
        let projects = self.connections.read(cx).note_projects();
        let view = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let input = input.clone();
            let apply = input.clone();
            let view = view.clone();
            let projects = projects.clone();
            dialog
                .title(t(
                    cx,
                    if project {
                        "notes.project"
                    } else {
                        "notes.rename"
                    },
                ))
                .w(px(420.))
                .on_ok(|_, _, _| false)
                .child(Input::new(&input).cleanable(true).when(project, |field| {
                    field.suffix(
                        Button::new("note-project-options")
                            .ghost()
                            .icon(IconName::ChevronDown)
                            .dropdown_menu(move |mut menu, _, _| {
                                for name in &projects {
                                    let input = input.clone();
                                    let name = name.clone();
                                    menu = menu.item(PopupMenuItem::new(name.clone()).on_click(
                                        move |_, window, cx| {
                                            input.update(cx, |input, cx| {
                                                input.set_value(name.clone(), window, cx)
                                            })
                                        },
                                    ));
                                }
                                menu.scrollable(true).max_h(px(220.))
                            }),
                    )
                }))
                .footer(move |_, _, _, cx| {
                    let apply = apply.clone();
                    let view = view.clone();
                    vec![
                        feedback::footer_actions()
                            .child(feedback::close_button(cx))
                            .child(
                                Button::new("apply-note-metadata")
                                    .primary()
                                    .icon(IconName::Check)
                                    .label(t(cx, "grid.apply"))
                                    .on_click(move |_, window, cx| {
                                        let value = apply.read(cx).value().trim().to_owned();
                                        if (!project && !valid_name(&value)) || value.len() > 1024 {
                                            feedback::error(t(cx, "notes.invalid_name"), cx);
                                            return;
                                        }
                                        let ok = view
                                            .update(cx, |this, cx| {
                                                if this.saving || this.picking {
                                                    return false;
                                                }
                                                if project {
                                                    this.note.project = value.clone();
                                                    this.changed(cx);
                                                } else {
                                                    let name = if value
                                                        .to_lowercase()
                                                        .ends_with(".note")
                                                    {
                                                        value[..value.len() - 5].to_string()
                                                    } else {
                                                        value.clone()
                                                    };
                                                    if !valid_name(&name) {
                                                        feedback::error(
                                                            t(cx, "notes.invalid_name"),
                                                            cx,
                                                        );
                                                        return false;
                                                    }
                                                    this.note.title = name.clone();
                                                    this.changed(cx);
                                                    if !this.draft {
                                                        let destination = this
                                                            .path
                                                            .with_file_name(format!("{name}.note"));
                                                        if destination != this.path {
                                                            this.debounce = None;
                                                            this.save_to(
                                                                destination,
                                                                true,
                                                                true,
                                                                cx,
                                                            );
                                                        }
                                                    }
                                                }
                                                true
                                            })
                                            .unwrap_or(false);
                                        if ok {
                                            window.close_dialog(cx);
                                        }
                                    }),
                            ),
                    ]
                })
        });
    }
    pub(super) fn render_tab(&self, cx: &Context<Self>) -> AnyElement {
        let status = if !self.error.is_empty() {
            self.error.clone()
        } else {
            t(
                cx,
                if self.saving {
                    "notes.saving"
                } else if self.dirty() {
                    "common.status.unsaved"
                } else {
                    "notes.saved"
                },
            )
            .to_string()
        };
        let project = if self.note.project.is_empty() {
            t(cx, "dbwork.ungrouped").to_string()
        } else {
            self.note.project.clone()
        };
        let info = crate::locale::tr(
            cx,
            "notes.tab_info",
            &[
                ("title", self.display_title(cx).as_ref()),
                ("status", &status),
                ("project", &project),
                ("path", &compact_path(&self.path.display().to_string())),
            ],
        );
        let color = if !self.error.is_empty() || self.dirty() && !self.saving {
            cx.theme().danger
        } else if self.saving {
            cx.theme().warning
        } else {
            cx.theme().success
        };
        let name = self.key.clone();
        let close = name.clone();
        let view = cx.entity().downgrade();
        div()
            .id(SharedString::from(format!("tab-title-{name}")))
            .h_flex()
            .gap_2()
            .child(crate::assets::note_icon(2, cx))
            .child(
                div()
                    .max_w(px(220.))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(self.display_title(cx)),
            )
            .child(div().size(px(7.)).rounded_full().bg(color))
            .tooltip(move |window, cx| {
                let info = info.clone();
                gpui_component::tooltip::Tooltip::element(move |_, _| {
                    div()
                        .v_flex()
                        .max_w(px(520.))
                        .children(info.lines().map(|line| {
                            div()
                                .max_w(px(520.))
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(line.to_owned())
                        }))
                })
                .build(window, cx)
            })
            .child(
                Button::new(SharedString::from(format!("tab-close-{name}")))
                    .ghost()
                    .icon(IconName::Close)
                    .tooltip(t(cx, "common.action.close"))
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        crate::tabs::send(close.clone(), crate::tabs::CloseMode::Current, cx);
                    }),
            )
            .context_menu(move |menu, _, cx| {
                crate::tabs::close_menu(
                    Self::file_menu(menu, view.clone(), cx).separator(),
                    name.clone(),
                    cx,
                )
            })
            .into_any_element()
    }
}

/// Bound tooltip paths by Unicode scalar count, keeping root and filename tail.
fn compact_path(path: &str) -> String {
    let chars = path.chars().collect::<Vec<_>>();
    if chars.len() <= 72 {
        return path.to_owned();
    }
    format!(
        "{}…{}",
        chars[..24].iter().collect::<String>(),
        chars[chars.len() - 47..].iter().collect::<String>()
    )
}
#[cfg(test)]
mod path_tests {
    use super::compact_path;
    #[test]
    fn tooltip_path_is_bounded_and_unicode_safe() {
        assert_eq!(compact_path("C:/notes/test.note"), "C:/notes/test.note");
        let long = format!("C:/笔记/{}/report.note", "项目/".repeat(100));
        let short = compact_path(&long);
        assert_eq!(short.chars().count(), 72);
        assert!(short.starts_with("C:/笔记/"));
        assert!(short.ends_with("/report.note"));
    }
}

use super::*;
use crate::locale::t;
use gpui_component::menu::{DropdownMenu, PopupMenuItem};
#[derive(Default)]
pub(super) struct DdlState {
    origin: Option<Uuid>,
    preferred: Option<(Uuid, String)>,
    context: Option<(Uuid, String, u64)>,
    selected: Option<Object>,
    candidates: Vec<Object>,
    note: Option<feedback::NoteDdlContext>,
    browse: bool,
    designer: Option<Entity<crate::table_designer::Designer>>,
    picker: Option<Entity<crate::ddl_picker::Picker>>,
    subscription: Option<Subscription>,
}
impl DatabaseView {
    pub(crate) fn set_ddl_context(&mut self, event: feedback::DdlContext, cx: &mut Context<Self>) {
        if event.scope.is_none() && event.origin.is_some() && event.origin != self.ddl.origin {
            return;
        }
        self.ddl.origin = event.origin;
        if self.ddl.preferred == event.scope && event.object.is_none() && !event.open {
            return;
        }
        self.ddl.preferred = event.scope;
        self.ddl.context = None;
        self.ddl.selected = event.object;
        self.ddl.candidates = event.candidates;
        self.ddl.note = event.note;
        self.ddl.browse = false;
        self.ddl.designer = None;
        self.ddl.picker = None;
        cx.notify();
    }
    pub fn render_objects(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.ddl.note.is_some() && self.ddl.selected.is_none() && !self.ddl.browse {
            return self.note_ddl_landing(cx);
        }
        if let Some((id, database)) = self.ddl.preferred.clone()
            && self.note_connection_state(id) != Some(true)
        {
            return div()
                .v_flex()
                .p_3()
                .gap_2()
                .child(div().text_sm().child(database))
                .when(self.ddl.note.is_some(), |body| {
                    body.child(self.ddl_selector(cx))
                })
                .child(
                    Button::new("ddl-connect-load")
                        .ghost()
                        .icon(IconName::LoaderCircle)
                        .label(t(cx, "ddl.connect_load"))
                        .disabled(self.note_connection_state(id).is_none())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.ddl.context = None;
                            this.ensure_note_connection(id, true, cx)
                        })),
                )
                .into_any_element();
        }
        let context = self
            .ddl
            .preferred
            .clone()
            .or_else(|| self.selected_database.clone())
            .filter(|(id, _)| self.note_connection_state(*id) == Some(true))
            .map(|(id, db)| (id, db, self.note_generation(id)));
        if context != self.ddl.context {
            if self.ddl.context.is_some() {
                self.ddl.selected = None;
            }
            self.ddl.context = context;
            self.ddl.designer = None;
            self.ddl.picker = None;
            self.ddl.subscription = None;
        }
        if self.ddl.note.is_some() && self.ddl.selected.is_none() && !self.ddl.browse {
            return self.note_ddl_landing(cx);
        }
        let Some((id, database, generation)) = self.ddl.context.clone() else {
            return div()
                .p_3()
                .child(t(cx, "ddl.choose_scope"))
                .into_any_element();
        };
        let Some(profile) = self.note_profile(id).cloned() else {
            return div().into_any_element();
        };
        let scope_label = format!("{} / {}", profile.name, database);
        if profile.kind == DatabaseKind::Redis {
            return div()
                .p_3()
                .child(t(cx, "ddl.unsupported"))
                .into_any_element();
        }
        if let Some(object) = self.ddl.selected.clone() {
            if self.ddl.designer.is_none() {
                let target = OpenTable {
                    connection: id,
                    database,
                    generation,
                    object,
                    mode: crate::table_view::Mode::Design,
                    kind: profile.kind,
                    writable: profile.supports_mutation(),
                    mysql_raw_gbk: profile.mysql_raw_gbk,
                };
                let source = cx.entity().downgrade();
                self.ddl.designer =
                    Some(cx.new(|cx| {
                        crate::table_designer::Designer::new(source, target, window, cx)
                    }));
            }
            return div()
                .v_flex()
                .size_full()
                .min_w_0()
                .min_h_0()
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .truncate()
                        .child(scope_label.clone()),
                )
                .child(self.ddl_selector(cx))
                .child(
                    div().flex_1().min_h_0().min_w_0().child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .min_w_0()
                            .child(self.ddl.designer.as_ref().unwrap().clone()),
                    ),
                )
                .into_any_element();
        }
        if self.ddl.picker.is_none() {
            let source = cx.entity().downgrade();
            let paged = self.paged_catalog_supported(id);
            let picker = cx.new(|cx| {
                crate::ddl_picker::Picker::new(source, id, database, generation, paged, window, cx)
            });
            self.ddl.subscription = Some(cx.subscribe(
                &picker,
                |this, _, event: &crate::ddl_picker::Selected, cx| {
                    if this.ddl.context.as_ref() == Some(&event.scope) {
                        this.ddl.selected = Some(event.object.clone());
                        this.ddl.designer = None;
                        cx.notify();
                    }
                },
            ));
            self.ddl.picker = Some(picker);
        }
        div()
            .v_flex()
            .size_full()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .truncate()
                    .child(scope_label),
            )
            .when(
                !self.ddl.candidates.is_empty() || self.ddl.note.is_some(),
                |body| body.child(self.ddl_selector(cx)),
            )
            .child(self.ddl.picker.as_ref().unwrap().clone())
            .into_any_element()
    }
    fn ddl_selector(&self, cx: &Context<Self>) -> AnyElement {
        let label = self
            .ddl
            .selected
            .as_ref()
            .map(|o| {
                if o.schema.is_empty() {
                    o.name.clone()
                } else {
                    format!("{}.{}", o.schema, o.name)
                }
            })
            .unwrap_or_else(|| t(cx, "ddl.context_tables").to_string());
        if let Some(note) = &self.ddl.note {
            let mut targets = note.tables.clone();
            for target in &note.fallback {
                if !targets
                    .iter()
                    .any(|old| old.database == target.database && old.object == target.object)
                {
                    targets.push(target.clone());
                }
            }
            return self.note_ddl_menu("ddl-note-selection", label.into(), targets, cx);
        }
        let candidates = self.ddl.candidates.clone();
        let owner = cx.entity().downgrade();
        Button::new("ddl-context-table")
            .ghost()
            .label(label)
            .dropdown_caret(true)
            .dropdown_menu(move |mut menu, _, cx| {
                for object in &candidates {
                    let object = object.clone();
                    let target = owner.clone();
                    let label = if object.schema.is_empty() {
                        object.name.clone()
                    } else {
                        format!("{}.{}", object.schema, object.name)
                    };
                    menu = menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                        let object = object.clone();
                        let target = target.clone();
                        cx.defer(move |cx| {
                            let _ = target.update(cx, |this, cx| {
                                this.ddl.selected = Some(object);
                                this.ddl.designer = None;
                                cx.notify();
                            });
                        });
                    }));
                }
                let target = owner.clone();
                menu.item(PopupMenuItem::new(t(cx, "ddl.select_table")).on_click(
                    move |_, _, cx| {
                        let target = target.clone();
                        cx.defer(move |cx| {
                            let _ = target.update(cx, |this, cx| {
                                this.ddl.selected = None;
                                this.ddl.designer = None;
                                cx.notify();
                            });
                        });
                    },
                ))
            })
            .into_any_element()
    }
    fn note_ddl_landing(&self, cx: &Context<Self>) -> AnyElement {
        let note = self.ddl.note.as_ref().unwrap();
        let scope = self
            .ddl
            .preferred
            .as_ref()
            .map(|(id, db)| {
                format!(
                    "{} / {}",
                    self.note_profile(*id)
                        .map(|p| p.name.as_str())
                        .unwrap_or_default(),
                    db
                )
            })
            .unwrap_or_default();
        div()
            .v_flex()
            .gap_2()
            .p_3()
            .min_w_0()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(scope),
            )
            .child(div().whitespace_normal().child(t(
                cx,
                if self.ddl.preferred.is_none() {
                    "ddl.choose_scope"
                } else if note.recognized {
                    "ddl.note.choose"
                } else {
                    "ddl.note.unresolved"
                },
            )))
            .when(!note.tables.is_empty(), |body| {
                body.child(self.note_ddl_menu(
                    "ddl-note-current",
                    t(
                        cx,
                        if note.selection {
                            "ddl.note.selection_tables"
                        } else {
                            "ddl.context_tables"
                        },
                    ),
                    note.tables.clone(),
                    cx,
                ))
            })
            .when(
                !note.fallback.is_empty() && note.fallback != note.tables,
                |body| {
                    body.child(self.note_ddl_menu(
                        "ddl-note-unit",
                        t(cx, "ddl.note.unit_tables"),
                        note.fallback.clone(),
                        cx,
                    ))
                },
            )
            .child(
                Button::new("ddl-note-browse")
                    .ghost()
                    .icon(IconName::Search)
                    .label(t(cx, "ddl.note.browse"))
                    .disabled(self.ddl.preferred.is_none())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.ddl.browse = true;
                        this.ddl.picker = None;
                        cx.notify();
                    })),
            )
            .into_any_element()
    }
    fn note_ddl_menu(
        &self,
        id: &'static str,
        label: SharedString,
        targets: Vec<feedback::NoteDdlTable>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let owner = cx.entity().downgrade();
        let scope = self.ddl.preferred.clone();
        let origin = self.ddl.origin;
        let note = self.ddl.note.clone();
        let generation = scope.as_ref().map(|(id, _)| self.note_generation(*id));
        Button::new(id)
            .ghost()
            .label(label)
            .dropdown_caret(true)
            .dropdown_menu(move |mut menu, _, cx| {
                for target in &targets {
                    let table = if target.object.schema.is_empty() {
                        format!("{}.{}", target.database, target.object.name)
                    } else {
                        format!(
                            "{}.{}.{}",
                            target.database, target.object.schema, target.object.name
                        )
                    };
                    let label = crate::locale::tr(
                        cx,
                        "ddl.note.table_line",
                        &[("table", &table), ("line", &target.line.to_string())],
                    );
                    let owner = owner.clone();
                    let target = target.clone();
                    let scope = scope.clone();
                    let note = note.clone();
                    menu = menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                        let owner = owner.clone();
                        let target = target.clone();
                        let scope = scope.clone();
                        let note = note.clone();
                        cx.defer(move |cx| {
                            let _ = owner.update(cx, |this, cx| {
                                if this.ddl.preferred != scope
                                    || this.ddl.origin != origin
                                    || this.ddl.note != note
                                    || scope.as_ref().map(|(id, _)| this.note_generation(*id))
                                        != generation
                                {
                                    feedback::error(t(cx, "mcp.stale"), cx);
                                    return;
                                }
                                if let Some((id, _)) = scope {
                                    this.ddl.preferred = Some((id, target.database));
                                }
                                this.ddl.context = None;
                                this.ddl.selected = Some(target.object);
                                this.ddl.designer = None;
                                this.ddl.browse = false;
                                cx.notify();
                            });
                        });
                    }));
                }
                let owner = owner.clone();
                let scope = scope.clone();
                let note = note.clone();
                menu.item(
                    PopupMenuItem::new(t(cx, "ddl.note.browse")).on_click(move |_, _, cx| {
                        let owner = owner.clone();
                        let scope = scope.clone();
                        let note = note.clone();
                        cx.defer(move |cx| {
                            let _ = owner.update(cx, |this, cx| {
                                if this.ddl.preferred != scope
                                    || this.ddl.origin != origin
                                    || this.ddl.note != note
                                {
                                    feedback::error(t(cx, "mcp.stale"), cx);
                                    return;
                                }
                                this.ddl.selected = None;
                                this.ddl.designer = None;
                                this.ddl.picker = None;
                                this.ddl.browse = true;
                                cx.notify();
                            });
                        });
                    }),
                )
                .scrollable(true)
                .max_h(px(280.))
                .min_w(px(300.))
            })
            .into_any_element()
    }
}

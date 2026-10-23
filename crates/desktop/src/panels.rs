use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, IconName, StyledExt,
    button::{Button, ButtonVariants},
    dock::{Panel, PanelEvent},
};

pub struct WorkbenchPanel {
    name: &'static str,
    title: &'static str,
    focus: FocusHandle,
    content: PanelContent,
    detail: Option<crate::feedback::DetailRequest>,
    detail_mode: bool,
    detail_grid: Option<Entity<gpui_component::table::TableState<crate::query_grid::QueryGrid>>>,
    detail_json: Option<Entity<crate::result_document::ResultDocument>>,
}

enum PanelContent {
    NoteHome(Entity<crate::note_library::NoteHome>),
    Notes(Entity<crate::note_library::NoteLibrary>),
    Databases(Entity<crate::database::DatabaseView>),
    DatabaseObjects(Entity<crate::database::DatabaseView>),
    Info {
        heading: &'static str,
        body: &'static str,
    },
}

impl WorkbenchPanel {
    pub fn show_detail(
        &mut self,
        detail: crate::feedback::DetailRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rows = turbodbn_services::database::workbench::Rows {
            columns: vec![
                crate::locale::t(cx, "details.field").to_string(),
                crate::locale::t(cx, "details.value").to_string(),
            ],
            values: detail
                .entries
                .iter()
                .map(|(k, v)| vec![Some(k.clone()), Some(v.clone())])
                .collect(),
        };
        self.detail_grid = Some(cx.new(|cx| {
            gpui_component::table::TableState::new(
                crate::query_grid::QueryGrid::new(rows, 0),
                window,
                cx,
            )
        }));
        self.detail_json = Some(cx.new(|cx| {
            crate::result_document::ResultDocument::new(
                turbodbn_services::database::workbench::Rows {
                    columns: vec!["json".into()],
                    values: vec![vec![Some(detail.text.clone())]],
                },
                window,
                cx,
            )
        }));
        let refresh = detail.refresh;
        self.detail = Some(detail);
        if !refresh {
            self.detail_mode = false;
        }
        cx.notify();
    }
    pub fn clear_detail(&mut self, cx: &mut Context<Self>) {
        self.detail = None;
        self.detail_grid = None;
        self.detail_json = None;
        cx.notify();
    }
    pub fn database_objects(
        view: Entity<crate::database::DatabaseView>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            name: "schema",
            title: "database.objects.title",
            focus: cx.focus_handle(),
            detail: None,
            detail_mode: false,
            detail_grid: None,
            detail_json: None,
            content: PanelContent::DatabaseObjects(view),
        }
    }

    pub fn show_databases(
        &mut self,
        view: Entity<crate::database::DatabaseView>,
        cx: &mut Context<Self>,
    ) {
        self.title = "database.navigation.title";
        self.content = PanelContent::Databases(view);
        cx.notify();
    }
    pub fn show_notes(
        &mut self,
        view: Entity<crate::note_library::NoteLibrary>,
        cx: &mut Context<Self>,
    ) {
        self.title = "notes.title";
        self.content = PanelContent::Notes(view);
        cx.notify();
    }
    pub fn notebook(
        library: Entity<crate::note_library::NoteLibrary>,
        cx: &mut Context<Self>,
    ) -> Self {
        let home = cx.new(|cx| crate::note_library::NoteHome::new(library, cx));
        Self {
            name: "notebook",
            title: "notes.title",
            focus: cx.focus_handle(),
            content: PanelContent::NoteHome(home),
            detail: None,
            detail_mode: false,
            detail_grid: None,
            detail_json: None,
        }
    }

    pub fn info(
        name: &'static str,
        title: &'static str,
        heading: &'static str,
        body: &'static str,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            name,
            title,
            focus: cx.focus_handle(),
            detail: None,
            detail_mode: false,
            detail_grid: None,
            detail_json: None,
            content: PanelContent::Info { heading, body },
        }
    }
}

impl EventEmitter<PanelEvent> for WorkbenchPanel {}
impl Focusable for WorkbenchPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for WorkbenchPanel {
    fn toolbar_buttons(&mut self, _: &mut Window, cx: &mut Context<Self>) -> Option<Vec<Button>> {
        (self.name == "notebook").then(|| crate::tabs::new_note_button(self.name, cx))
    }

    fn panel_name(&self) -> &'static str {
        self.name
    }
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let icon = match self.name {
            "notebook" => IconName::BookOpen,
            "navigation" => IconName::Folder,
            "schema" => IconName::LayoutDashboard,
            _ => IconName::Info,
        };
        crate::tabs::title(self.name, icon, crate::locale::t(cx, self.title), cx)
    }
    fn dropdown_menu(
        &mut self,
        menu: gpui_component::menu::PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_component::menu::PopupMenu {
        if self.name == "notebook" {
            crate::tabs::editor_menu(
                crate::tabs::opened_notes(menu, self.name, window, cx).separator(),
                self.name,
                cx,
            )
        } else if let PanelContent::Databases(view) = &self.content {
            view.update(cx, |this, cx| this.navigation_menu(menu, cx))
        } else {
            menu
        }
    }
    fn zoomable(&self, _: &App) -> Option<gpui_component::dock::PanelControl> {
        if self.name == "navigation" {
            None
        } else {
            Some(gpui_component::dock::PanelControl::Menu)
        }
    }
    fn title_suffix(&mut self, _: &mut Window, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if let PanelContent::Databases(view) = &self.content {
            Some(view.update(cx, |v, cx| v.navigation_tools(cx)))
        } else if let PanelContent::Notes(view) = &self.content {
            Some(view.update(cx, |view, cx| view.tools(cx)))
        } else {
            None
        }
    }
    fn closable(&self, _: &App) -> bool {
        false
    }
}

impl Render for WorkbenchPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = div()
            .id(self.name)
            .track_focus(&self.focus)
            .size_full()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .v_flex()
            .gap_4()
            .p_4()
            .text_color(cx.theme().foreground)
            .bg(cx.theme().background);
        if let Some(detail) = self.detail.clone() {
            if !detail.inspector {
                let fence = "~".repeat(
                    detail
                        .text
                        .lines()
                        .map(|line| line.chars().take_while(|c| *c == '~').count())
                        .max()
                        .unwrap_or(0)
                        .max(2)
                        + 1,
                );
                let markdown = format!("{fence}sql\n{}\n{fence}", detail.text);
                let copy = detail.text.clone();
                return content
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(div().flex_1().min_w_0().child(detail.title))
                            .child(
                                Button::new("ddl-copy")
                                    .ghost()
                                    .icon(IconName::Copy)
                                    .tooltip(crate::locale::t(cx, "dbwork.copy.ddl"))
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            copy.clone(),
                                        ));
                                        crate::feedback::success(
                                            crate::locale::t(cx, "feedback.copied"),
                                            cx,
                                        );
                                    }),
                            ),
                    )
                    .child(
                        gpui_component::text::TextView::markdown(
                            "ddl-definition",
                            markdown,
                            window,
                            cx,
                        )
                        .selectable(true),
                    );
            }
            let text = detail.text.clone();
            return content
                .child(
                    div()
                        .h_flex()
                        .gap_2()
                        .child(div().flex_1().min_w_0().child(detail.title))
                        .child(
                            Button::new("details-table")
                                .ghost()
                                .label(crate::locale::t(cx, "grid.structure.table"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.detail_mode = false;
                                    cx.notify();
                                })),
                        )
                        .child(Button::new("details-json").ghost().label("JSON").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.detail_mode = true;
                                cx.notify();
                            }),
                        ))
                        .child(
                            Button::new("details-copy")
                                .ghost()
                                .icon(IconName::Copy)
                                .tooltip(crate::locale::t(cx, "feedback.copy"))
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                                    crate::feedback::success(
                                        crate::locale::t(cx, "feedback.copied"),
                                        cx,
                                    );
                                }),
                        ),
                )
                .when(!self.detail_mode, |el| {
                    el.child(div().flex_1().min_h_0().w_full().child(
                        gpui_component::table::Table::new(self.detail_grid.as_ref().unwrap()),
                    ))
                })
                .when(self.detail_mode, |el| {
                    el.child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .child(self.detail_json.as_ref().unwrap().clone()),
                    )
                });
        }

        match &self.content {
            PanelContent::NoteHome(view) => div()
                .id("note-home-panel")
                .size_full()
                .min_h_0()
                .child(view.clone()),
            PanelContent::Notes(view) => div()
                .id("notes-panel")
                .size_full()
                .min_h_0()
                .child(view.clone()),
            PanelContent::Databases(view) => div()
                .id("database-sidebar-split")
                .size_full()
                .min_h_0()
                .text_color(cx.theme().foreground)
                .bg(cx.theme().background)
                .child(view.update(cx, |view, cx| view.navigation(window, cx))),
            PanelContent::DatabaseObjects(view) => {
                content.child(view.update(cx, |view, cx| view.render_objects(window, cx)))
            }
            PanelContent::Info { heading, body } => content
                .child(
                    div()
                        .text_size(rems(crate::typography::BODY))
                        .child(crate::locale::t(cx, heading)),
                )
                .child(
                    div()
                        .text_size(rems(crate::typography::BODY))
                        .text_color(cx.theme().muted_foreground)
                        .child(crate::locale::t(cx, body)),
                ),
        }
    }
}

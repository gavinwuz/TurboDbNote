use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, IconName, StyledExt,
    button::{Button, ButtonVariants},
    dock::{Panel, PanelEvent},
    input::{Input, InputState},
};

pub struct WorkbenchPanel {
    name: &'static str,
    title: &'static str,
    focus: FocusHandle,
    content: PanelContent,
    detail: Option<crate::feedback::DetailRequest>,
    detail_input: Option<Entity<InputState>>,
    detail_mode: bool,
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
    fn set_detail_value(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.detail_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .default_value(text)
        }));
    }
    pub fn show_detail(
        &mut self,
        detail: crate::feedback::DetailRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.detail_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .default_value(detail.text.clone())
        }));
        self.detail = Some(detail);
        self.detail_mode = false;
        cx.notify();
    }
    pub fn clear_detail(&mut self, cx: &mut Context<Self>) {
        self.detail = None;
        self.detail_input = None;
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
            detail_input: None,
            detail_mode: false,
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
            detail_input: None,
            detail_mode: false,
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
            detail_input: None,
            detail_mode: false,
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
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_component::menu::PopupMenu {
        if self.name == "notebook" {
            crate::tabs::editor_menu(menu, self.name, cx)
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
            .overflow_y_scroll()
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
            let input = self.detail_input.clone().unwrap();
            let text = detail.text.clone();
            return content
                .child(
                    div()
                        .h_flex()
                        .gap_2()
                        .child(div().flex_1().child(detail.title))
                        .child(
                            Button::new("details-table")
                                .ghost()
                                .label(crate::locale::t(cx, "grid.structure.table"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.detail_mode = false;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("details-text")
                                .ghost()
                                .label(crate::locale::t(cx, "grid.structure.text"))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.detail_mode = true;
                                    if let Some(detail) = &this.detail {
                                        this.set_detail_value(detail.text.clone(), window, cx);
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("details-copy")
                                .ghost()
                                .icon(IconName::Copy)
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
                    el.child(
                        div()
                            .id("detail-fields")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .v_flex()
                            .gap_1()
                            .children(detail.entries.into_iter().enumerate().map(
                                |(i, (name, value))| {
                                    let selected = value.clone();
                                    let copy = value.clone();
                                    div()
                                        .h_flex()
                                        .gap_2()
                                        .child(
                                            Button::new(("copy-detail-value", i))
                                                .ghost()
                                                .icon(IconName::Copy)
                                                .tooltip(crate::locale::t(cx, "grid.copy.cell"))
                                                .on_click(move |_, _, cx| {
                                                    cx.write_to_clipboard(
                                                        ClipboardItem::new_string(copy.clone()),
                                                    );
                                                    crate::feedback::success(
                                                        crate::locale::t(cx, "feedback.copied"),
                                                        cx,
                                                    );
                                                }),
                                        )
                                        .child(
                                            Button::new(("detail-field", i))
                                                .ghost()
                                                .label(name)
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        this.set_detail_value(
                                                            selected.clone(),
                                                            window,
                                                            cx,
                                                        );
                                                        cx.notify();
                                                    },
                                                )),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .max_h(px(80.))
                                                .overflow_hidden()
                                                .whitespace_normal()
                                                .child(value.chars().take(300).collect::<String>()),
                                        )
                                },
                            )),
                    )
                })
                .child(
                    Input::new(&input)
                        .disabled(true)
                        .when(self.detail_mode, |input| input.flex_1().min_h_0())
                        .when(!self.detail_mode, |input| input.h(px(320.)).flex_shrink_0()),
                );
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

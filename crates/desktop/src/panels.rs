use crate::cell::CellView;
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, IconName, StyledExt,
    dock::{Panel, PanelEvent},
};
use turbodbn_core::Note;

pub struct WorkbenchPanel {
    name: &'static str,
    title: &'static str,
    focus: FocusHandle,
    content: PanelContent,
}

enum PanelContent {
    Notebook(Vec<Entity<CellView>>),
    Info {
        heading: &'static str,
        body: &'static str,
    },
}

impl WorkbenchPanel {
    pub fn show_navigation(&mut self, connections: bool, cx: &mut Context<Self>) {
        self.title = if connections {
            "navigation.connections"
        } else {
            "navigation.projects"
        };
        self.content = if connections {
            PanelContent::Info {
                heading: "connections.empty.title",
                body: "connections.empty.description",
            }
        } else {
            PanelContent::Info {
                heading: "notebook.welcome.title",
                body: "projects.empty.description",
            }
        };
        cx.notify();
    }

    pub fn notebook(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let cells = Note::welcome()
            .cells
            .into_iter()
            .map(|cell| cx.new(|cx| CellView::new(cell, window, cx)))
            .collect();
        Self {
            name: "notebook",
            title: "notebook.welcome.title",
            focus: cx.focus_handle(),
            content: PanelContent::Notebook(cells),
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
        } else {
            menu
        }
    }
    fn closable(&self, _: &App) -> bool {
        false
    }
}

impl Render for WorkbenchPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
        match &self.content {
            PanelContent::Notebook(cells) => content
                .child(
                    div()
                        .text_size(rems(crate::typography::META))
                        .text_color(cx.theme().muted_foreground)
                        .child(crate::locale::t(cx, "notebook.memory.description")),
                )
                .children(cells.iter().cloned()),
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

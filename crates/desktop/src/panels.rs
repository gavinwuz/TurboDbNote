use crate::cell::CellView;
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, StyledExt,
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
        self.title = if connections { "连接" } else { "项目" };
        self.content = if connections {
            PanelContent::Info {
                heading: "尚未添加连接",
                body: "连接配置集中管理；表、视图与 DDL 位于右侧数据库结构面板。",
            }
        } else {
            PanelContent::Info {
                heading: "入门工作台.note",
                body: "仅管理 .note 文件。当前为示例文档，文件管理将在后续接入。",
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
            title: "入门工作台.note",
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
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.title
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
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("工作区 / 入门工作台 · 本次编辑仅保留在内存中"),
                )
                .children(cells.iter().cloned()),
            PanelContent::Info { heading, body } => {
                content.child(div().text_sm().child(*heading)).child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(*body),
                )
            }
        }
    }
}

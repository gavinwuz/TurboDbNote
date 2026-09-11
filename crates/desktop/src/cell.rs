use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, StyledExt,
    button::Button,
    input::{Input, InputState},
};
use turbodbn_core::{Cell, CellContent, QueryState, TaskStatus};

/// One entity per cell: folding never recreates the editor or its undo history.
pub struct CellView {
    cell: Cell,
    editor: Entity<InputState>,
    collapsed: bool,
    dirty: bool,
    _query: Option<QueryState>,
    _subscription: Subscription,
}

impl CellView {
    pub fn new(cell: Cell, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (source, language) = match &cell.content {
            CellContent::Sql { source, .. } => (source.clone(), Some("sql")),
            CellContent::Markdown { source } => (source.clone(), None),
            CellContent::Task { description, .. } => (description.clone(), None),
        };
        let editor = cx.new(|cx| {
            let state = InputState::new(window, cx)
                .multi_line(true)
                .default_value(source);
            if let Some(language) = language {
                state.code_editor(language)
            } else {
                state
            }
        });
        // The editor's rope owns live text. Materialize a document snapshot only
        // when saving/executing, not an O(document-size) String on every keystroke.
        let subscription = cx.subscribe(&editor, |this, _, event, cx| {
            if matches!(event, gpui_component::input::InputEvent::Change) && !this.dirty {
                this.dirty = true;
                cx.notify();
            }
        });
        let query =
            matches!(cell.content, CellContent::Sql { .. }).then(|| QueryState::new(cell.id));
        Self {
            cell,
            editor,
            collapsed: false,
            dirty: false,
            _query: query,
            _subscription: subscription,
        }
    }
}

impl Render for CellView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let kind = match self.cell.content {
            CellContent::Sql { .. } => "SQL",
            CellContent::Markdown { .. } => "MD",
            CellContent::Task { .. } => "任务",
        };
        let is_sql = matches!(self.cell.content, CellContent::Sql { .. });
        let task_status = match self.cell.content {
            CellContent::Task { status, .. } => Some(status),
            _ => None,
        };
        div()
            .v_flex()
            .gap_3()
            .p_4()
            .border_1()
            .border_color(cx.theme().border)
            .rounded_lg()
            .bg(cx.theme().background)
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(
                        Button::new("fold")
                            .label(if self.collapsed { "+" } else { "−" })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.collapsed = !this.collapsed;
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(kind),
                    )
                    .child(div().flex_1().child(self.cell.title.clone()))
                    .when(self.dirty, |el| {
                        el.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("未保存"),
                        )
                    }),
            )
            .when(!self.collapsed, |el| {
                el.child(Input::new(&self.editor).h(px(if task_status.is_some() {
                    76.
                } else {
                    150.
                })))
                .when(is_sql, |el| {
                    el.child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("尚未连接数据库 · 查询结果将在这里展示"),
                    )
                })
                .when_some(task_status, |el, status| {
                    el.child(
                        Button::new("task-status")
                            .label(match status {
                                TaskStatus::Pending => "待完成",
                                TaskStatus::InProgress => "进行中",
                                TaskStatus::Completed => "已完成",
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let CellContent::Task { status, .. } = &mut this.cell.content {
                                    *status = match status {
                                        TaskStatus::Pending => TaskStatus::InProgress,
                                        TaskStatus::InProgress => TaskStatus::Completed,
                                        TaskStatus::Completed => TaskStatus::Pending,
                                    };
                                    this.dirty = true;
                                    cx.notify();
                                }
                            })),
                    )
                })
            })
    }
}

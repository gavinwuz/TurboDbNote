use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, IconName, StyledExt,
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
            CellContent::Task { .. } => "notebook.kind.task",
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
                            .icon(if self.collapsed {
                                IconName::ChevronRight
                            } else {
                                IconName::ChevronDown
                            })
                            .label(crate::locale::t(
                                cx,
                                if self.collapsed {
                                    "notebook.action.expand"
                                } else {
                                    "notebook.action.collapse"
                                },
                            ))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.collapsed = !this.collapsed;
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .text_size(rems(crate::typography::META))
                            .text_color(cx.theme().muted_foreground)
                            .child(crate::locale::message(cx, kind)),
                    )
                    .child(div().flex_1().child(self.cell.title.clone()))
                    .when(self.dirty, |el| {
                        el.child(
                            div()
                                .text_size(rems(crate::typography::META))
                                .text_color(cx.theme().muted_foreground)
                                .child(crate::locale::t(cx, "common.status.unsaved")),
                        )
                    }),
            )
            .when(!self.collapsed, |el| {
                el.child(
                    Input::new(&self.editor)
                        .text_size(rems(crate::typography::BODY))
                        .h(rems(if task_status.is_some() { 4.75 } else { 9.375 })),
                )
                .when(is_sql, |el| {
                    el.child(
                        div()
                            .text_size(rems(crate::typography::BODY))
                            .text_color(cx.theme().muted_foreground)
                            .child(crate::locale::t(cx, "notebook.results.empty")),
                    )
                })
                .when_some(task_status, |el, status| {
                    el.child(
                        Button::new("task-status")
                            .icon(IconName::CircleCheck)
                            .label(crate::locale::t(
                                cx,
                                match status {
                                    TaskStatus::Pending => "notebook.task.pending",
                                    TaskStatus::InProgress => "notebook.task.in_progress",
                                    TaskStatus::Completed => "notebook.task.completed",
                                },
                            ))
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

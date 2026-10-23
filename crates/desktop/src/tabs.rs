use crate::locale::t;
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Icon, IconName, StyledExt,
    button::{Button, ButtonVariants},
    menu::{ContextMenuExt, PopupMenu, PopupMenuItem},
};

#[derive(Clone, Copy, Debug)]
pub enum CloseMode {
    Current,
    Others,
    Right,
    Left,
    All,
}
#[derive(Clone)]
pub struct CloseTabs {
    pub name: String,
    pub mode: CloseMode,
}
pub struct NewNote(pub String);
#[derive(Clone)]
pub enum LayoutAction {
    Activate(String),
    Select(String),
    Restore(String),
    MoveTo { name: String, pane: usize },
    ClosePane(usize),
    Move { from: String, to: String },
    Split { name: String, left: bool },
}
#[derive(Clone)]
struct TabDrag {
    name: String,
    label: SharedString,
}
impl Render for TabDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().p_2().child(self.label.clone())
    }
}
pub(crate) fn layout(action: LayoutAction, cx: &mut App) {
    let events = cx.global::<TabRouter>().0.clone();
    events.update(cx, |_, cx| cx.emit(action));
}

pub struct TabEvents;
impl EventEmitter<LayoutAction> for TabEvents {}
impl EventEmitter<NewNote> for TabEvents {}
impl EventEmitter<CloseTabs> for TabEvents {}
#[derive(Clone)]
pub struct OpenNote {
    pub key: String,
    pub title: String,
    pub project: String,
    pub connection: String,
    pub position: String,
    pub dirty: bool,
}
pub struct TabRouter(pub Entity<TabEvents>, pub usize, pub Vec<OpenNote>);
impl Global for TabRouter {}
pub fn is_fixed(name: &str) -> bool {
    matches!(name, "navigation" | "ai-chat" | "properties" | "schema")
}
pub fn targets<T: AsRef<str> + Clone>(names: &[T], name: &str, mode: CloseMode) -> Vec<T> {
    if is_fixed(name) {
        return vec![];
    }
    let Some(index) = names.iter().position(|item| item.as_ref() == name) else {
        return vec![];
    };
    names
        .iter()
        .enumerate()
        .filter_map(|(i, name)| {
            let keep = match mode {
                CloseMode::Current => i == index,
                CloseMode::Others => i != index,
                CloseMode::Right => i > index,
                CloseMode::Left => i < index,
                CloseMode::All => true,
            };
            (keep && !is_fixed(name.as_ref())).then_some(name.clone())
        })
        .collect()
}
pub(crate) fn send(name: String, mode: CloseMode, cx: &mut App) {
    let events = cx.global::<TabRouter>().0.clone();
    events.update(cx, |_, cx| cx.emit(CloseTabs { name, mode }));
}
pub fn editor_menu(menu: PopupMenu, name: impl Into<String>, cx: &App) -> PopupMenu {
    let name = name.into();
    menu.item(
        PopupMenuItem::new(t(cx, "tabs.close.all"))
            .icon(IconName::Delete)
            .on_click(move |_, _, cx| send(name.clone(), CloseMode::All, cx)),
    )
}
pub fn title(
    name: impl Into<String>,
    icon: IconName,
    label: impl Into<SharedString>,
    cx: &App,
) -> AnyElement {
    let name = name.into();
    let close_name = name.clone();
    let activate_name = name.clone();
    let drop_name = name.clone();
    let label = label.into();
    let title = div()
        .id(SharedString::from(format!("tab-title-{name}")))
        .h_flex()
        .gap_2()
        .child(Icon::new(icon))
        .child(label.clone());
    if is_fixed(&name) {
        return title.into_any_element();
    }
    title
        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
            layout(LayoutAction::Activate(activate_name.clone()), cx)
        })
        .on_drag(
            TabDrag {
                name: name.clone(),
                label,
            },
            |drag, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| drag.clone())
            },
        )
        .on_drop(move |drag: &TabDrag, _, cx| {
            layout(
                LayoutAction::Move {
                    from: drag.name.clone(),
                    to: drop_name.clone(),
                },
                cx,
            )
        })
        .child(
            Button::new(SharedString::from(format!("tab-close-{name}")))
                .ghost()
                .icon(IconName::Close)
                .tooltip(t(cx, "common.action.close"))
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    send(close_name.clone(), CloseMode::Current, cx);
                }),
        )
        .context_menu(move |menu, _, cx| close_menu(menu, name.clone(), cx))
        .into_any_element()
}
pub(crate) fn close_menu(mut menu: PopupMenu, name: String, cx: &App) -> PopupMenu {
    if !is_fixed(&name) {
        for (key, left) in [("tabs.split.left", true), ("tabs.split.right", false)] {
            let name = name.clone();
            menu = menu.item(
                PopupMenuItem::new(t(cx, key))
                    .icon(if left {
                        IconName::ArrowLeft
                    } else {
                        IconName::ArrowRight
                    })
                    .on_click(move |_, _, cx| {
                        layout(
                            LayoutAction::Split {
                                name: name.clone(),
                                left,
                            },
                            cx,
                        )
                    }),
            );
        }
        let origin = name.clone();
        if cx.try_global::<TabRouter>().is_some_and(|r| r.1 > 1) {
            menu = menu
                .item(
                    PopupMenuItem::new(t(cx, "tabs.restore_single"))
                        .icon(IconName::Minimize)
                        .on_click(move |_, _, cx| {
                            layout(LayoutAction::Restore(origin.clone()), cx)
                        }),
                )
                .separator();
        }
    }
    for (label, icon, mode) in [
        ("common.action.close", IconName::Close, CloseMode::Current),
        ("tabs.close.others", IconName::Delete, CloseMode::Others),
        ("tabs.close.right", IconName::ArrowRight, CloseMode::Right),
        ("tabs.close.left", IconName::ArrowLeft, CloseMode::Left),
        ("tabs.close.all", IconName::Delete, CloseMode::All),
    ] {
        let name = name.clone();
        menu = menu.item(
            PopupMenuItem::new(t(cx, label))
                .icon(icon)
                .on_click(move |_, _, cx| send(name.clone(), mode, cx)),
        );
    }
    menu
}
pub fn new_note_button(origin: impl Into<String>, cx: &App) -> Vec<Button> {
    let origin = origin.into();
    let restore_origin = origin.clone();
    let mut buttons = vec![
        Button::new("tab-new-note")
            .icon(IconName::Plus)
            .tooltip(t(cx, "notes.new"))
            .on_click(move |_, _, cx| {
                let events = cx.global::<TabRouter>().0.clone();
                events.update(cx, |_, cx| cx.emit(NewNote(origin.clone())));
            }),
    ];
    if cx.try_global::<TabRouter>().is_some_and(|r| r.1 > 1) {
        buttons.push(
            Button::new("tab-restore-single")
                .icon(IconName::Minimize)
                .tooltip(t(cx, "tabs.restore_single"))
                .on_click(move |_, _, cx| {
                    layout(LayoutAction::Restore(restore_origin.clone()), cx)
                }),
        );
    }
    buttons
}

pub(crate) fn draggable(
    name: String,
    label: SharedString,
    element: Stateful<Div>,
) -> Stateful<Div> {
    let activate = name.clone();
    let target = name.clone();
    element
        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
            layout(LayoutAction::Activate(activate.clone()), cx)
        })
        .on_drag(TabDrag { name, label }, |drag, _, _, cx| {
            cx.stop_propagation();
            cx.new(|_| drag.clone())
        })
        .on_drop(move |drag: &TabDrag, _, cx| {
            layout(
                LayoutAction::Move {
                    from: drag.name.clone(),
                    to: target.clone(),
                },
                cx,
            )
        })
}

pub(crate) struct EmptyPane {
    pane: usize,
    focus: FocusHandle,
}
impl EmptyPane {
    pub fn new(pane: usize, cx: &mut Context<Self>) -> Self {
        Self {
            pane,
            focus: cx.focus_handle(),
        }
    }
}
impl EventEmitter<gpui_component::dock::PanelEvent> for EmptyPane {}
impl Focusable for EmptyPane {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl gpui_component::dock::Panel for EmptyPane {
    fn panel_name(&self) -> &'static str {
        "empty-center"
    }
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        t(cx, "tabs.empty")
    }
    fn closable(&self, _: &App) -> bool {
        false
    }
    fn zoomable(&self, _: &App) -> Option<gpui_component::dock::PanelControl> {
        None
    }
    fn toolbar_buttons(&mut self, _: &mut Window, cx: &mut Context<Self>) -> Option<Vec<Button>> {
        let mut buttons = new_note_button(format!("empty-center-{}", self.pane), cx);
        let pane = self.pane;
        buttons.push(
            Button::new("empty-close-pane")
                .icon(IconName::Close)
                .tooltip(t(cx, "tabs.close.pane"))
                .on_click(move |_, _, cx| layout(LayoutAction::ClosePane(pane), cx)),
        );
        Some(buttons)
    }
}
impl Render for EmptyPane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pane = self.pane;
        div()
            .id("empty-center-drop")
            .v_flex()
            .size_full()
            .justify_center()
            .items_center()
            .gap_2()
            .child(t(cx, "tabs.empty_hint"))
            .child(
                Button::new("empty-new-note")
                    .icon(IconName::Plus)
                    .label(t(cx, "notes.new"))
                    .on_click(move |_, _, cx| {
                        let events = cx.global::<TabRouter>().0.clone();
                        events.update(cx, |_, cx| cx.emit(NewNote(format!("empty-center-{pane}"))));
                    }),
            )
            .on_drop(move |drag: &TabDrag, _, cx| {
                layout(
                    LayoutAction::MoveTo {
                        name: drag.name.clone(),
                        pane,
                    },
                    cx,
                )
            })
    }
}

pub(crate) fn opened_notes(
    mut menu: PopupMenu,
    origin: &str,
    window: &mut Window,
    cx: &mut App,
) -> PopupMenu {
    let notes = cx.global::<TabRouter>().2.clone();
    let mut terms = Vec::new();
    for note in notes {
        let key = note.key.clone();
        let project = if note.project.is_empty() {
            t(cx, "dbwork.ungrouped").to_string()
        } else {
            note.project
        };
        terms.push(format!("{} {} {}", note.title, project, note.connection));
        let info = format!("{} · {} · {}", note.title, project, note.connection);
        let row_id = key.clone();
        menu = menu.item(
            PopupMenuItem::element(move |_, cx| {
                let info = info.clone();
                div()
                    .id(SharedString::from(format!("opened-note-{row_id}")))
                    .tooltip(move |window, cx| {
                        gpui_component::tooltip::Tooltip::new(info.clone()).build(window, cx)
                    })
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .w(px(520.))
                    .min_w_0()
                    .text_sm()
                    .child(div().flex_1().min_w_0().truncate().child(format!(
                        "{}{}",
                        if note.dirty { "● " } else { "" },
                        note.title
                    )))
                    .child(
                        div()
                            .w(px(110.))
                            .flex_shrink_0()
                            .truncate()
                            .text_color(cx.theme().muted_foreground)
                            .child(project.clone()),
                    )
                    .child(
                        div()
                            .w(px(38.))
                            .flex_shrink_0()
                            .text_color(cx.theme().muted_foreground)
                            .child(note.position.clone()),
                    )
            })
            .checked(note.key == origin)
            .on_click(move |_, window, cx| {
                let key = key.clone();
                window.defer(cx, move |_, cx| layout(LayoutAction::Select(key), cx));
            }),
        );
    }
    menu.min_w(px(360.))
        .max_w(px(640.))
        .max_h(px(420.))
        .searchable(t(cx, "notes.opened.search"), terms, window, cx)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn closing_respects_order_and_boundaries() {
        let names = ["a", "b", "c"];
        assert_eq!(targets(&names, "b", CloseMode::Others), vec!["a", "c"]);
        assert_eq!(targets(&names, "b", CloseMode::Left), vec!["a"]);
        assert_eq!(targets(&names, "b", CloseMode::Right), vec!["c"]);
        assert!(targets(&names, "a", CloseMode::Left).is_empty());
        assert!(targets(&names, "c", CloseMode::Right).is_empty());
        assert!(targets(&names, "missing", CloseMode::Others).is_empty());
        assert_eq!(targets(&names, "b", CloseMode::All), names);
    }
    #[::core::prelude::v1::test]
    fn fixed_tools_never_close_even_in_bulk_requests() {
        let names = [
            "navigation",
            "notebook",
            "settings",
            "ai-chat",
            "properties",
            "schema",
        ];
        assert_eq!(
            targets(&names, "notebook", CloseMode::All),
            vec!["notebook", "settings"]
        );
        assert!(targets(&names, "ai-chat", CloseMode::All).is_empty());
        assert!(targets(&names, "navigation", CloseMode::Current).is_empty());
        assert!(targets::<&str>(&[], "notebook", CloseMode::All).is_empty());
    }
}

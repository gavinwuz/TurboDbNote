use crate::locale::t;
use gpui::{prelude::*, *};
use gpui_component::{
    Icon, IconName, StyledExt,
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
pub struct TabEvents;
impl EventEmitter<CloseTabs> for TabEvents {}
pub struct TabRouter(pub Entity<TabEvents>);
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
pub fn editor_menu(menu: PopupMenu, name: &'static str, cx: &App) -> PopupMenu {
    menu.item(
        PopupMenuItem::new(t(cx, "tabs.close.all"))
            .icon(IconName::Delete)
            .on_click(move |_, _, cx| send(name.into(), CloseMode::All, cx)),
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
    let title = div()
        .id(SharedString::from(format!("tab-title-{name}")))
        .h_flex()
        .gap_2()
        .child(Icon::new(icon))
        .child(label.into());
    if is_fixed(&name) {
        return title.into_any_element();
    }
    title
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

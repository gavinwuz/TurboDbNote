//! Gate component menus through GPUI's clipped hit testing, not raw layout bounds.
use gpui::{prelude::*, *};
use gpui_component::menu::{ContextMenuExt, PopupMenu};
use std::{cell::Cell, rc::Rc};

pub fn scoped(
    id: impl Into<ElementId>,
    element: impl IntoElement,
    builder: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
) -> impl IntoElement {
    scoped_inner(id, element, builder, false)
}
pub fn cell(
    id: impl Into<ElementId>,
    element: impl IntoElement,
    builder: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
) -> impl IntoElement {
    scoped_inner(id, element, builder, true)
}
fn scoped_inner(
    id: impl Into<ElementId>,
    element: impl IntoElement,
    builder: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
    fill: bool,
) -> impl IntoElement {
    let hit = Rc::new(Cell::new(false));
    let trigger = hit.clone();
    div().id(id).w_full().when(fill, |el| el.h_full()).child(
        div()
            .id("menu-hit-target")
            .w_full()
            .when(fill, |el| el.h_full())
            .on_mouse_down(MouseButton::Right, move |_, _, _| trigger.set(true))
            .child(element)
            .context_menu(move |menu, window, cx| {
                if hit.replace(false) {
                    builder(menu, window, cx)
                } else {
                    menu
                }
            }),
    )
}

/// Shared column copy actions for both database navigation layouts.
pub fn column_copy(
    mut menu: PopupMenu,
    name: &str,
    kind: Option<turbodbn_services::DatabaseKind>,
    cx: &App,
) -> PopupMenu {
    use turbodbn_services::DatabaseKind;
    let quoted = match kind {
        Some(DatabaseKind::MySql | DatabaseKind::ClickHouse) => {
            format!("`{}`", name.replace('`', "``"))
        }
        _ => format!("\"{}\"", name.replace('"', "\"\"")),
    };
    for (key, value) in [
        ("grid.column.copy", name.to_owned()),
        ("grid.column.copy_quoted", quoted),
    ] {
        menu = menu.item(
            gpui_component::menu::PopupMenuItem::new(crate::locale::t(cx, key)).on_click(
                move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(value.clone()));
                    crate::feedback::success(crate::locale::t(cx, "feedback.copied"), cx);
                },
            ),
        );
    }
    menu
}

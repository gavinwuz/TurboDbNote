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

use gpui::{prelude::*, *};
use gpui_component::scroll::{Scrollbar, ScrollbarShow};
/// A bounded viewport with visible controls for both overflow axes and reserved gutters.
pub(crate) fn view(id: &'static str, handle: &ScrollHandle, child: impl IntoElement) -> AnyElement {
    div()
        .relative()
        .flex_1()
        .size_full()
        .min_h_0()
        .min_w_0()
        .overflow_hidden()
        .child(
            div()
                .id(id)
                .size_full()
                .overflow_scroll()
                .track_scroll(handle)
                .pr(px(16.))
                .pb(px(16.))
                .child(child),
        )
        .child(
            div()
                .absolute()
                .top_0()
                .bottom(px(16.))
                .right_0()
                .w(px(14.))
                .child(Scrollbar::vertical(handle).scrollbar_show(ScrollbarShow::Always)),
        )
        .child(
            div()
                .absolute()
                .left_0()
                .right(px(16.))
                .bottom_0()
                .h(px(14.))
                .child(Scrollbar::horizontal(handle).scrollbar_show(ScrollbarShow::Always)),
        )
        .into_any_element()
}

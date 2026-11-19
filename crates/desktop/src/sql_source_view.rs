use crate::{feedback, locale::t};
use gpui::{prelude::*, *};
use gpui_component::{
    Selectable, StyledExt, WindowExt,
    button::{Button, ButtonVariants},
    input::{Input, InputState},
    text::{TextView, TextViewStyle},
};

pub struct SqlSourceView {
    source: SharedString,
    search: Entity<InputState>,
    _search: Subscription,
    wrap: bool,
    rendered: Option<(String, SharedString)>,
}
impl SqlSourceView {
    pub fn new(source: SharedString, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder(t(cx, "script.full.search")));
        Self {
            source,
            _search: cx.observe(&search, |_, _, cx| cx.notify()),
            search,
            wrap: true,
            rendered: None,
        }
    }
}
impl Render for SqlSourceView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.search.read(cx).value().to_lowercase();
        if self.rendered.as_ref().is_none_or(|(old, _)| old != &query) {
            let lines = self
                .source
                .lines()
                .enumerate()
                .filter(|(_, line)| query.is_empty() || line.to_lowercase().contains(&query))
                .map(|(i, line)| format!("{:>6}  {line}", i + 1))
                .collect::<Vec<_>>()
                .join("\n");
            self.rendered = Some((
                query,
                crate::cell::sql_reading(&lines, turbodbn_services::DatabaseKind::Sqlite).into(),
            ));
        }
        let mut block = if self.wrap {
            div().whitespace_normal()
        } else {
            div().whitespace_nowrap()
        };
        let style = TextViewStyle::default().code_block(block.style().clone());
        div()
            .v_flex()
            .gap_2()
            .min_w_0()
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(Input::new(&self.search).cleanable(true).flex_1())
                    .child(
                        Button::new("source-wrap")
                            .ghost()
                            .selected(self.wrap)
                            .label(t(cx, "script.full.wrap"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.wrap = !this.wrap;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("source-copy")
                            .ghost()
                            .label(t(cx, "script.copy_source"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(
                                    this.source.to_string(),
                                ));
                                feedback::success(t(cx, "feedback.copied"), cx);
                            })),
                    ),
            )
            .child(
                div().h(px(500.)).w_full().child(
                    TextView::markdown(
                        "full-source-text",
                        self.rendered.as_ref().unwrap().1.clone(),
                        window,
                        cx,
                    )
                    .style(style)
                    .selectable(true)
                    .scrollable(true),
                ),
            )
    }
}

pub fn open(source: SharedString, window: &mut Window, cx: &mut App) {
    let view = cx.new(|cx| SqlSourceView::new(source, window, cx));
    window.open_dialog(cx, move |dialog, _, cx| {
        dialog
            .title(t(cx, "script.view_full"))
            .width(px(960.))
            .child(view.clone())
            .child(feedback::actions().child(feedback::close_button(cx)))
    });
}

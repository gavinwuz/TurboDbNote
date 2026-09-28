//! Shared transient feedback and explicit confirmation, independent of feature views.
use gpui::{prelude::*, *};
use gpui_component::{
    Disableable, Icon, IconName, StyledExt, WindowExt,
    button::{Button, ButtonVariants},
    notification::NotificationType,
};
#[derive(Clone)]
pub struct Message {
    pub kind: NotificationType,
    pub text: SharedString,
}
pub struct Events;
impl EventEmitter<Message> for Events {}
pub struct Hub(pub Entity<Events>);
impl Global for Hub {}
pub fn init(cx: &mut App) {
    let events = cx.new(|_| Events);
    cx.set_global(Hub(events));
}
pub fn notify(kind: NotificationType, text: impl Into<SharedString>, cx: &mut App) {
    let event = Message {
        kind,
        text: text.into(),
    };
    let hub = cx.global::<Hub>().0.clone();
    hub.update(cx, |_, cx| cx.emit(event));
}
pub fn success(text: impl Into<SharedString>, cx: &mut App) {
    notify(NotificationType::Success, text, cx)
}
pub fn error(text: impl Into<SharedString>, cx: &mut App) {
    notify(NotificationType::Error, text, cx)
}
pub fn actions() -> Div {
    div().h_flex().justify_end().w_full().gap_3().pt_3()
}
pub fn close_button(cx: &App) -> Button {
    Button::new("dialog-close")
        .icon(IconName::Close)
        .label(crate::locale::t(cx, "feedback.close"))
        .on_click(|_, window, cx| window.close_dialog(cx))
}
pub(crate) fn footer_actions() -> Div {
    div().h_flex().gap_3()
}

pub fn confirm(
    title: impl Into<SharedString>,
    body: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut App,
    action: impl Fn(&mut Window, &mut App) + 'static,
) {
    confirm_labels(
        title,
        body,
        crate::locale::t(cx, "feedback.close"),
        crate::locale::t(cx, "feedback.confirm"),
        window,
        cx,
        action,
    );
}

pub fn confirm_labels(
    title: impl Into<SharedString>,
    body: impl Into<SharedString>,
    cancel_label: SharedString,
    accept_label: SharedString,
    window: &mut Window,
    cx: &mut App,
    action: impl Fn(&mut Window, &mut App) + 'static,
) {
    let title = title.into();
    let body = body.into();
    let action = std::rc::Rc::new(action);
    window.open_dialog(cx, move |dialog, _, _| {
        let action = action.clone();
        let keyboard_action = action.clone();
        let cancel_label = cancel_label.clone();
        let accept_label = accept_label.clone();
        dialog
            .title(title.clone())
            .on_ok(move |_, window, cx| {
                window.close_dialog(cx);
                keyboard_action(window, cx);
                false
            })
            .w(px(520.))
            .overlay_closable(false)
            .child(
                div()
                    .w(px(470.))
                    .h_flex()
                    .items_start()
                    .gap_3()
                    .child(Icon::new(IconName::TriangleAlert))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_normal()
                            .child(body.clone()),
                    ),
            )
            .footer(move |_, _, _, cx| {
                let action = action.clone();
                vec![
                    footer_actions()
                        .child(close_button(cx).label(cancel_label.clone()))
                        .child(
                            Button::new("confirm-risk")
                                .primary()
                                .icon(IconName::Check)
                                .label(accept_label.clone())
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    action(window, cx);
                                }),
                        ),
                ]
            })
    });
}
/// SQL text and target are frozen before opening the second confirmation.
pub fn sql_preview(
    sql: String,
    target: String,
    window: &mut Window,
    cx: &mut App,
    execute: impl Fn(String, &mut Window, &mut App) -> bool + 'static,
) {
    let _ = sql_preview_observed(sql, target, window, cx, execute);
}
struct PreviewOutcome(async_channel::Sender<bool>);
impl Drop for PreviewOutcome {
    fn drop(&mut self) {
        let _ = self.0.try_send(false);
    }
}
pub fn sql_preview_observed(
    sql: String,
    target: String,
    window: &mut Window,
    cx: &mut App,
    execute: impl Fn(String, &mut Window, &mut App) -> bool + 'static,
) -> async_channel::Receiver<bool> {
    let (sender, receiver) = async_channel::bounded(1);
    let lifecycle = std::rc::Rc::new(PreviewOutcome(sender));
    use gpui_component::input::InputState;
    let input = cx.new(|cx| {
        InputState::new(window, cx)
            .multi_line(true)
            .code_editor("sql")
            .default_value(sql)
    });
    let execute = std::rc::Rc::new(execute);
    window.open_dialog(cx, move |dialog, _, cx| {
        let lifecycle = lifecycle.clone();
        let input = input.clone();
        let execute = execute.clone();
        let confirmed_target = target.clone();
        dialog
            .title(crate::locale::t(cx, "dbwork.sql.preview"))
            .on_ok(|_, _, _| false)
            .w(px(740.))
            .overlay_closable(false)
            .child(
                div()
                    .w(px(690.))
                    .v_flex()
                    .gap_3()
                    .child(target.clone())
                    .child(
                        div()
                            .w_full()
                            .h_flex()
                            .items_start()
                            .gap_2()
                            .child(Icon::new(IconName::TriangleAlert))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .whitespace_normal()
                                    .child(crate::locale::t(cx, "dbwork.sql.warning")),
                            ),
                    )
                    .child(editable_multiline(&input, false, cx).h(px(280.))),
            )
            .footer(move |_, _, _, cx| {
                let lifecycle = lifecycle.clone();
                let copy = input.clone();
                let field = input.clone();
                let execute = execute.clone();
                let target = confirmed_target.clone();
                vec![
                    footer_actions()
                        .child(close_button(cx))
                        .child(
                            Button::new("copy-preview-sql")
                                .icon(IconName::Copy)
                                .label(crate::locale::t(cx, "dbwork.copy"))
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        copy.read(cx).value().to_string(),
                                    ));
                                    success(crate::locale::t(cx, "feedback.copied"), cx);
                                }),
                        )
                        .child(
                            Button::new("execute-preview-sql")
                                .primary()
                                .icon(IconName::Check)
                                .label(crate::locale::t(cx, "dbwork.execute"))
                                .on_click(move |_, window, cx| {
                                    let sql = field.read(cx).value().to_string();
                                    if sql.trim().is_empty() {
                                        error(crate::locale::t(cx, "feedback.sql.empty"), cx);
                                        return;
                                    }
                                    let execute = execute.clone();
                                    let lifecycle = lifecycle.clone();
                                    confirm(
                                        crate::locale::t(cx, "feedback.sql.confirm"),
                                        format!(
                                            "{}\n{}",
                                            target,
                                            crate::locale::t(cx, "dbwork.sql.warning")
                                        ),
                                        window,
                                        cx,
                                        move |window, cx| {
                                            if execute(sql.clone(), window, cx) {
                                                let _ = lifecycle.0.try_send(true);
                                                window.close_dialog(cx);
                                            }
                                        },
                                    );
                                }),
                        ),
                ]
            })
    });
    receiver
}

#[derive(Clone)]
pub struct DetailRequest {
    pub inspector: bool,
    pub title: String,
    pub entries: Vec<(String, String)>,
    pub text: String,
}
impl EventEmitter<DetailRequest> for Events {}
pub fn details(event: DetailRequest, cx: &mut App) {
    let hub = cx.global::<Hub>().0.clone();
    hub.update(cx, |_, cx| cx.emit(event));
}

/// Bound notification layout while keeping the original text for the copy action.
pub fn message_preview(text: &str) -> String {
    let mut result = String::new();
    let mut run = 0;
    for ch in text.chars().take(8000) {
        if ch.is_whitespace() {
            run = 0;
        } else {
            run += 1;
        }
        result.push(ch);
        if run >= 48 {
            result.push('\u{200b}');
            run = 0;
        }
    }
    if text.chars().nth(8000).is_some() {
        result.push_str("\n…");
    }
    result
}

/// GPUI Component's built-in clear button only appears for single-line inputs.
pub fn editable_multiline(
    state: &Entity<gpui_component::input::InputState>,
    disabled: bool,
    cx: &App,
) -> gpui_component::input::Input {
    let clear = state.clone();
    gpui_component::input::Input::new(state)
        .disabled(disabled)
        .when(!state.read(cx).value().is_empty(), |input| {
            input.suffix(
                Button::new("clear-multiline-content")
                    .ghost()
                    .icon(IconName::CircleX)
                    .tooltip(crate::locale::t(cx, "notes.clear_content"))
                    .disabled(disabled)
                    .on_click(move |_, window, cx| {
                        if !disabled {
                            clear.update(cx, |input, cx| input.set_value("", window, cx));
                        }
                    }),
            )
        })
}

#[cfg(test)]
mod tests {
    #[test]
    fn preview_lifetime_reports_cancel_and_never_replaces_acceptance() {
        let (sender, receiver) = async_channel::bounded(1);
        let owner = std::rc::Rc::new(super::PreviewOutcome(sender));
        let rendered = owner.clone();
        drop(owner);
        assert!(receiver.try_recv().is_err());
        drop(rendered);
        assert_eq!(receiver.try_recv(), Ok(false));
        let (sender, receiver) = async_channel::bounded(1);
        let owner = super::PreviewOutcome(sender);
        owner.0.try_send(true).unwrap();
        drop(owner);
        assert_eq!(receiver.try_recv(), Ok(true));
    }
    #[test]
    fn long_notification_preview_is_bounded_without_mutating_copy_source() {
        let original = "a".repeat(9000);
        let preview = super::message_preview(&original);
        assert_eq!(original.len(), 9000);
        assert!(preview.chars().count() < 8200);
        assert!(preview.contains('\u{200b}') && preview.ends_with('…'));
        assert_eq!(super::message_preview("first\nsecond"), "first\nsecond");
    }
}

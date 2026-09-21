//! Shared transient feedback and explicit confirmation, independent of feature views.
use gpui::{prelude::*, *};
use gpui_component::{
    Icon, IconName, StyledExt, WindowExt,
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
    let title = title.into();
    let body = body.into();
    let action = std::rc::Rc::new(action);
    window.open_dialog(cx, move |dialog, _, _| {
        let action = action.clone();
        let keyboard_action = action.clone();
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
                    footer_actions().child(close_button(cx)).child(
                        Button::new("confirm-risk")
                            .primary()
                            .icon(IconName::Check)
                            .label(crate::locale::t(cx, "feedback.confirm"))
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
    use gpui_component::input::{Input, InputState};
    let input = cx.new(|cx| {
        InputState::new(window, cx)
            .multi_line(true)
            .code_editor("sql")
            .default_value(sql)
    });
    let execute = std::rc::Rc::new(execute);
    window.open_dialog(cx, move |dialog, _, cx| {
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
                    .child(Input::new(&input).h(px(280.))),
            )
            .footer(move |_, _, _, cx| {
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
                                                window.close_dialog(cx);
                                            }
                                        },
                                    );
                                }),
                        ),
                ]
            })
    });
}

use crate::chat::ChatView;
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Disableable, IconName, Selectable, StyledExt, Theme, ThemeMode,
    button::Button,
    dock::{Panel, PanelEvent},
};

#[derive(Clone, Copy, PartialEq)]
pub enum Category {
    General,
    Ai,
}
pub enum SettingsRequest {
    ToggleAiZoom,
    Open(Option<Category>),
    Close,
    ReturnToChat,
}
impl EventEmitter<SettingsRequest> for ChatView {}
pub struct SettingsView {
    category: Category,
    chat: Entity<ChatView>,
    _observe: Subscription,
    _inputs: Vec<Subscription>,
    focus: FocusHandle,
    close_prompt: bool,
    close_after_save: bool,
}
impl SettingsView {
    pub fn new(chat: Entity<ChatView>, cx: &mut Context<Self>) -> Self {
        let inputs = chat.read(cx).settings_inputs();
        Self {
            _inputs: inputs
                .iter()
                .map(|input| cx.observe(input, |_, _, cx| cx.notify()))
                .collect(),
            category: Category::General,
            focus: cx.focus_handle(),
            close_prompt: false,
            close_after_save: false,
            _observe: cx.observe(&chat, |this, _, cx| {
                if this.close_after_save && !this.chat.read(cx).settings_saving() {
                    this.close_after_save = false;
                    if !this.chat.read(cx).settings_dirty(cx) {
                        this.close_prompt = false;
                        this.chat
                            .update(cx, |_, cx| cx.emit(SettingsRequest::Close));
                    }
                }
                cx.notify();
            }),
            chat,
        }
    }
    pub fn select(&mut self, category: Option<Category>, cx: &mut Context<Self>) {
        if let Some(category) = category {
            self.category = category;
        }
        cx.notify();
    }
    fn close(&mut self, cx: &mut Context<Self>) {
        if self.chat.read(cx).settings_dirty(cx) {
            self.close_prompt = true;
            cx.notify();
        } else {
            self.chat
                .update(cx, |_, cx| cx.emit(SettingsRequest::Close));
        }
    }
}
impl EventEmitter<PanelEvent> for SettingsView {}
impl Focusable for SettingsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for SettingsView {
    fn panel_name(&self) -> &'static str {
        "settings"
    }
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.chat.read(cx).settings_dirty(cx) {
            "设置 •"
        } else {
            "设置"
        }
    }
    fn closable(&self, _: &App) -> bool {
        false
    }
    fn title_suffix(&mut self, _: &mut Window, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        Some(
            Button::new("close-settings-tab")
                .icon(IconName::Close)
                .tooltip("关闭设置")
                .disabled(self.chat.read(cx).settings_saving())
                .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
        )
    }
}
impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ai = self.category == Category::Ai;
        let dark = cx.theme().mode.is_dark();
        let body =
            if ai {
                self.chat
                    .update(cx, |chat, cx| chat.render_ai_settings(window, cx))
                    .into_any_element()
            } else {
                let chat = self.chat.clone();
                div()
                    .v_flex()
                    .gap_4()
                    .child(div().text_lg().child("通用"))
                    .child("外观主题")
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(
                                Button::new("theme-light")
                                    .icon(IconName::Sun)
                                    .label("白天 Light")
                                    .selected(!dark)
                                    .on_click(|_, window, cx| {
                                        Theme::change(ThemeMode::Light, Some(window), cx)
                                    }),
                            )
                            .child(
                                Button::new("theme-dark")
                                    .icon(IconName::Moon)
                                    .label("黑夜 Dark")
                                    .selected(dark)
                                    .on_click(|_, window, cx| {
                                        Theme::change(ThemeMode::Dark, Some(window), cx)
                                    }),
                            ),
                    )
                    .child(Button::new("save-general").label("保存设置").on_click(
                        move |_, _, cx| chat.update(cx, |chat, cx| chat.save_settings(false, cx)),
                    ))
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.chat.read(cx).settings_status()),
                    )
                    .into_any_element()
            };
        let saving = self.chat.read(cx).settings_saving();
        div()
            .v_flex()
            .size_full()
            .min_h_0()
            .p_3()
            .gap_3()
            .track_focus(&self.focus)
            .when(self.close_prompt, |el| {
                el.child(
                    div()
                        .v_flex()
                        .gap_2()
                        .p_3()
                        .bg(cx.theme().muted)
                        .child("设置尚未保存")
                        .child(
                            div()
                                .h_flex()
                                .gap_2()
                                .flex_wrap()
                                .child(
                                    Button::new("save-close")
                                        .label("保存并关闭")
                                        .disabled(saving || self.chat.read(cx).settings_busy())
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.close_after_save = true;
                                            this.chat
                                                .update(cx, |chat, cx| chat.save_all_settings(cx));
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    Button::new("discard-close")
                                        .label("放弃")
                                        .disabled(saving)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.chat.update(cx, |chat, cx| {
                                                chat.discard_settings(window, cx)
                                            });
                                            this.close_prompt = false;
                                            this.chat.update(cx, |_, cx| {
                                                cx.emit(SettingsRequest::Close)
                                            });
                                        })),
                                )
                                .child(
                                    Button::new("continue-settings")
                                        .label("继续编辑")
                                        .disabled(saving)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.close_prompt = false;
                                            cx.notify();
                                        })),
                                ),
                        )
                        .child(self.chat.read(cx).settings_status()),
                )
            })
            .child(
                div()
                    .h_flex()
                    .flex_1()
                    .min_h_0()
                    .gap_5()
                    .child(
                        div()
                            .v_flex()
                            .w(px(130.))
                            .h_full()
                            .gap_2()
                            .pr_3()
                            .border_r_1()
                            .border_color(cx.theme().border)
                            .child(
                                Button::new("general")
                                    .icon(IconName::Settings)
                                    .label("通用")
                                    .selected(!ai)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.category = Category::General;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("ai-settings")
                                    .icon(IconName::Bot)
                                    .label("AI")
                                    .selected(ai)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.category = Category::Ai;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .id("settings-content")
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .overflow_y_scroll()
                            .child(body),
                    ),
            )
            .child(
                Button::new("return-chat")
                    .icon(IconName::ArrowRight)
                    .label("返回聊天")
                    .on_click({
                        let chat = self.chat.clone();
                        move |_, _, cx| {
                            chat.update(cx, |_, cx| cx.emit(SettingsRequest::ReturnToChat))
                        }
                    }),
            )
    }
}

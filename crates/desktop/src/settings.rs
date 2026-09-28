use crate::chat::ChatView;
use crate::{configuration::Configuration, locale::t};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Disableable, IconName, Selectable, StyledExt, WindowExt,
    button::Button,
    dock::{Panel, PanelEvent},
};

#[derive(Clone, Copy, PartialEq)]
pub enum Category {
    General,
    Shortcuts,
    Database,
    Ai,
    Providers,
    Extensions,
}
pub enum SettingsRequest {
    ToggleAiZoom,
    Open(Option<Category>),
    Close,
    CancelClose,
    ReturnToChat,
}
impl EventEmitter<SettingsRequest> for ChatView {}
pub struct SettingsView {
    drivers: Entity<crate::drivers::DriverManager>,
    category: Category,
    database_tab: usize,
    configuration: Entity<Configuration>,
    _configuration_observe: Subscription,
    chat: Entity<ChatView>,
    _observe: Subscription,
    _inputs: Vec<Subscription>,
    focus: FocusHandle,
    close_prompt: bool,
    close_after_save: bool,
    autosave: Option<Task<()>>,
    autosave_signature: String,
    _theme_observe: Subscription,
}
impl SettingsView {
    pub fn new(
        chat: Entity<ChatView>,
        drivers: Entity<crate::drivers::DriverManager>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let inputs = chat.read(cx).settings_inputs();
        let configuration = cx.new(|cx| Configuration::new(window, cx));
        Self {
            drivers,
            _configuration_observe: cx.observe(&configuration, |this, _, cx| {
                this.schedule_save(cx);
                cx.notify();
            }),
            _theme_observe: cx.observe_global::<gpui_component::Theme>(|this, cx| {
                this.schedule_save(cx);
                cx.notify();
            }),
            autosave: None,
            autosave_signature: String::new(),
            configuration,
            _inputs: inputs
                .iter()
                .map(|input| {
                    cx.observe(input, |this, _, cx| {
                        this.schedule_save(cx);
                        cx.notify();
                    })
                })
                .collect(),
            category: Category::General,
            database_tab: 0,
            focus: cx.focus_handle(),
            close_prompt: false,
            close_after_save: false,
            _observe: cx.observe(&chat, |this, _, cx| {
                if this.close_after_save && !this.chat.read(cx).settings_saving() {
                    this.close_after_save = false;
                    if !this.chat.read(cx).settings_dirty(cx)
                        && !this.configuration.read(cx).dirty(cx)
                    {
                        this.close_prompt = false;
                        this.chat
                            .update(cx, |_, cx| cx.emit(SettingsRequest::Close));
                    } else {
                        this.close_prompt = true;
                    }
                }
                this.schedule_save(cx);
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
    pub fn reset_theme(&mut self, cx: &mut Context<Self>) {
        self.configuration
            .update(cx, |form, cx| form.reset_theme(cx));
    }
    pub fn pending(&self, cx: &App) -> bool {
        self.chat.read(cx).settings_saving()
            || self.chat.read(cx).settings_dirty(cx)
            || self.configuration.read(cx).dirty(cx)
    }
    fn schedule_save(&mut self, cx: &mut Context<Self>) {
        if self.chat.read(cx).settings_saving() {
            return;
        }
        if !self.configuration.read(cx).dirty(cx) && !self.chat.read(cx).settings_dirty(cx) {
            self.autosave_signature.clear();
            return;
        }
        let signature = format!(
            "{}:{}",
            self.configuration.read(cx).signature(cx),
            self.chat.read(cx).settings_signature(cx)
        );
        if self.autosave_signature == signature {
            return;
        }
        self.autosave_signature = signature;
        self.autosave = Some(cx.spawn(async move |view, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(600))
                .await;
            let _ = view.update(cx, |this, cx| {
                this.flush(false, cx);
            });
        }));
    }
    fn flush(&mut self, closing: bool, cx: &mut Context<Self>) {
        if self.chat.read(cx).settings_saving() {
            self.close_after_save |= closing;
            return;
        }
        if self.configuration.read(cx).dirty(cx)
            && !self.configuration.update(cx, |form, cx| form.save(cx))
        {
            self.close_prompt |= closing;
            cx.notify();
            return;
        }
        if self.chat.read(cx).settings_dirty(cx) {
            self.close_after_save |= closing;
            self.chat.update(cx, |chat, cx| chat.save_all_settings(cx));
            if closing && !self.chat.read(cx).settings_saving() {
                self.close_after_save = false;
                self.close_prompt = true;
            }
        } else if closing {
            self.close_prompt = false;
            self.chat
                .update(cx, |_, cx| cx.emit(SettingsRequest::Close));
        }
        cx.notify();
    }
    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.autosave = None;
        self.flush(true, cx);
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
        let title =
            if self.chat.read(cx).settings_dirty(cx) || self.configuration.read(cx).dirty(cx) {
                format!("{} •", t(cx, "settings.title"))
            } else {
                t(cx, "settings.title").to_string()
            };
        crate::tabs::title("settings", IconName::Settings, title, cx)
    }
    fn dropdown_menu(
        &mut self,
        menu: gpui_component::menu::PopupMenu,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_component::menu::PopupMenu {
        crate::tabs::editor_menu(menu, "settings", cx)
    }
    fn closable(&self, _: &App) -> bool {
        false
    }
}
impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ai = self.category == Category::Ai;
        let locked = self.chat.read(cx).settings_saving();
        self.configuration.update(cx, |form, _| {
            form.category = self.category;
            form.locked = locked;
            form.database_tab = self.database_tab;
        });
        let agent_settings = ai || self.category == Category::Extensions;
        let active_engine = self.chat.read(cx).settings_engine();
        let can_switch = self.chat.read(cx).settings_can_switch(cx);
        let body = if self.category == Category::Database {
            let tabs = div().h_flex().gap_2().children(
                ["settings2.query", "settings2.sql", "settings2.drivers"]
                    .into_iter()
                    .enumerate()
                    .map(|(index, label)| {
                        Button::new(("database-tab", index))
                            .label(t(cx, label))
                            .selected(self.database_tab == index)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.database_tab = index;
                                cx.notify();
                            }))
                    }),
            );
            let content = if self.database_tab == 2 {
                let mut list = div().v_flex().gap_2();
                for kind in turbodbn_services::DatabaseKind::ALL {
                    list = list.child(
                        div()
                            .h_flex()
                            .gap_3()
                            .p_3()
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .child(kind.label())
                            .child(div().flex_1())
                            .child(t(cx, "settings2.builtin")),
                    );
                }
                list.child(
                    Button::new("legacy-driver-settings")
                        .icon(IconName::Settings2)
                        .label(t(cx, "settings2.legacy_driver"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            let drivers = this.drivers.clone();
                            window.open_dialog(cx, move |dialog, _, cx| {
                                dialog
                                    .title(t(cx, "settings2.legacy_driver"))
                                    .w(px(680.))
                                    .child(drivers.clone())
                                    .footer(|_, _, _, cx| {
                                        vec![
                                            crate::feedback::footer_actions()
                                                .child(crate::feedback::close_button(cx)),
                                        ]
                                    })
                            });
                        })),
                )
                .into_any_element()
            } else {
                self.configuration.clone().into_any_element()
            };
            div()
                .v_flex()
                .gap_4()
                .child(tabs)
                .child(content)
                .into_any_element()
        } else if agent_settings {
            let mut list = div().v_flex().gap_3();
            for engine in [
                turbodbn_services::agent::Engine::Codex,
                turbodbn_services::agent::Engine::Claude,
            ] {
                let status = if active_engine == engine {
                    self.chat.read(cx).connection_status()
                } else {
                    "settings2.inactive"
                };
                list = list.child(
                    div()
                        .h_flex()
                        .gap_3()
                        .p_3()
                        .border_1()
                        .border_color(cx.theme().border)
                        .rounded_md()
                        .child(IconName::Bot)
                        .child(div().flex_1().child(engine.name()))
                        .child(t(cx, status))
                        .child(
                            Button::new(engine.name())
                                .icon(IconName::Settings2)
                                .label(t(cx, "settings2.configure"))
                                .disabled(active_engine != engine && !can_switch)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.chat.update(cx, |chat, cx| {
                                        chat.select_settings_engine(engine, window, cx)
                                    });
                                    let chat = this.chat.clone();
                                    window.open_dialog(cx, move |dialog, window, cx| {
                                        let body = chat.update(cx, |chat, cx| {
                                            chat.render_ai_settings(window, cx)
                                        });
                                        dialog
                                            .title(engine.name())
                                            .w(px(700.))
                                            .overlay_closable(false)
                                            .child(
                                                div()
                                                    .id("agent-config-scroll")
                                                    .max_h(px(480.))
                                                    .overflow_y_scroll()
                                                    .child(body),
                                            )
                                            .footer(|_, _, _, cx| {
                                                vec![
                                                    crate::feedback::footer_actions()
                                                        .child(crate::feedback::close_button(cx)),
                                                ]
                                            })
                                    });
                                    cx.notify();
                                })),
                        ),
                );
            }
            self.configuration
                .update(cx, |form, _| form.category = Category::Extensions);
            list.child(self.configuration.clone()).into_any_element()
        } else {
            self.configuration.clone().into_any_element()
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
                        .child(t(cx, "settings.autosave.error"))
                        .child(
                            div()
                                .h_flex()
                                .gap_2()
                                .flex_wrap()
                                .child(
                                    Button::new("save-close")
                                        .icon(IconName::Check)
                                        .label(t(cx, "common.action.retry"))
                                        .disabled(saving || self.chat.read(cx).settings_busy())
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.flush(true, cx);
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    Button::new("discard-close")
                                        .icon(IconName::Delete)
                                        .label(t(cx, "common.action.discard"))
                                        .disabled(saving)
                                        .on_click(cx.listener(|_, _, window, cx| {
                                            let view = cx.entity().downgrade();
                                            crate::feedback::confirm(
                                                t(cx, "feedback.settings.discard"),
                                                t(cx, "feedback.settings.discard.body"),
                                                window,
                                                cx,
                                                move |window, cx| {
                                                    let _ = view.update(cx, |this, cx| {
                                                        if this.chat.read(cx).settings_saving() {
                                                            return;
                                                        }
                                                        this.configuration
                                                            .update(cx, |form, cx| {
                                                                form.discard(window, cx)
                                                            });
                                                        this.chat.update(cx, |chat, cx| {
                                                            chat.discard_settings(window, cx)
                                                        });
                                                        this.close_prompt = false;
                                                        this.chat.update(cx, |_, cx| {
                                                            cx.emit(SettingsRequest::Close)
                                                        });
                                                    });
                                                },
                                            );
                                        })),
                                )
                                .child(
                                    Button::new("continue-settings")
                                        .icon(IconName::ArrowLeft)
                                        .label(t(cx, "common.action.continue"))
                                        .disabled(saving)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.close_prompt = false;
                                            this.chat.update(cx, |_, cx| {
                                                cx.emit(SettingsRequest::CancelClose)
                                            });
                                            cx.notify();
                                        })),
                                ),
                        )
                        .child(crate::locale::message(
                            cx,
                            &self.chat.read(cx).settings_status(),
                        ))
                        .child(crate::locale::message(
                            cx,
                            &self.configuration.read(cx).status,
                        )),
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
                            .w(rems(10.25))
                            .flex_shrink_0()
                            .h_full()
                            .gap_2()
                            .pr_3()
                            .border_r_1()
                            .border_color(cx.theme().border)
                            .child(
                                Button::new("general")
                                    .icon(IconName::Settings)
                                    .label(t(cx, "settings.general.title"))
                                    .selected(self.category == Category::General)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.category = Category::General;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("shortcut-settings")
                                    .icon(IconName::Settings2)
                                    .label(t(cx, "settings2.shortcuts"))
                                    .selected(self.category == Category::Shortcuts)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.category = Category::Shortcuts;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("database-settings")
                                    .icon(IconName::GalleryVerticalEnd)
                                    .label(t(cx, "settings.database.title"))
                                    .selected(self.category == Category::Database)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.category = Category::Database;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("ai-settings")
                                    .icon(IconName::Bot)
                                    .label(t(cx, "settings.agents.title"))
                                    .selected(ai || self.category == Category::Extensions)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.category = Category::Ai;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("providers-settings")
                                    .icon(IconName::Globe)
                                    .label(t(cx, "settings.providers.title"))
                                    .selected(self.category == Category::Providers)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.category = Category::Providers;
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
                div()
                    .text_size(rems(crate::typography::BODY))
                    .text_color(cx.theme().muted_foreground)
                    .child(if self.chat.read(cx).settings_saving() {
                        t(cx, "settings.autosave.saving")
                    } else if self.configuration.read(cx).dirty(cx)
                        || self.chat.read(cx).settings_dirty(cx)
                    {
                        t(cx, "settings.autosave.pending")
                    } else {
                        t(cx, "settings.autosave.saved")
                    })
                    .when(self.pending(cx), |el| {
                        el.child(crate::locale::message(
                            cx,
                            &self.chat.read(cx).settings_status(),
                        ))
                    })
                    .when(
                        !self.configuration.read(cx).status.is_empty()
                            && self.configuration.read(cx).status != "common.status.saved",
                        |el| {
                            el.child(crate::locale::message(
                                cx,
                                &self.configuration.read(cx).status,
                            ))
                        },
                    ),
            )
            .child(
                Button::new("return-chat")
                    .icon(IconName::ArrowRight)
                    .label(t(cx, "chat.action.return"))
                    .on_click({
                        let chat = self.chat.clone();
                        move |_, _, cx| {
                            chat.update(cx, |_, cx| cx.emit(SettingsRequest::ReturnToChat))
                        }
                    }),
            )
    }
}

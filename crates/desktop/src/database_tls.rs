use super::*;
use gpui_component::WindowExt;
use mysql::TlsIssue;
use std::{cell::Cell, rc::Rc};

#[derive(Clone)]
pub(super) enum TlsPrompt {
    Failure {
        profile: Profile,
        mode: TlsMode,
        issue: TlsIssue,
    },
    Success {
        profile: Profile,
        mode: TlsMode,
        remember: bool,
    },
}
pub(super) fn mode_label(mode: TlsMode) -> &'static str {
    match mode {
        TlsMode::VerifyIdentity => "加密并验证身份",
        TlsMode::Required => "仅加密（不验证身份）",
        TlsMode::Disabled => "不加密",
    }
}
pub(super) fn is_local_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}
impl DatabaseView {
    fn tls_snapshot_matches(&self, profile: &Profile, cx: &App) -> bool {
        self.draft(cx).ok().as_ref() == Some(profile)
    }
    pub(super) fn show_tls_prompt(
        &mut self,
        prompt: TlsPrompt,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let profile = match &prompt {
            TlsPrompt::Failure { profile, .. } | TlsPrompt::Success { profile, .. } => profile,
        };
        if !self.tls_snapshot_matches(profile, cx) {
            return;
        }
        match prompt {
            TlsPrompt::Failure {
                profile,
                mode,
                issue,
            } => {
                let view = cx.entity().downgrade();
                let remember = Rc::new(Cell::new(false));
                window.open_dialog(cx,move |dialog,_,cx| {
                    let configure=view.clone();let config_profile=profile.clone();let checkbox=remember.clone();
                    let mut actions=div().h_flex().gap_2().flex_wrap()
                        .child(Button::new("tls-configure").primary().icon(IconName::Settings).label("配置 TLS / 证书")
                            .on_click(move |_,window,cx|{
                                window.close_dialog(cx);
                                let _=configure.update(cx,|this,cx|{if this.tls_snapshot_matches(&config_profile,cx){this.tab=1;cx.notify();}});
                            }));
                    if issue==TlsIssue::Certificate && mode==TlsMode::VerifyIdentity {
                        actions=actions.child(retry_button("tls-encrypted-test","仅加密测试",TlsMode::Required,view.clone(),profile.clone(),remember.clone()));
                    }
                    if issue!=TlsIssue::Required {
                        actions=actions.child(retry_button("tls-plaintext-test","不加密测试",TlsMode::Disabled,view.clone(),profile.clone(),remember.clone()));
                    }
                    actions=actions.child(Button::new("tls-cancel").label("取消").on_click(|_,window,cx|window.close_dialog(cx)));
                    dialog.title("连接加密设置需要处理").w(px(560.)).overlay_closable(false)
                        .child(div().v_flex().gap_3()
                            .child(format!("{} · {}:{}",profile.name,profile.host,profile.port))
                            .child(format!("当前尝试：{}",mode_label(mode)))
                            .child(issue.message())
                            .child("仅加密保留传输加密，但不验证服务器身份；不加密测试将建立未加密连接。")
                            .child(Switch::new("remember-tls-mode").label("测试成功后保存当前连接及此模式")
                                .checked(remember.get()).on_click(move |checked,_,cx|{checkbox.set(*checked);cx.refresh_windows();}))
                            .child(div().text_sm().text_color(cx.theme().muted_foreground).child("未勾选时只用于本次测试，已保存配置保持不变。"))
                            .child(actions))
                });
            }
            TlsPrompt::Success {
                mut profile,
                mode,
                remember,
            } => {
                if remember {
                    self.editing.tls = mode;
                    profile.tls = mode;
                    self.save(window, cx);
                }
                let view = cx.entity().downgrade();
                window.open_dialog(cx, move |dialog, _, cx| {
                    let connect = view.clone();
                    let snapshot = profile.clone();
                    let (saving, saved, status) = view
                        .upgrade()
                        .map(|view| {
                            let this = view.read(cx);
                            (
                                this.saving,
                                this.profiles.iter().any(|p| p == &profile),
                                this.status.clone(),
                            )
                        })
                        .unwrap_or((true, false, String::new()));
                    dialog
                        .title("连接测试成功")
                        .w(px(520.))
                        .overlay_closable(false)
                        .child(
                            div()
                                .v_flex()
                                .gap_3()
                                .child(format!("{} · {}", profile.name, mode_label(mode)))
                                .child(if !remember {
                                    "本次测试未修改已保存的 TLS 设置。"
                                } else if saving {
                                    "正在保存连接…"
                                } else if saved {
                                    "连接及 TLS 模式已保存。"
                                } else {
                                    "保存未完成，请查看下方原因。"
                                })
                                .when(remember && !saving && !saved, |el| el.child(status))
                                .child(
                                    div()
                                        .h_flex()
                                        .gap_2()
                                        .child(
                                            Button::new("tls-connect-confirmed")
                                                .primary()
                                                .icon(IconName::ArrowRight)
                                                .label("使用此模式连接")
                                                .disabled(saving)
                                                .on_click(move |_, window, cx| {
                                                    window.close_dialog(cx);
                                                    let _ = connect.update(cx, |this, cx| {
                                                        if this.tls_snapshot_matches(&snapshot, cx)
                                                        {
                                                            this.start_attempt(
                                                                false,
                                                                false,
                                                                Some((mode, false)),
                                                                cx,
                                                            );
                                                        }
                                                    });
                                                }),
                                        )
                                        .child(
                                            Button::new("tls-done")
                                                .label("完成")
                                                .on_click(|_, window, cx| window.close_dialog(cx)),
                                        ),
                                ),
                        )
                });
            }
        }
    }
}
fn retry_button(
    id: &'static str,
    label: &'static str,
    mode: TlsMode,
    view: WeakEntity<DatabaseView>,
    profile: Profile,
    remember: Rc<Cell<bool>>,
) -> Button {
    Button::new(id).label(label).on_click(move |_, window, cx| {
        window.close_dialog(cx);
        let _ = view.update(cx, |this, cx| {
            if this.tls_snapshot_matches(&profile, cx) {
                this.start_attempt(true, false, Some((mode, remember.get())), cx);
            }
        });
    })
}

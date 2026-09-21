use super::*;
use gpui_component::WindowExt;
use mysql::TlsIssue;

#[derive(Clone)]
pub(super) enum TlsPrompt {
    Failure {
        profile: Profile,
        mode: TlsMode,
        issue: TlsIssue,
        discover: bool,
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
        self.raw_draft(cx).ok().as_ref() == Some(profile)
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
        if !self.tls_snapshot_matches(profile, cx) || self.saving {
            return;
        }
        match prompt {
            TlsPrompt::Failure {
                profile,
                mode,
                issue,
                discover,
            } => {
                if mode == TlsMode::Disabled {
                    return;
                }
                let view = cx.entity().downgrade();
                window.open_dialog(cx, move |dialog, _, cx| {
                    let view = view.clone();
                    let snapshot = profile.clone();
                    dialog
                        .title(crate::locale::t(cx, "db.tls.retry.title"))
                        .w(px(520.))
                        .overlay_closable(false)
                        .child(
                            div().w(px(470.)).v_flex().gap_3().child(
                                div()
                                    .h_flex()
                                    .gap_2()
                                    .child(gpui_component::Icon::new(IconName::TriangleAlert))
                                    .child(div().flex_1().min_w_0().whitespace_normal().child(
                                        crate::locale::t(
                                            cx,
                                            if issue == TlsIssue::Required {
                                                "db.tls.required"
                                            } else {
                                                "db.tls.retry.body"
                                            },
                                        ),
                                    )),
                            ),
                        )
                        .footer(move |_, _, _, cx| {
                            let view = view.clone();
                            let snapshot = snapshot.clone();
                            vec![
                                feedback::footer_actions()
                                    .child(feedback::close_button(cx))
                                    .child(
                                        Button::new("tls-disable-retry")
                                            .primary()
                                            .icon(IconName::Redo)
                                            .label(crate::locale::t(cx, "db.tls.retry.action"))
                                            .disabled(issue == TlsIssue::Required)
                                            .on_click(move |_, window, cx| {
                                                window.close_dialog(cx);
                                                let _ = view.update(cx, |this, cx| {
                                                    if this.tls_snapshot_matches(&snapshot, cx)
                                                        && !this.saving
                                                    {
                                                        this.start_attempt(
                                                            true,
                                                            discover,
                                                            Some((TlsMode::Disabled, true)),
                                                            cx,
                                                        );
                                                    }
                                                });
                                            }),
                                    )
                                    .into_any_element(),
                            ]
                        })
                });
            }
            TlsPrompt::Success {
                profile,
                mode,
                remember,
            } => {
                if !remember {
                    return;
                }
                self.editing.tls = mode;
                if let Some(source) = self.catalog_source.get_mut(&profile.id) {
                    source.tls = mode;
                }
                // Save only TLS, and only for the exact saved configuration that was tested.
                // Incomplete or modified drafts stay drafts until the user explicitly saves.
                if self.profiles.iter().find(|p| p.id == profile.id) != Some(&profile) {
                    feedback::success(crate::locale::t(cx, "db.tls.retry.draft"), cx);
                    cx.notify();
                    return;
                }
                let mut updated = profile.clone();
                updated.tls = mode;
                let payload = updated.clone();
                let credential = self.history.get(&profile.id).and_then(|h| h.credential_id);
                let path = self.file.clone();
                self.saving = true;
                let work = cx
                    .background_executor()
                    .spawn(async move { connection_store::save(&path, &payload, credential) });
                self.save_task = Some(cx.spawn_in(window, async move |view, cx| {
                    let result = work.await;
                    let _ = view.update(cx, |this, cx| {
                        this.saving = false;
                        match result {
                            Ok(mut history) => {
                                if let Some(previous) = this.history.get(&profile.id)
                                    && previous.last_used_at > history.last_used_at
                                {
                                    history.last_used_at = previous.last_used_at.clone();
                                }
                                this.history.insert(profile.id, history);
                                if let Some(saved) =
                                    this.profiles.iter_mut().find(|p| p.id == profile.id)
                                {
                                    *saved = updated;
                                }
                                if this.baseline == profile {
                                    this.baseline.tls = mode;
                                }
                                feedback::success(crate::locale::t(cx, "db.tls.retry.saved"), cx);
                            }
                            Err(error) => feedback::error(error, cx),
                        }
                        cx.notify();
                    });
                }));
                cx.notify();
            }
        }
    }
}

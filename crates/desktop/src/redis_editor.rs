use super::*;
use turbodbn_services::database::redis_value::Mutation;

impl TableView {
    pub(super) fn redis_ttl_label(&self, cx: &App) -> SharedString {
        if self.redis_ttl == -1 {
            return t(cx, "redis.ttl_persistent");
        }
        if self.redis_ttl < 0 {
            return t(cx, "redis.ttl_missing");
        }
        let seconds = self
            .redis_ttl
            .saturating_sub(self.redis_read_at.elapsed().as_secs() as i64)
            .max(0);
        crate::locale::tr(
            cx,
            "redis.ttl_remaining",
            &[(
                "time",
                &format!(
                    "{:02}:{:02}:{:02}",
                    seconds / 3600,
                    seconds % 3600 / 60,
                    seconds % 60
                ),
            )],
        )
    }
    pub(super) fn redis_actions(&self, cx: &Context<Self>) -> impl IntoElement {
        div()
            .h_flex()
            .gap_1()
            .child(
                Button::new("redis-create")
                    .ghost()
                    .icon(IconName::Plus)
                    .tooltip(t(cx, "redis.create_key"))
                    .disabled(self.busy)
                    .on_click(
                        cx.listener(|this, _, window, cx| this.redis_edit(true, false, window, cx)),
                    ),
            )
            .child(
                Button::new("redis-add")
                    .ghost()
                    .icon(IconName::Plus)
                    .tooltip(t(cx, "redis.add_member"))
                    .disabled(
                        self.busy
                            || !["hash", "set", "zset", "list"].contains(&self.redis_kind.as_str()),
                    )
                    .on_click(
                        cx.listener(|this, _, window, cx| this.redis_edit(false, true, window, cx)),
                    ),
            )
            .child(
                Button::new("redis-edit")
                    .ghost()
                    .icon(IconName::Settings2)
                    .tooltip(t(cx, "redis.edit_value"))
                    .disabled(
                        self.busy
                            || self.redis_truncated
                            || self.rows.values.is_empty()
                            || (self.redis_kind != "string" && self.selected.is_none()),
                    )
                    .on_click(
                        cx.listener(|this, _, window, cx| {
                            this.redis_edit(false, false, window, cx)
                        }),
                    ),
            )
            .child(
                Button::new("redis-delete")
                    .ghost()
                    .icon(gpui_component::Icon::new(IconName::Delete).text_color(cx.theme().danger))
                    .tooltip(t(cx, "redis.delete_key"))
                    .disabled(self.busy || self.redis_kind == "none")
                    .on_click(cx.listener(|this, _, window, cx| {
                        let Some(key) = decode_key(&this.target.object.parent) else {
                            return;
                        };
                        this.confirm_redis(
                            Mutation {
                                key,
                                kind: this.redis_kind.clone(),
                                member: String::new(),
                                expected: None,
                                value: String::new(),
                                delete: true,
                            },
                            window,
                            cx,
                        );
                    })),
            )
    }
    fn redis_edit(
        &mut self,
        new_key: bool,
        append: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let row = self
            .rows
            .values
            .get(self.selected.map(|s| s.0).unwrap_or(0));
        let member = if new_key || append || self.redis_kind == "string" {
            String::new()
        } else {
            row.and_then(|r| r.first())
                .and_then(Clone::clone)
                .unwrap_or_default()
        };
        let old = if new_key || append {
            None
        } else {
            row.and_then(|r| r.last()).and_then(Clone::clone)
        };
        if old.as_ref().is_some_and(|v| v.starts_with("hex:"))
            || member.starts_with("hex:")
            || (!new_key && !append && self.redis_truncated)
        {
            crate::feedback::error(t(cx, "redis.binary_readonly"), cx);
            return;
        }
        let key_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(member.clone())
                .placeholder(t(
                    cx,
                    if new_key {
                        "redis.key_name"
                    } else {
                        "redis.member_name"
                    },
                ))
        });
        let value_input = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .default_value(old.clone().unwrap_or_default())
        });
        let owner = cx.entity().downgrade();
        let kind = if new_key {
            "none".into()
        } else {
            self.redis_kind.clone()
        };
        let key = if new_key {
            vec![]
        } else {
            decode_key(&self.target.object.parent).unwrap_or_default()
        };
        window.open_dialog(cx, move |dialog, _, cx| {
            let owner = owner.clone();
            let key_input = key_input.clone();
            let value_input = value_input.clone();
            let kind = kind.clone();
            let key = key.clone();
            let old = old.clone();
            let member = member.clone();
            dialog
                .overlay_closable(false)
                .on_ok(|_, _, _| false)
                .title(t(
                    cx,
                    if new_key {
                        "redis.create_key"
                    } else {
                        "redis.edit_value"
                    },
                ))
                .w(px(560.))
                .when(
                    new_key || (append && ["hash", "zset"].contains(&kind.as_str())),
                    |el| el.child(Input::new(&key_input).cleanable(true)),
                )
                .child(crate::feedback::editable_multiline(&value_input, false, cx).h(px(160.)))
                .footer(move |_, _, _, cx| {
                    let owner = owner.clone();
                    let key_input = key_input.clone();
                    let value_input = value_input.clone();
                    let kind = kind.clone();
                    let key = key.clone();
                    let old = old.clone();
                    let member = member.clone();
                    vec![
                        crate::feedback::actions()
                            .child(crate::feedback::close_button(cx))
                            .child(
                                Button::new("redis-review")
                                    .primary()
                                    .label(t(cx, "feedback.confirm"))
                                    .on_click(move |_, window, cx| {
                                        let entered = key_input.read(cx).value().to_string();
                                        let change = Mutation {
                                            key: if new_key {
                                                entered.clone().into_bytes()
                                            } else {
                                                key.clone()
                                            },
                                            kind: kind.clone(),
                                            member: if append { entered } else { member.clone() },
                                            expected: old.clone(),
                                            value: value_input.read(cx).value().to_string(),
                                            delete: false,
                                        };
                                        if let Err(error) = change.validate() {
                                            crate::feedback::error(
                                                crate::locale::message(cx, &error),
                                                cx,
                                            );
                                            return;
                                        }
                                        window.close_dialog(cx);
                                        let _ = owner.update(cx, |this, cx| {
                                            this.confirm_redis(change, window, cx)
                                        });
                                    }),
                            ),
                    ]
                })
        });
    }
    fn confirm_redis(&self, change: Mutation, window: &mut Window, cx: &mut Context<Self>) {
        let owner = cx.entity().downgrade();
        let title = t(
            cx,
            if change.delete {
                "redis.delete_key"
            } else {
                "redis.edit_value"
            },
        );
        let body = crate::locale::tr(
            cx,
            if change.delete {
                "redis.delete_confirm"
            } else {
                "redis.write_confirm"
            },
            &[
                ("database", &self.target.database),
                (
                    "key",
                    &turbodbn_services::database::redis_value::display(&change.key),
                ),
            ],
        );
        crate::feedback::confirm(title, body, window, cx, move |_, cx| {
            let _ = owner.update(cx, |this, cx| {
                if !this.busy {
                    this.offset = 0;
                    this.redis_cursor = "0".into();
                    this.request(Action::RedisWrite(change.clone()), cx);
                }
            });
        });
    }
}

fn decode_key(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    hex.as_bytes()
        .chunks(2)
        .map(|c| {
            std::str::from_utf8(c)
                .ok()
                .and_then(|v| u8::from_str_radix(v, 16).ok())
        })
        .collect()
}

use super::*;
use crate::{database::DatabaseView, locale::tr};
fn detail_key(object: &workbench::Object) -> String {
    serde_json::to_string(&(&object.schema, &object.name)).unwrap()
}
fn columns(width: f32) -> usize {
    ((width.max(0.) + 8.) / 280.).floor().clamp(1., 8.) as usize
}
impl TableView {
    fn load_visible_details(&mut self, range: std::ops::Range<usize>, cx: &mut Context<Self>) {
        self.cards_visible = range.clone();
        if self.cards_detail_task.is_some() {
            return;
        }
        let objects = self
            .cards
            .get(range)
            .unwrap_or_default()
            .iter()
            .filter(|o| {
                let key = detail_key(o);
                !self.cards_details.contains_key(&key) && !self.cards_details_failed.contains(&key)
            })
            .take(12)
            .cloned()
            .collect::<Vec<_>>();
        if objects.is_empty() {
            return;
        }
        let source = self.source.clone();
        let target = self.target.clone();
        let epoch = self.cards_detail_epoch;
        let requested = objects.clone();
        self.cards_detail_task = Some(cx.spawn(async move |owner, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(250))
                .await;
            let visible = owner
                .update(cx, |this, cx| {
                    let visible = this
                        .cards
                        .get(this.cards_visible.clone())
                        .unwrap_or_default();
                    let still_visible = requested
                        .iter()
                        .any(|object| visible.iter().any(|v| detail_key(v) == detail_key(object)));
                    if !still_visible {
                        this.cards_detail_task = None;
                        cx.notify();
                    }
                    still_visible
                })
                .unwrap_or(false);
            if !visible {
                return;
            }
            let result = match source.update(cx, |source, cx| {
                source.mcp_request(
                    target.connection,
                    target.generation,
                    target.database,
                    Action::TableCardDetails(objects),
                    cx,
                )
            }) {
                Ok(receiver) => receiver
                    .recv()
                    .await
                    .unwrap_or_else(|_| Err("mcp.stale".into())),
                Err(_) => Err("mcp.stale".into()),
            };
            let _ = owner.update(cx, |this, cx| {
                if this.cards_detail_epoch != epoch {
                    return;
                }
                this.cards_detail_task = None;
                match result {
                    Ok(Reply::TableCardDetails(details)) => {
                        for detail in details {
                            while this.cards_details.len() >= 512 {
                                this.cards_details.pop_first();
                            }
                            let key =
                                serde_json::to_string(&(&detail.schema, &detail.name)).unwrap();
                            this.cards_details.insert(key, detail);
                        }
                    }
                    _ => {
                        for object in requested {
                            this.cards_details_failed.insert(detail_key(&object));
                        }
                    }
                }
                cx.notify();
            });
        }));
    }
    pub(super) fn reset_card_details(&mut self) {
        self.cards_detail_epoch += 1;
        self.cards_detail_task = None;
        self.cards_details.clear();
        self.cards_details_failed.clear();
    }
    fn render_card(&self, i: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(object) = self.cards.get(i).cloned() else {
            return div().flex_1().into_any_element();
        };
        let key = detail_key(&object);
        let detail = self.cards_details.get(&key);
        let comment = detail
            .map(|d| d.comment.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| object.comment.clone());
        let mut target = self.target.clone();
        target.object = object.clone();
        let menu_target = target.clone();
        let source = self.source.clone();
        let tooltip = format!(
            "{}\n{}\n{}",
            object.name,
            comment,
            detail.map(|d| d.primary.join(", ")).unwrap_or_default()
        );
        let mut card = div()
            .id(("table-card", i))
            .v_flex()
            .gap_2()
            .w_full()
            .h(px(72.))
            .p_2()
            .border_1()
            .border_color(cx.theme().border)
            .rounded_md()
            .tooltip(move |window, cx| {
                gpui_component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
            })
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .min_w_0()
                    .items_center()
                    .child(gpui_component::Icon::new(IconName::GalleryVerticalEnd))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(object.name.clone()),
                    )
                    .when(!comment.is_empty(), |line| {
                        line.child(
                            div()
                                .w(relative(0.45))
                                .text_right()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .truncate()
                                .child(comment.clone()),
                        )
                    }),
            );
        if let Some(detail) = detail {
            let line = div()
                .h_flex()
                .gap_1()
                .items_center()
                .min_w_0()
                .text_sm()
                .when(detail.primary.is_empty(), |line| {
                    line.child(t(cx, "database.cards.no_primary"))
                })
                .when(!detail.primary.is_empty(), |line| {
                    line.child(t(cx, "database.cards.primary"))
                        .child(
                            gpui_component::Icon::default()
                                .path("icons/key.svg")
                                .text_color(cx.theme().primary),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(detail.primary.join(", ")),
                        )
                })
                .when(detail.primary.is_empty(), |line| line.child(div().flex_1()))
                .when_some(detail.next_auto_increment.clone(), |line, value| {
                    let hint = tr(cx, "database.cards.next_id", &[("value", &value)]);
                    line.child(
                        div()
                            .id(("card-next-value", i))
                            .flex_shrink_0()
                            .text_color(cx.theme().muted_foreground)
                            .child(value)
                            .tooltip(move |window, cx| {
                                gpui_component::tooltip::Tooltip::new(hint.clone())
                                    .build(window, cx)
                            }),
                    )
                });
            card = card.child(line);
        } else {
            card = card.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(t(
                        cx,
                        if self.cards_details_failed.contains(&key) {
                            "database.cards.details_failed"
                        } else {
                            "database.cards.details_loading"
                        },
                    )),
            );
        }
        card = card.on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                if event.click_count == 2 {
                    let mut target = target.clone();
                    target.mode = Mode::Grid;
                    let _ = this.source.update(cx, |source, cx| {
                        if source.note_generation(target.connection) == target.generation {
                            target.writable = source
                                .note_profile(target.connection)
                                .is_some_and(|p| p.supports_grid_mutation());
                            cx.emit(target);
                        }
                    });
                }
            }),
        );
        div()
            .flex_1()
            .min_w_0()
            .child(crate::context_menu::scoped(
                SharedString::from(format!("card-menu-{i}")),
                card,
                move |menu, _, cx| {
                    DatabaseView::table_card_menu(menu, source.clone(), menu_target.clone(), cx)
                },
            ))
            .into_any_element()
    }
    pub(super) fn render_cards(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let count = self.cards_columns;
        let owner = cx.entity().downgrade();
        div()
            .v_flex()
            .relative()
            .size_full()
            .gap_2()
            .p_3()
            .child(
                canvas(
                    move |bounds, _, cx| {
                        let cols = columns(f32::from(bounds.size.width) - 24.);
                        if owner
                            .read_with(cx, |view, _| view.cards_columns != cols)
                            .unwrap_or(false)
                        {
                            let owner = owner.clone();
                            cx.defer(move |cx| {
                                let _ = owner.update(cx, |view, cx| {
                                    let first = view.cards_visible.start;
                                    view.cards_columns = cols;
                                    view.cards_scroll
                                        .scroll_to_item(first / cols, ScrollStrategy::Top);
                                    cx.notify();
                                });
                            });
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .items_center()
                    .child(Input::new(&self.cards_search).cleanable(true).flex_1())
                    .child(
                        Button::new("cards-refresh")
                            .ghost()
                            .icon(gpui_component::Icon::default().path("icons/refresh.svg"))
                            .tooltip(t(cx, "dbwork.refresh"))
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.reset_card_details();
                                this.cards.clear();
                                this.cards_offset = 0;
                                this.load(cx);
                            })),
                    ),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("{} / {}", self.target.database, self.cards.len())),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .pr(px(12.))
                    .child(
                        uniform_list(
                            "table-cards",
                            self.cards.len().div_ceil(count),
                            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                                this.load_visible_details(
                                    (range.start * count).min(this.cards.len())
                                        ..(range.end * count).min(this.cards.len()),
                                    cx,
                                );
                                range
                                    .map(|row| {
                                        div().h_flex().w_full().gap_2().h(px(80.)).children(
                                            (0..count).map(|column| {
                                                this.render_card(row * count + column, cx)
                                            }),
                                        )
                                    })
                                    .collect::<Vec<_>>()
                            }),
                        )
                        .track_scroll(self.cards_scroll.clone())
                        .size_full(),
                    )
                    .child(
                        div()
                            .absolute()
                            .right_0()
                            .top_0()
                            .bottom_0()
                            .w(px(12.))
                            .child(
                                Scrollbar::vertical(&self.cards_scroll)
                                    .scrollbar_show(ScrollbarShow::Always),
                            ),
                    ),
            )
            .when(self.cards.is_empty() && !self.busy, |el| {
                el.child(t(cx, "database.cards.empty"))
            })
            .when(!self.notice.is_empty(), |el| el.child(self.notice.clone()))
            .when(self.cards_more, |el| {
                el.child(
                    Button::new("cards-more")
                        .ghost()
                        .label(t(cx, "database.cards.more"))
                        .disabled(self.busy)
                        .on_click(cx.listener(|this, _, _, cx| this.load(cx))),
                )
            })
            .into_any_element()
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn responsive_columns_are_bounded() {
        assert_eq!(super::columns(200.), 1);
        assert_eq!(super::columns(600.), 2);
        assert_eq!(super::columns(1200.), 4);
        assert_eq!(super::columns(10000.), 8);
    }
}

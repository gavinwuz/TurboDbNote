use super::*;
use crate::{database::DatabaseView, locale::tr};
fn detail_key(object: &workbench::Object) -> String {
    serde_json::to_string(&(&object.schema, &object.name)).unwrap()
}
fn columns(width: f32) -> usize {
    ((width.max(0.) + 8.) / 280.).floor().clamp(1., 8.) as usize
}
fn masonry_positions(
    heights: impl IntoIterator<Item = f32>,
    count: usize,
    width: f32,
) -> (Vec<(f32, f32, f32)>, f32) {
    let mut bottoms = vec![0f32; count.max(1)];
    let mut positions = vec![];
    for height in heights {
        let column = bottoms
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.total_cmp(b.1))
            .map(|(i, _)| i)
            .unwrap_or(0);
        positions.push((column as f32 * (width + 12.), bottoms[column], height));
        bottoms[column] += height + 12.;
    }
    (positions, bottoms.into_iter().fold(0f32, f32::max))
}
pub(super) struct ErCard {
    pub object: workbench::Object,
    pub columns: Vec<workbench::Column>,
    pub scroll: ScrollHandle,
    pub width: f32,
    pub pending: Option<bool>,
    pub id: usize,
}
impl TableView {
    fn load_visible_details(&mut self, range: std::ops::Range<usize>, cx: &mut Context<Self>) {
        self.cards_visible = range.clone();
        if self.database_er() && self.cards_details.len() > 128 {
            let visible = self
                .cards
                .get(range.clone())
                .unwrap_or_default()
                .iter()
                .map(detail_key)
                .collect::<std::collections::HashSet<_>>();
            self.cards_details.retain(|key, _| visible.contains(key));
            self.er_card_scrolls.retain(|key, _| visible.contains(key));
        }
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
            .take(if self.database_er() { 4 } else { 12 })
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
                    if target.mode == Mode::Er {
                        Action::ErCardDetails(objects)
                    } else {
                        Action::TableCardDetails(objects)
                    },
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
                            if this.database_er() {
                                this.er_field_counts
                                    .insert(key.clone(), detail.columns.len());
                            }
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
    fn render_er_card(&mut self, i: usize, width: f32, cx: &mut Context<Self>) -> AnyElement {
        let Some(object) = self.cards.get(i).cloned() else {
            return div().into_any_element();
        };
        let key = detail_key(&object);
        let detail = self.cards_details.get(&key).cloned();
        let columns = detail
            .as_ref()
            .map(|d| d.columns.clone())
            .unwrap_or_default();
        let handle = self.er_card_scrolls.entry(key.clone()).or_default().clone();
        let pending = detail
            .is_none()
            .then(|| self.cards_details_failed.contains(&key));
        self.render_er_table_card(
            ErCard {
                object,
                columns,
                scroll: handle,
                width,
                pending,
                id: i,
            },
            cx,
        )
    }
    pub(super) fn render_er_table_card(&self, card: ErCard, cx: &mut Context<Self>) -> AnyElement {
        let ErCard {
            object,
            columns,
            scroll: handle,
            width,
            pending,
            id: i,
        } = card;
        let font = cx
            .global::<crate::preferences::Preferences>()
            .general
            .font_size;
        let title_hint = format!("{}\n{}", object.name, object.comment);
        let primary = columns.iter().filter(|c| c.primary).count();
        let count = columns.len();
        let mut target = self.target.clone();
        target.object = object.clone();
        target.mode = Mode::Grid;
        let menu_target = target.clone();
        let source = self.source.clone();
        let row_height = (font * 1.6).max(24.);
        let fields = div()
            .id(("database-er-fields", i))
            .track_scroll(&handle)
            .overflow_y_scroll()
            .h(px(count.clamp(1, 15) as f32 * row_height
                + if primary > 0 && primary < count {
                    6.
                } else {
                    0.
                }))
            .pr(px(if count > 15 { 14. } else { 4. }))
            .children(
                columns
                    .iter()
                    .filter(|c| c.primary)
                    .chain(columns.iter().filter(|c| !c.primary))
                    .enumerate()
                    .map(|(index, column)| {
                        let hint =
                            format!("{} · {}\n{}", column.name, column.data_type, column.comment);
                        div()
                            .id(("database-er-column", index))
                            .h_flex()
                            .items_center()
                            .gap_1()
                            .h(px(row_height))
                            .min_w_0()
                            .when(
                                primary > 0 && primary < count && index + 1 == primary,
                                |row| row.border_b_1().border_color(cx.theme().border),
                            )
                            .child(grid_ui::column_icon(Some(column), cx))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .child(column.name.clone()),
                            )
                            .tooltip(move |window, cx| {
                                gpui_component::tooltip::Tooltip::new(hint.clone())
                                    .build(window, cx)
                            })
                    }),
            )
            .when_some(pending, |row, failed| {
                row.child(if failed {
                    t(cx, "notes.retry")
                } else {
                    t(cx, "notes.loading")
                })
            });
        let card = div()
            .id(("database-er-card", i))
            .w(px(width))
            .flex_shrink_0()
            .border_1()
            .border_color(cx.theme().border)
            .rounded_md()
            .overflow_hidden()
            .v_flex()
            .relative()
            .on_drop(width_ui::drop_width)
            .child(
                div()
                    .id(("database-er-title", i))
                    .h_flex()
                    .gap_2()
                    .p_2()
                    .bg(cx.theme().secondary)
                    .child(gpui_component::Icon::new(IconName::GalleryVerticalEnd).size_4())
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(object.name.clone()),
                    )
                    .when(!object.comment.is_empty(), |row| {
                        row.child(
                            div()
                                .max_w(px(84.))
                                .min_w_0()
                                .text_right()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .text_color(cx.theme().muted_foreground)
                                .child(object.comment.clone()),
                        )
                    })
                    .child(width_ui::handle(
                        cx.entity().downgrade(),
                        width_ui::ER_WIDTH_KEY.into(),
                        width,
                        cx,
                    ))
                    .tooltip(move |window, cx| {
                        gpui_component::tooltip::Tooltip::new(title_hint.clone()).build(window, cx)
                    }),
            )
            .child(
                div()
                    .relative()
                    .min_w_0()
                    .child(fields)
                    .when(count > 15, |row| {
                        row.child(
                            div()
                                .absolute()
                                .right_0()
                                .top_0()
                                .bottom_0()
                                .w(px(12.))
                                .child(
                                    Scrollbar::vertical(&handle)
                                        .scrollbar_show(ScrollbarShow::Always),
                                ),
                        )
                    }),
            )
            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                if event.click_count() == 2 && this.database_er() {
                    let mut target = target.clone();
                    let _ = this.source.update(cx, |source, cx| {
                        if source.note_generation(target.connection) == target.generation {
                            target.writable = source
                                .note_profile(target.connection)
                                .is_some_and(|p| p.supports_grid_mutation());
                            cx.emit(target);
                        }
                    });
                }
            }));
        crate::context_menu::scoped(
            SharedString::from(format!("database-er-menu-{i}")),
            card,
            move |menu, _, cx| {
                DatabaseView::table_card_menu(menu, source.clone(), menu_target.clone(), cx)
            },
        )
        .into_any_element()
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
    fn render_er_masonry(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let available = (f32::from(self.er_viewport.width) - 16.).max(180.);
        let preferred = self.widths.er_width.unwrap_or(260.);
        let count = ((available + 12.) / (preferred + 12.)).floor().clamp(
            1.,
            if self.widths.er_width.is_some() {
                12.
            } else {
                4.
            },
        ) as usize;
        let width = self
            .widths
            .er_width
            .unwrap_or((available - (count - 1) as f32 * 12.) / count as f32)
            .min(available);
        let row_height = (cx
            .global::<crate::preferences::Preferences>()
            .general
            .font_size
            * 1.6)
            .max(24.);
        let top = -f32::from(self.er_masonry_scroll.offset().y);
        let viewport = f32::from(self.er_viewport.height).max(200.);
        let anchor = self
            .er_positions
            .iter()
            .enumerate()
            .find(|(_, (_, y, h))| *y + *h >= top)
            .map(|(i, (_, y, _))| (i, *y));
        let heights = self.cards.iter().map(|object| {
            let fields = self
                .er_field_counts
                .get(&detail_key(object))
                .copied()
                .unwrap_or(1)
                .clamp(1, 15);
            fields as f32 * row_height + row_height + 24.
        });
        let (positions, height) = masonry_positions(heights, count, width);
        let mut adjusted_top = top;
        if let Some((i, old_y)) = anchor
            && let Some((_, new_y, _)) = positions.get(i)
        {
            let delta = *new_y - old_y;
            if delta.abs() > 1. {
                adjusted_top = (top + delta).max(0.);
                self.er_masonry_scroll
                    .set_offset(point(px(0.), px(-adjusted_top)));
            }
        }
        self.er_positions = positions;
        let visible = self
            .er_positions
            .iter()
            .enumerate()
            .filter(|(_, (_, y, h))| {
                *y + *h >= adjusted_top - 200. && *y <= adjusted_top + viewport + 200.
            })
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        if let (Some(first), Some(last)) = (visible.first(), visible.last()) {
            self.load_visible_details(*first..*last + 1, cx);
        }

        let cards = visible
            .into_iter()
            .map(|i| {
                let (x, y, _) = self.er_positions[i];
                div()
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .child(self.render_er_card(i, width, cx))
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let owner = cx.entity().downgrade();
        let handle = self.er_masonry_scroll.clone();
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .on_drop(width_ui::drop_width)
            .child(
                canvas(
                    move |bounds, _, cx| {
                        if owner
                            .read_with(cx, |v, _| v.er_viewport != bounds.size)
                            .unwrap_or(false)
                        {
                            let owner = owner.clone();
                            cx.defer(move |cx| {
                                let _ = owner.update(cx, |v, cx| {
                                    v.er_viewport = bounds.size;
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
                    .id("er-masonry-scroll")
                    .track_scroll(&handle)
                    .overflow_y_scroll()
                    .size_full()
                    .pr(px(16.))
                    .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
                    .child(div().relative().w_full().h(px(height)).children(cards)),
            )
            .child(
                div()
                    .absolute()
                    .right_0()
                    .top_0()
                    .bottom_0()
                    .w(px(12.))
                    .child(Scrollbar::vertical(&handle).scrollbar_show(ScrollbarShow::Always)),
            )
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
                        if owner
                            .read_with(cx, |view, _| view.database_er())
                            .unwrap_or(false)
                        {
                            return;
                        }
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
                    .when(self.database_er(), |row| {
                        row.child(
                            Button::new("er-auto-width")
                                .ghost()
                                .label(t(cx, "er.width.auto"))
                                .on_click(cx.listener(|this, _, _, cx| this.reset_er_width(cx))),
                        )
                    })
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
            .child(if self.database_er() {
                self.render_er_masonry(cx)
            } else {
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
                                        div()
                                            .h_flex()
                                            .items_start()
                                            .w_full()
                                            .gap_2()
                                            .h(px(80.))
                                            .children((0..count).map(|column| {
                                                this.render_card(row * count + column, cx)
                                            }))
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
                    )
                    .into_any_element()
            })
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
    fn short_columns_are_filled_without_fixed_row_gaps() {
        let (positions, height) = super::masonry_positions([300., 60., 80., 40.], 2, 240.);
        assert_eq!(positions[2], (252., 72., 80.));
        assert_eq!(positions[3], (252., 164., 40.));
        assert_eq!(height, 312.);
        let (positions, _) = super::masonry_positions([40., 80.], 1, 240.);
        assert_eq!(positions[1].1, 52.);
    }

    #[test]
    fn responsive_columns_are_bounded() {
        assert_eq!(super::columns(200.), 1);
        assert_eq!(super::columns(600.), 2);
        assert_eq!(super::columns(1200.), 4);
        assert_eq!(super::columns(10000.), 8);
    }
}

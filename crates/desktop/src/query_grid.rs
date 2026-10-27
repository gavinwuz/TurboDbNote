use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme,
    table::{Column, TableDelegate, TableState},
    tooltip::Tooltip,
};
use turbodbn_services::database::workbench::Rows;

pub struct QueryGrid {
    pub rows: Rows,
    columns: Vec<Column>,
    offset: usize,
    row_numbers: bool,
    pub selected: Option<(usize, usize)>,
    difference_side: Option<usize>,
    status_labels: bool,
    pub scroll_link: Option<(std::rc::Rc<ScrollLink>, usize)>,
}
impl QueryGrid {
    pub fn new(rows: Rows, offset: usize) -> Self {
        let mut columns = vec![
            Column::new("row-number", "#")
                .width(px(52.))
                .fixed_left()
                .resizable(false),
        ];
        let widths = crate::grid_metrics::widths(&rows, 14.);
        columns.extend(rows.columns.iter().enumerate().map(|(i, name)| {
            Column::new(format!("column-{i}"), name.clone()).width(px(widths[i]))
        }));
        Self {
            rows,
            columns,
            offset,
            row_numbers: true,
            selected: None,
            difference_side: None,
            status_labels: false,
            scroll_link: None,
        }
    }
    pub fn with_status_labels(mut self) -> Self {
        self.status_labels = true;
        self
    }
    pub fn with_differences(mut self, side: usize) -> Self {
        self.difference_side = Some(side);
        self
    }
    pub fn with_column_widths(mut self, widths: &[f32]) -> Self {
        for (column, width) in self
            .columns
            .iter_mut()
            .skip(usize::from(self.row_numbers))
            .zip(widths)
        {
            column.width = px(*width);
        }
        self
    }
    pub fn fixed_columns(mut self, count: usize) -> Self {
        for column in self
            .columns
            .iter_mut()
            .skip(usize::from(self.row_numbers))
            .take(count)
        {
            *column = column.clone().fixed_left();
        }
        self
    }
    pub fn without_row_numbers(mut self) -> Self {
        self.columns.remove(0);
        self.row_numbers = false;
        self
    }
    pub fn selected_value(&self) -> Option<String> {
        let (row, col) = self.selected?;
        self.rows
            .values
            .get(row)?
            .get(col)
            .map(|v| v.clone().unwrap_or_else(|| "NULL".into()))
    }
}
impl TableDelegate for QueryGrid {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }
    fn rows_count(&self, _: &App) -> usize {
        self.rows.values.len()
    }
    fn column(&self, index: usize, _: &App) -> &Column {
        &self.columns[index]
    }
    fn render_td(
        &mut self,
        row: usize,
        index: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        if self.row_numbers && index == 0 {
            return div()
                .child((self.offset + row + 1).to_string())
                .into_any_element();
        }
        let col = index - usize::from(self.row_numbers);
        let value = self
            .rows
            .values
            .get(row)
            .and_then(|r| r.get(col))
            .and_then(|v| v.as_deref())
            .unwrap_or(if self.status_labels { "—" } else { "NULL" });
        let marker = self
            .rows
            .values
            .get(row)
            .and_then(|r| r.first())
            .and_then(|v| v.as_deref())
            .unwrap_or("=");
        let tint = self.difference_side.and_then(|side| match marker {
            "=" => None,
            "∅" => Some(cx.theme().muted_foreground),
            "-" => Some(cx.theme().danger),
            "+" => Some(cx.theme().success),
            "≠" => Some(cx.theme().warning),
            _ => Some(if side == 0 {
                cx.theme().danger
            } else {
                cx.theme().success
            }),
        });
        let value = if self.difference_side.is_some() && marker == "∅" && col > 0 {
            ""
        } else {
            value
        };
        let translated;
        let value = if self.status_labels && col == 0 {
            translated = crate::locale::t(
                cx,
                match marker {
                    "+" => "compare.status.added",
                    "-" => "compare.status.removed",
                    "≠" => "compare.status.modified",
                    _ => "compare.status.same",
                },
            );
            translated.as_ref()
        } else {
            value
        };
        let copy = value.to_string();
        let tooltip: SharedString = value.chars().take(4096).collect::<String>().into();
        let cell = div()
            .id(("query-value", row * 513 + index))
            .size_full()
            .px_1()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .when_some(tint, |el, tint| el.bg(tint.opacity(0.13)))
            .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
            .when(self.selected == Some((row, col)), |el| {
                el.bg(cx.theme().accent)
                    .border_1()
                    .border_color(cx.theme().primary)
            })
            .child(
                value
                    .chars()
                    .take(if self.difference_side.is_some() {
                        4096
                    } else {
                        256
                    })
                    .collect::<String>(),
            )
            .on_click(cx.listener(move |table, _, window, cx| {
                table.delegate_mut().selected = Some((row, col));
                if table.selected_row() != Some(row) {
                    table.set_selected_row(row, cx);
                }
                window.focus(&table.focus_handle(cx));
                cx.notify();
            }));
        crate::context_menu::cell(
            ("query-value-menu", row * 513 + index),
            cell,
            move |menu, _, cx| {
                let copy = copy.clone();
                menu.item(
                    gpui_component::menu::PopupMenuItem::new(crate::locale::t(
                        cx,
                        "grid.copy.cell",
                    ))
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                        crate::feedback::success(crate::locale::t(cx, "feedback.copied"), cx);
                    }),
                )
            },
        )
        .into_any_element()
    }
}

/// Shared read-only table presentation with explicit overflow scrollbars.
pub fn view(grid: &Entity<TableState<QueryGrid>>, cx: &App) -> AnyElement {
    use gpui_component::scroll::{Scrollbar, ScrollbarShow};
    let state = grid.read(cx);
    let horizontal = state.horizontal_scroll_handle.clone();
    let vertical = state.vertical_scroll_handle.clone();
    let link = state.delegate().scroll_link.clone();
    let horizontal = LinkedHandle {
        own: horizontal,
        peer: link.as_ref().map(|(l, i)| l.horizontal[1 - i].clone()),
        locked: link.as_ref().map(|(l, _)| l.locked.clone()),
    };
    let vertical = LinkedHandle {
        own: vertical,
        peer: link.as_ref().map(|(l, i)| l.vertical[1 - i].clone()),
        locked: link.as_ref().map(|(l, _)| l.locked.clone()),
    };
    div()
        .id(SharedString::from(format!(
            "query-grid-{}",
            grid.entity_id()
        )))
        .on_scroll_wheel(move |_, window, _| {
            if let Some((link, i)) = &link {
                let link = link.clone();
                let i = *i;
                window.on_next_frame(move |window, _| {
                    link.synchronize(i);
                    window.refresh();
                });
            }
        })
        .relative()
        .size_full()
        .pr_4()
        .pb_4()
        .min_h_0()
        .min_w_0()
        .child(
            gpui_component::table::Table::new(grid)
                .bordered(false)
                .scrollbar_visible(false, false),
        )
        .child(
            div()
                .absolute()
                .top(px(32.))
                .right_0()
                .bottom_0()
                .w(px(16.))
                .child(Scrollbar::vertical(&vertical).scrollbar_show(ScrollbarShow::Always)),
        )
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                .h(px(16.))
                .child(Scrollbar::horizontal(&horizontal).scrollbar_show(ScrollbarShow::Always)),
        )
        .into_any_element()
}

/// Shared only by explicitly paired comparison grids; no polling or entity cycles.
pub struct ScrollLink {
    pub locked: std::rc::Rc<std::cell::Cell<bool>>,
    horizontal: [gpui_component::VirtualListScrollHandle; 2],
    vertical: [UniformListScrollHandle; 2],
}
impl ScrollLink {
    pub fn new(
        grids: &[Entity<TableState<QueryGrid>>; 2],
        locked: std::rc::Rc<std::cell::Cell<bool>>,
        cx: &App,
    ) -> Self {
        Self {
            locked,
            horizontal: std::array::from_fn(|i| grids[i].read(cx).horizontal_scroll_handle.clone()),
            vertical: std::array::from_fn(|i| grids[i].read(cx).vertical_scroll_handle.clone()),
        }
    }
    pub fn synchronize(&self, i: usize) -> bool {
        use gpui_component::scroll::ScrollbarHandle;
        if !self.locked.get() {
            return false;
        }
        let horizontal = self.horizontal[i].offset();
        let vertical = self.vertical[i].offset();
        let changed = self.horizontal[1 - i].offset() != horizontal
            || self.vertical[1 - i].offset() != vertical;
        if changed {
            self.horizontal[1 - i].set_offset(horizontal);
            self.vertical[1 - i].set_offset(vertical);
        }
        changed
    }
}
#[derive(Clone)]
struct LinkedHandle<H> {
    own: H,
    peer: Option<H>,
    locked: Option<std::rc::Rc<std::cell::Cell<bool>>>,
}
impl<H: gpui_component::scroll::ScrollbarHandle + Clone> gpui_component::scroll::ScrollbarHandle
    for LinkedHandle<H>
{
    fn offset(&self) -> Point<Pixels> {
        self.own.offset()
    }
    fn set_offset(&self, offset: Point<Pixels>) {
        self.own.set_offset(offset);
        if self.locked.as_ref().is_some_and(|l| l.get())
            && let Some(peer) = &self.peer
        {
            peer.set_offset(offset);
        }
    }
    fn content_size(&self) -> Size<Pixels> {
        self.own.content_size()
    }
    fn start_drag(&self) {
        self.own.start_drag();
    }
    fn end_drag(&self) {
        self.own.end_drag();
    }
}

#[cfg(test)]
mod scroll_tests {
    use super::*;
    use gpui_component::scroll::ScrollbarHandle;
    #[::core::prelude::v1::test]
    fn attribute_grid_without_row_numbers_keeps_field_width_and_fixed_identity_columns() {
        let rows = Rows {
            columns: vec!["Δ".into(), "field".into(), "attribute".into()],
            values: vec![],
        };
        let grid = QueryGrid::new(rows, 0)
            .without_row_numbers()
            .fixed_columns(2)
            .with_column_widths(&[40., 160., 130.]);
        assert_eq!(grid.columns[0].width, px(40.));
        assert_eq!(grid.columns[1].width, px(160.));
        assert!(grid.columns[1].fixed.is_some());
        assert!(grid.columns[2].fixed.is_none());
    }
    #[::core::prelude::v1::test]
    fn comparison_handles_sync_both_directions_and_unlock_independently() {
        let link = ScrollLink {
            locked: std::rc::Rc::new(std::cell::Cell::new(true)),
            horizontal: std::array::from_fn(|_| gpui_component::VirtualListScrollHandle::new()),
            vertical: std::array::from_fn(|_| UniformListScrollHandle::new()),
        };
        link.horizontal[0].set_offset(point(px(-100.), px(0.)));
        link.vertical[0].set_offset(point(px(0.), px(-80.)));
        assert!(link.synchronize(0));
        assert_eq!(link.horizontal[1].offset(), link.horizontal[0].offset());
        assert_eq!(link.vertical[1].offset(), link.vertical[0].offset());
        link.locked.set(false);
        link.horizontal[1].set_offset(point(px(-200.), px(0.)));
        assert!(!link.synchronize(1));
        assert_eq!(link.horizontal[0].offset().x, px(-100.));
        link.locked.set(true);
        assert!(link.synchronize(1));
        assert_eq!(link.horizontal[0].offset().x, px(-200.));
        let bar = LinkedHandle {
            own: link.vertical[1].clone(),
            peer: Some(link.vertical[0].clone()),
            locked: Some(link.locked.clone()),
        };
        bar.set_offset(point(px(0.), px(-200.)));
        assert_eq!(link.vertical[0].offset().y, px(-200.));
        link.locked.set(false);
        bar.set_offset(point(px(0.), px(-300.)));
        assert_eq!(link.vertical[0].offset().y, px(-200.));
    }
}

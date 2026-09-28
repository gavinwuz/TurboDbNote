use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme,
    table::{Column, TableDelegate, TableState},
};
use turbodbn_services::database::workbench::Rows;

pub struct QueryGrid {
    pub rows: Rows,
    columns: Vec<Column>,
    offset: usize,
    pub selected: Option<(usize, usize)>,
}
impl QueryGrid {
    pub fn new(rows: Rows, offset: usize) -> Self {
        let mut columns = vec![
            Column::new("row-number", "#")
                .width(px(52.))
                .fixed_left()
                .resizable(false),
        ];
        columns.extend(
            rows.columns
                .iter()
                .enumerate()
                .map(|(i, name)| Column::new(format!("column-{i}"), name.clone()).width(px(180.))),
        );
        Self {
            rows,
            columns,
            offset,
            selected: None,
        }
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
        if index == 0 {
            return div()
                .child((self.offset + row + 1).to_string())
                .into_any_element();
        }
        let col = index - 1;
        let value = self
            .rows
            .values
            .get(row)
            .and_then(|r| r.get(col))
            .and_then(|v| v.as_deref())
            .unwrap_or("NULL");
        div()
            .id(("query-value", row * 513 + index))
            .size_full()
            .px_1()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .when(self.selected == Some((row, col)), |el| {
                el.bg(cx.theme().accent)
                    .border_1()
                    .border_color(cx.theme().primary)
            })
            .child(value.chars().take(256).collect::<String>())
            .on_click(cx.listener(move |table, _, window, cx| {
                table.delegate_mut().selected = Some((row, col));
                window.focus(&table.focus_handle(cx));
                cx.notify();
            }))
            .into_any_element()
    }
}

use crate::{
    database::{DatabaseView, OpenTable},
    locale::t,
};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Disableable, IconName, StyledExt,
    button::{Button, ButtonVariants},
    dock::{Panel, PanelEvent},
    input::{Input, InputState},
    menu::{ContextMenuExt, PopupMenuItem},
};
use turbodbn_services::database::workbench::{self, Action, Reply, Request, Rows, Schema};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Grid,
    Note,
    Er,
    Compare,
    Import,
    Export,
}
pub struct TableView {
    key: String,
    source: WeakEntity<DatabaseView>,
    target: OpenTable,
    focus: FocusHandle,
    rows: Rows,
    schema: Option<Schema>,
    offset: usize,
    more: bool,
    busy: bool,
    notice: String,
    sql: String,
    note: Entity<InputState>,
    comparison: Entity<InputState>,
    _note: Subscription,
    scroll: UniformListScrollHandle,
    baseline: Option<Rows>,
}
impl TableView {
    pub fn new(
        key: String,
        source: WeakEntity<DatabaseView>,
        target: OpenTable,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let note = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .placeholder(t(cx, "dbwork.note.placeholder"))
        });
        let comparison =
            cx.new(|cx| InputState::new(window, cx).placeholder(t(cx, "dbwork.compare.target")));
        cx.defer_in(window, |this, _, cx| {
            this.load(cx);
        });
        Self {
            key,
            source,
            target,
            focus: cx.focus_handle(),
            rows: Rows::default(),
            schema: None,
            offset: 0,
            more: false,
            busy: false,
            notice: String::new(),
            sql: String::new(),
            _note: cx.observe(&note, |_, _, cx| cx.notify()),
            note,
            comparison,
            scroll: UniformListScrollHandle::new(),
            baseline: None,
        }
    }
    fn request(&mut self, action: Action, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let target = cx.entity().downgrade();
        let request = Request {
            database: self.target.database.clone(),
            action,
        };
        match self.source.update(cx, |source, cx| {
            source.table_request(
                self.target.connection,
                self.target.generation,
                request,
                target,
                cx,
            )
        }) {
            Ok(Ok(())) => {
                self.busy = true;
                self.notice = t(cx, "dbwork.loading").to_string();
            }
            Ok(Err(e)) => self.notice = e,
            Err(_) => self.notice = t(cx, "dbwork.disconnected").to_string(),
        }
        cx.notify();
    }
    fn load(&mut self, cx: &mut Context<Self>) {
        if matches!(self.target.mode, Mode::Er | Mode::Import) {
            self.request(Action::Describe(self.target.object.clone()), cx);
        } else {
            self.request(
                Action::Page {
                    object: self.target.object.clone(),
                    offset: self.offset,
                },
                cx,
            );
        }
    }
    pub fn receive(&mut self, result: Result<Reply, String>, cx: &mut Context<Self>) {
        self.busy = false;
        match result {
            Err(e) => {
                self.baseline = None;
                crate::feedback::error(e.clone(), cx);
                self.notice = e;
            }
            Ok(Reply::Page {
                rows,
                offset,
                more,
                sql,
            }) => {
                if let Some(baseline) = self.baseline.take() {
                    if baseline.columns != rows.columns {
                        self.notice = t(cx, "dbwork.compare.columns").to_string();
                    } else {
                        let mut counts = std::collections::HashMap::<String, i64>::new();
                        for row in &baseline.values {
                            *counts
                                .entry(serde_json::to_string(row).unwrap_or_default())
                                .or_default() += 1;
                        }
                        for row in &rows.values {
                            *counts
                                .entry(serde_json::to_string(row).unwrap_or_default())
                                .or_default() -= 1;
                        }
                        let left: i64 = counts.values().filter(|v| **v > 0).sum();
                        let right: i64 = counts.values().filter(|v| **v < 0).map(|v| -v).sum();
                        self.notice = format!(
                            "当前页比较：左侧独有 {left} 行，右侧独有 {right} 行；不是全表比较"
                        );
                    }
                } else {
                    self.rows = rows;
                    self.offset = offset;
                    self.more = more;
                    self.sql = sql;
                    self.notice = t(cx, "dbwork.page.notice").to_string();
                }
            }
            Ok(Reply::Schema(schema)) => {
                self.schema = Some(schema);
                self.notice = String::new();
            }
            Ok(Reply::Executed) => {
                self.notice = t(cx, "dbwork.executed").to_string();
                crate::feedback::success(self.notice.clone(), cx);
            }
            _ => self.notice = t(cx, "dbwork.unexpected").to_string(),
        }
        cx.notify();
    }
    fn compare(&mut self, cx: &mut Context<Self>) {
        let name = self.comparison.read(cx).value().trim().to_string();
        if name.is_empty() {
            return;
        }
        let mut object = self.target.object.clone();
        object.name = name;
        self.baseline = Some(self.rows.clone());
        self.request(
            Action::Page {
                object,
                offset: self.offset,
            },
            cx,
        );
        if !self.busy {
            self.baseline = None;
        }
    }
    fn export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match workbench::export_csv(&self.rows) {
            Ok(text) => save_text(text, "data-page.csv", window, cx),
            Err(e) => {
                self.notice = e;
                cx.notify();
            }
        }
    }
    fn import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(schema) = self.schema.clone() else {
            return;
        };
        let target = self.target.clone();
        let pick = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t(cx, "dbwork.import.csv")),
        });
        cx.spawn_in(window, async move |view, cx| {
            if let Ok(Ok(Some(paths))) = pick.await
                && let Some(path) = paths.into_iter().next()
            {
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        use std::io::Read;
                        let mut text = String::new();
                        std::fs::File::open(path)
                            .map_err(|e| e.to_string())?
                            .take(48_001)
                            .read_to_string(&mut text)
                            .map_err(|e| e.to_string())?;
                        workbench::import_csv(
                            target.kind,
                            &target.database,
                            &target.object,
                            &schema.columns,
                            &text,
                        )
                    })
                    .await;
                let _ = cx.update(|window, cx| {
                    let _ = view.update(cx, |this, cx| match result {
                        Ok(sql) => this.preview(sql, window, cx),
                        Err(e) => {
                            this.notice = e;
                            cx.notify();
                        }
                    });
                });
            }
        })
        .detach();
    }
    fn preview(&mut self, sql: String, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity().downgrade();
        let target = format!("{} / {}", self.target.database, self.target.object.name);
        let guard = self
            .schema
            .as_ref()
            .map(|s| (self.target.object.clone(), s.ddl.clone()));
        crate::feedback::sql_preview(sql, target, window, cx, move |sql, _, cx| {
            view.update(cx, |this, cx| {
                this.request(
                    Action::Execute {
                        sql,
                        guard: guard.clone(),
                    },
                    cx,
                );
                this.busy
            })
            .unwrap_or(false)
        });
    }
    fn grid(&self, cx: &mut Context<Self>) -> AnyElement {
        let width = px((self.rows.columns.len().max(1) * 180) as f32);
        let head = div()
            .h_flex()
            .w(width)
            .h(px(32.))
            .bg(cx.theme().muted)
            .children(self.rows.columns.iter().map(|c| {
                div()
                    .w(px(180.))
                    .flex_shrink_0()
                    .px_2()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(c.clone())
            }));
        div()
            .id("data-grid-horizontal")
            .size_full()
            .overflow_x_scroll()
            .child(
                div().v_flex().w(width).h_full().child(head).child(
                    uniform_list(
                        "data-grid-rows",
                        self.rows.values.len(),
                        cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                            range
                                .map(|ix| {
                                    div()
                                        .h_flex()
                                        .h(px(30.))
                                        .border_b_1()
                                        .border_color(cx.theme().border)
                                        .children(this.rows.values[ix].iter().enumerate().map(
                                            |(col, v)| {
                                                let value =
                                                    v.clone().unwrap_or_else(|| "NULL".into());
                                                let copy = value.clone();
                                                div()
                                                    .id(("cell", ix * 512 + col))
                                                    .w(px(180.))
                                                    .flex_shrink_0()
                                                    .px_2()
                                                    .overflow_hidden()
                                                    .text_ellipsis()
                                                    .whitespace_nowrap()
                                                    .text_color(if v.is_none() {
                                                        cx.theme().muted_foreground
                                                    } else {
                                                        cx.theme().foreground
                                                    })
                                                    .child(value)
                                                    .context_menu(move |menu, _, cx| {
                                                        let copy = copy.clone();
                                                        menu.item(
                                                            PopupMenuItem::new(t(
                                                                cx,
                                                                "dbwork.copy",
                                                            ))
                                                            .on_click(move |_, _, cx| {
                                                                cx.write_to_clipboard(
                                                                    ClipboardItem::new_string(
                                                                        copy.clone(),
                                                                    ),
                                                                )
                                                            }),
                                                        )
                                                    })
                                            },
                                        ))
                                        .into_any_element()
                                })
                                .collect::<Vec<_>>()
                        }),
                    )
                    .track_scroll(self.scroll.clone())
                    .flex_1()
                    .min_h_0()
                    .w(width),
                ),
            )
            .into_any_element()
    }
    fn er(&self, cx: &Context<Self>) -> AnyElement {
        let Some(schema) = &self.schema else {
            return div().child(self.notice.clone()).into_any_element();
        };
        div()
            .id("er-diagram")
            .size_full()
            .overflow_scroll()
            .v_flex()
            .gap_4()
            .p_4()
            .child(
                div()
                    .w(px(340.))
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded_lg()
                    .p_3()
                    .v_flex()
                    .gap_2()
                    .child(div().text_lg().child(self.target.object.name.clone()))
                    .children(schema.columns.iter().map(|c| {
                        div()
                            .h_flex()
                            .gap_2()
                            .child(if c.primary {
                                gpui_component::Icon::default()
                                    .path("icons/key.svg")
                                    .into_any_element()
                            } else {
                                div().child("·").into_any_element()
                            })
                            .child(c.name.clone())
                            .child(
                                div()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(c.data_type.clone()),
                            )
                    })),
            )
            .children(schema.foreign_keys.values.iter().map(|r| {
                div().h_flex().gap_3().child("└──→").child(
                    div()
                        .border_1()
                        .border_color(cx.theme().border)
                        .rounded_lg()
                        .p_3()
                        .child(
                            r.iter()
                                .filter_map(|v| v.as_deref())
                                .collect::<Vec<_>>()
                                .join(" · "),
                        ),
                )
            }))
            .when(schema.foreign_keys.values.is_empty(), |el| {
                el.child(t(cx, "dbwork.er.empty"))
            })
            .into_any_element()
    }
}
impl EventEmitter<PanelEvent> for TableView {}
impl Focusable for TableView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for TableView {
    fn panel_name(&self) -> &'static str {
        "table-workspace"
    }
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::tabs::title(
            self.key.clone(),
            IconName::GalleryVerticalEnd,
            format!(
                "{} · {}",
                self.target.object.name,
                t(
                    cx,
                    match self.target.mode {
                        Mode::Grid => "dbwork.data",
                        Mode::Note => "dbwork.note",
                        Mode::Er => "dbwork.er",
                        Mode::Compare => "dbwork.compare",
                        Mode::Import => "dbwork.import",
                        Mode::Export => "dbwork.export",
                    }
                )
            ),
            cx,
        )
    }
    fn closable(&self, _: &App) -> bool {
        false
    }
    fn zoomable(&self, _: &App) -> Option<gpui_component::dock::PanelControl> {
        None
    }
}
impl Render for TableView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mode = self.target.mode;
        let mut body = div()
            .v_flex()
            .size_full()
            .min_h_0()
            .p_3()
            .gap_2()
            .track_focus(&self.focus)
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(format!(
                                "{} / {}",
                                self.target.database, self.target.object.name
                            )),
                    )
                    .child(
                        Button::new("table-refresh")
                            .ghost()
                            .icon(IconName::Redo)
                            .tooltip(t(cx, "dbwork.refresh"))
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.load(cx))),
                    )
                    .when(!matches!(mode, Mode::Er | Mode::Import), |el| {
                        el.child(
                            Button::new("table-export")
                                .ghost()
                                .icon(gpui_component::Icon::default().path("icons/export.svg"))
                                .label(t(cx, "dbwork.export.page"))
                                .disabled(self.busy || self.rows.columns.is_empty())
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.export(window, cx)),
                                ),
                        )
                    }),
            );
        if mode == Mode::Note {
            body = body
                .child(div().text_sm().child(self.sql.clone()))
                .child(Input::new(&self.note).h(px(140.)))
                .child(
                    Button::new("save-table-note")
                        .ghost()
                        .icon(gpui_component::Icon::default().path("icons/export.svg"))
                        .label(t(cx, "dbwork.note.save"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            let text = format!(
                                "# {}\n\n```sql\n{}\n```\n\n{}",
                                this.target.object.name,
                                this.sql,
                                this.note.read(cx).value()
                            );
                            save_text(text, "table-note.md", window, cx);
                        })),
                );
        }
        if mode == Mode::Compare {
            body = body.child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(Input::new(&self.comparison).flex_1())
                    .child(
                        Button::new("compare-page")
                            .label(t(cx, "dbwork.compare.page"))
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.compare(cx))),
                    ),
            );
        }
        if mode == Mode::Er {
            body = body.child(self.er(cx));
        } else if mode == Mode::Import {
            body = body.child(t(cx, "dbwork.import.notice")).child(
                Button::new("import-csv-file")
                    .icon(gpui_component::Icon::default().path("icons/import.svg"))
                    .label(t(cx, "dbwork.import.csv"))
                    .disabled(self.busy || self.schema.is_none())
                    .on_click(cx.listener(|this, _, window, cx| this.import(window, cx))),
            );
        } else {
            body = body
                .child(div().flex_1().min_h_0().child(self.grid(cx)))
                .child(
                    div()
                        .h_flex()
                        .gap_2()
                        .child(
                            Button::new("page-previous")
                                .ghost()
                                .icon(IconName::ChevronLeft)
                                .disabled(self.busy || self.offset == 0)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.offset = this.offset.saturating_sub(workbench::PAGE_SIZE);
                                    this.load(cx);
                                })),
                        )
                        .child(format!(
                            "{} · {} / {}",
                            self.offset / workbench::PAGE_SIZE + 1,
                            self.rows.values.len(),
                            workbench::PAGE_SIZE
                        ))
                        .child(
                            Button::new("page-next")
                                .ghost()
                                .icon(IconName::ChevronRight)
                                .disabled(self.busy || !self.more)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.offset += workbench::PAGE_SIZE;
                                    this.load(cx);
                                })),
                        ),
                );
        }
        body.child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(self.notice.clone()),
        )
    }
}
/// Write only after the native save picker. IO stays off the rendering thread.
pub fn save_text<T: 'static>(text: String, name: &str, window: &mut Window, cx: &mut Context<T>) {
    let directory = dirs::document_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    let pick = cx.prompt_for_new_path(&directory, Some(name));
    cx.spawn_in(window, async move |_, cx| {
        if let Ok(Ok(Some(path))) = pick.await {
            let result = cx
                .background_executor()
                .spawn(async move { std::fs::write(path, text).map_err(|e| e.to_string()) })
                .await;
            let _ = cx.update(|_, cx| match result {
                Ok(()) => crate::feedback::success(t(cx, "feedback.file.saved"), cx),
                Err(error) => crate::feedback::error(error, cx),
            });
        }
    })
    .detach();
}

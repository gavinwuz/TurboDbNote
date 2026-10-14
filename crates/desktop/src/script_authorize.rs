//! A single authorization bound to the frozen script and target; it never executes SQL itself.
use crate::{
    feedback,
    locale::{t, tr},
};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Disableable, Icon, IconName, Selectable, StyledExt, WindowExt,
    button::{Button, ButtonVariants},
    text::TextView,
};
use std::{
    rc::Rc,
    sync::{Arc, Mutex},
};
use turbodbn_services::sql_script::{self, AuthorizationSummary, Entry, Reader};
type Accept = Rc<dyn Fn(&mut Window, &mut App) -> bool>;
pub struct AuthorizationView {
    reader: Arc<Mutex<Reader>>,
    summary: AuthorizationSummary,
    target: String,
    start: u64,
    end: u64,
    legacy: bool,
    page: u64,
    risk_only: bool,
    rows: Vec<Entry>,
    preview: SharedString,
    loading: bool,
    error: SharedString,
    epoch: u64,
    task: Option<Task<()>>,
    accept: Accept,
    outcome: async_channel::Sender<bool>,
}
impl AuthorizationView {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        reader: Arc<Mutex<Reader>>,
        summary: AuthorizationSummary,
        target: String,
        start: u64,
        end: u64,
        legacy: bool,
        accept: Accept,
        outcome: async_channel::Sender<bool>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.defer_in(window, |this, _, cx| this.load(0, cx));
        Self {
            reader,
            summary,
            target,
            start,
            end,
            legacy,
            page: 0,
            risk_only: false,
            rows: vec![],
            preview: "".into(),
            loading: true,
            error: "".into(),
            epoch: 0,
            task: None,
            accept,
            outcome,
        }
    }
    fn load(&mut self, page: u64, cx: &mut Context<Self>) {
        self.page = page;
        self.epoch += 1;
        let epoch = self.epoch;
        self.loading = true;
        self.error = "".into();
        self.preview = "".into();
        let reader = self.reader.clone();
        let start = self.start;
        let end = self.end;
        let risk = self.risk_only;
        let legacy = self.legacy;
        let work = cx.background_executor().spawn(async move {
            reader
                .lock()
                .map_err(|_| "script.error.execution")?
                .authorization_page(start, end, page, risk, legacy)
        });
        self.task = Some(cx.spawn(async move |owner, cx| {
            let result = work.await;
            let _ = owner.update(cx, |this, cx| {
                if this.epoch != epoch {
                    return;
                }
                this.loading = false;
                match result {
                    Ok(rows) => this.rows = rows,
                    Err(error) => this.error = crate::locale::message(cx, &error),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn select(&mut self, number: u64, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.error = "".into();
        self.epoch += 1;
        let epoch = self.epoch;
        let reader = self.reader.clone();
        let work = cx.background_executor().spawn(async move {
            let mut reader = reader.lock().map_err(|_| "script.error.execution")?;
            Ok::<_, String>((reader.statement(number)?, reader.database_kind()))
        });
        self.task = Some(cx.spawn(async move |owner, cx| {
            let result = work.await;
            let _ = owner.update(cx, |this, cx| {
                if this.epoch != epoch {
                    return;
                }
                match result {
                    Ok((sql, kind)) => this.preview = crate::cell::sql_reading(&sql, kind).into(),
                    Err(error) => this.error = crate::locale::message(cx, &error),
                }
                cx.notify();
            });
        }));
    }
}
impl Drop for AuthorizationView {
    fn drop(&mut self) {
        let _ = self.outcome.try_send(false);
    }
}
impl Render for AuthorizationView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let total = if self.risk_only {
            self.summary.risky
        } else {
            self.end - self.start
        };
        let mut body = div().v_flex().gap_3().child(self.target.clone()).child(tr(
            cx,
            "script.batch.summary",
            &[
                ("total", &(self.end - self.start).to_string()),
                ("reads", &self.summary.reads.to_string()),
                ("writes", &self.summary.writes.to_string()),
                ("ddl", &self.summary.ddl.to_string()),
                ("session", &self.summary.session.to_string()),
                ("locks", &self.summary.locks.to_string()),
                ("transactions", &self.summary.transactions.to_string()),
            ],
        ));
        if self.summary.destructive > 0
            || self.summary.relaxed_checks > 0
            || self.summary.locks > 0
            || self.summary.transactions > 0
        {
            body = body.child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(Icon::new(IconName::TriangleAlert))
                    .child(tr(
                        cx,
                        "script.batch.risks",
                        &[
                            ("destructive", &self.summary.destructive.to_string()),
                            ("checks", &self.summary.relaxed_checks.to_string()),
                            ("locks", &self.summary.locks.to_string()),
                            ("transactions", &self.summary.transactions.to_string()),
                        ],
                    )),
            );
        }
        body = body
            .child(t(cx, "script.batch.warning"))
            .child(
                div().h_flex().child(
                    Button::new("batch-risk-only")
                        .ghost()
                        .icon(IconName::TriangleAlert)
                        .label(t(cx, "script.batch.only_risk"))
                        .selected(self.risk_only)
                        .disabled(self.loading)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.risk_only = !this.risk_only;
                            this.load(0, cx);
                        })),
                ),
            )
            .child(
                div()
                    .v_flex()
                    .h(px(180.))
                    .children(self.rows.iter().map(|row| {
                        let number = row.number;
                        div()
                            .id(SharedString::from(format!("authorize-row-{number}")))
                            .h_flex()
                            .h(px(30.))
                            .gap_2()
                            .px_2()
                            .items_center()
                            .hover(|s| s.bg(cx.theme().muted))
                            .child(div().w(px(44.)).child((number + 1).to_string()))
                            .child(div().w(px(48.)).child(format!("L{}", row.line)))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(row.preview.clone()),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| this.select(number, cx)))
                    })),
            )
            .child(
                div()
                    .h_flex()
                    .justify_end()
                    .gap_3()
                    .child(
                        Button::new("batch-preview-prev")
                            .ghost()
                            .icon(IconName::ChevronLeft)
                            .disabled(self.loading || self.page == 0)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.load(this.page.saturating_sub(sql_script::PAGE_SIZE), cx)
                            })),
                    )
                    .child(format!(
                        "{}–{} / {}",
                        if total == 0 { 0 } else { self.page + 1 },
                        (self.page + sql_script::PAGE_SIZE).min(total),
                        total
                    ))
                    .child(
                        Button::new("batch-preview-next")
                            .ghost()
                            .icon(IconName::ChevronRight)
                            .disabled(self.loading || self.page + sql_script::PAGE_SIZE >= total)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.load(this.page + sql_script::PAGE_SIZE, cx)
                            })),
                    ),
            );
        if !self.preview.is_empty() {
            body = body.child(
                div()
                    .id("batch-original-preview")
                    .max_h(px(140.))
                    .overflow_y_scroll()
                    .child(
                        TextView::markdown("batch-original-sql", self.preview.clone(), window, cx)
                            .selectable(true),
                    ),
            );
        }
        if !self.error.is_empty() {
            body = body.child(self.error.clone());
        }
        body.child(
            feedback::actions().child(feedback::close_button(cx)).child(
                Button::new("batch-authorize-execute")
                    .primary()
                    .icon(IconName::Check)
                    .label(t(cx, "script.batch.execute"))
                    .disabled(self.loading || !self.error.is_empty())
                    .on_click(cx.listener(|this, _, window, cx| {
                        let accept = this.accept.clone();
                        if accept(window, cx) {
                            let _ = this.outcome.try_send(true);
                            window.close_dialog(cx);
                        }
                    })),
            ),
        )
    }
}
#[allow(clippy::too_many_arguments)]
pub fn open(
    reader: Arc<Mutex<Reader>>,
    summary: AuthorizationSummary,
    target: String,
    start: u64,
    end: u64,
    legacy: bool,
    window: &mut Window,
    cx: &mut App,
    accept: impl Fn(&mut Window, &mut App) -> bool + 'static,
) -> async_channel::Receiver<bool> {
    let (sender, receiver) = async_channel::bounded(1);
    let view = cx.new(|cx| {
        AuthorizationView::new(
            reader,
            summary,
            target,
            start,
            end,
            legacy,
            Rc::new(accept),
            sender,
            window,
            cx,
        )
    });
    window.open_dialog(cx, move |dialog, _, cx| {
        dialog
            .title(t(cx, "script.batch.title"))
            .w(px(740.))
            .overlay_closable(false)
            .child(view.clone())
    });
    receiver
}

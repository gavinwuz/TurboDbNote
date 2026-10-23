//! Deferred resizing: only the drag guide moves; data/layout changes once on drop.
use super::*;
use serde::{Deserialize, Serialize};
const MIN: f32 = 64.;
const MAX: f32 = 1200.;
#[derive(Default, Clone, Serialize, Deserialize)]
pub(super) struct Layout {
    pub widths: BTreeMap<String, f32>,
}
impl Layout {
    fn valid(&self) -> bool {
        self.widths.len() <= 512
            && self
                .widths
                .iter()
                .all(|(k, v)| k.len() <= 4096 && v.is_finite() && (MIN..=MAX).contains(v))
    }
}
#[derive(Clone)]
pub(super) struct WidthDrag {
    owner: WeakEntity<TableView>,
    field: String,
    start: std::rc::Rc<std::cell::Cell<Pixels>>,
    width: f32,
}
impl Render for WidthDrag {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().w(px(2.)).h(px(1200.)).bg(cx.theme().primary)
    }
}
pub(super) fn handle(
    owner: WeakEntity<TableView>,
    field: String,
    width: f32,
    cx: &App,
) -> AnyElement {
    div()
        .id("deferred-column-resize")
        .h_full()
        .w(px(8.))
        .flex_shrink_0()
        .cursor_col_resize()
        .border_r_1()
        .border_color(cx.theme().border)
        .on_drag(
            WidthDrag {
                owner,
                field,
                start: std::rc::Rc::new(std::cell::Cell::new(px(0.))),
                width,
            },
            |drag, offset, window, cx| {
                cx.stop_propagation();
                drag.start
                    .set(window.mouse_position().x - offset.x + px(4.));
                cx.new(|_| drag.clone())
            },
        )
        .into_any_element()
}
pub(super) fn drop_width(drag: &WidthDrag, window: &mut Window, cx: &mut App) {
    let width =
        (drag.width + f32::from(window.mouse_position().x - drag.start.get())).clamp(MIN, MAX);
    let _ = drag
        .owner
        .update(cx, |this, cx| this.apply_width(&drag.field, width, cx));
}
impl TableView {
    pub(super) fn init_widths(&mut self, cx: &mut Context<Self>) {
        let Some(path) = crate::preferences::Preferences::path().ok() else {
            return;
        };
        let path = path.with_file_name("grid-widths-legacy.json");
        let key = format!(
            "grid-widths:{}",
            serde_json::to_string(&(
                self.target.connection,
                &self.target.database,
                &self.target.object.schema,
                &self.target.object.name
            ))
            .unwrap()
        );
        let load_path = path.clone();
        let load_key = key.clone();
        cx.spawn(async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    crate::app_store::load::<Layout>(
                        &load_path,
                        &load_key,
                        128 * 1024,
                        Layout::valid,
                    )
                })
                .await;
            let _ = view.update(cx, |this, cx| match result {
                Ok(layout) if this.width_revision == 0 => {
                    this.widths = layout;
                    this.sync_grid(cx);
                    let widths = this.widths.widths.clone();
                    this.grid_state.update(cx, |table, cx| {
                        for (field, width) in widths {
                            table.delegate_mut().set_width(&field, width);
                        }
                        table.refresh(cx);
                    });
                }
                Err(error) => {
                    this.widths_writable = false;
                    crate::feedback::error(error, cx);
                }
                _ => {}
            });
        })
        .detach();
        let (sender, receiver) = async_channel::bounded::<Layout>(1);
        self.width_sender = Some(sender);
        cx.spawn(async move |view, cx| {
            while let Ok(mut layout) = receiver.recv().await {
                while let Ok(latest) = receiver.try_recv() {
                    layout = latest;
                }
                let path = path.clone();
                let key = key.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move { crate::app_store::save(&path, &key, &layout, 128 * 1024) })
                    .await;
                if let Err(error) = result {
                    let _ = view.update(cx, |this, cx| {
                        this.widths_writable = false;
                        crate::feedback::error(error, cx);
                    });
                }
            }
        })
        .detach();
    }
    pub(super) fn apply_width(&mut self, field: &str, width: f32, cx: &mut Context<Self>) {
        if !width.is_finite() {
            return;
        }
        if !self.widths.widths.contains_key(field) && self.widths.widths.len() >= 512 {
            return;
        }
        self.widths
            .widths
            .insert(field.into(), width.clamp(MIN, MAX));
        self.width_revision += 1;
        self.grid_state.update(cx, |table, cx| {
            table.delegate_mut().set_width(field, width.clamp(MIN, MAX));
            table.refresh(cx);
        });
        if self.widths_writable
            && let Some(sender) = &self.width_sender
        {
            let _ = sender.force_send(self.widths.clone());
        }
    }
    pub(super) fn width_action(
        &mut self,
        col: usize,
        action: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(field) = self.rows.columns.get(col).cloned() else {
            return;
        };
        let width = self
            .widths
            .widths
            .get(&field)
            .copied()
            .or_else(|| self.auto_widths.get(&field).copied())
            .unwrap_or(220.);
        if action == "fit" {
            let maximum = self
                .rows
                .values
                .iter()
                .take(100)
                .filter_map(|r| r.get(col)?.as_deref())
                .map(|v| v.chars().take(128).count())
                .max()
                .unwrap_or(0)
                .max(field.chars().count());
            self.apply_width(&field, (maximum as f32 * 8. + 90.).clamp(MIN, 640.), cx);
            return;
        }
        if action == "reset" {
            self.widths.widths.remove(&field);
            self.auto_widths.remove(&field);
            self.width_revision += 1;
            self.sync_grid(cx);
            self.grid_state.update(cx, |table, cx| table.refresh(cx));
            if self.widths_writable
                && let Some(sender) = &self.width_sender
            {
                let _ = sender.force_send(self.widths.clone());
            }
            return;
        }
        let input =
            cx.new(|cx| InputState::new(window, cx).default_value((width as u32).to_string()));
        let owner = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let input = input.clone();
            let owner = owner.clone();
            let field = field.clone();
            dialog
                .title(t(cx, "grid.width.set"))
                .child(t(cx, "grid.width.hint"))
                .child(Input::new(&input).cleanable(true))
                .footer(move |_, _, _, cx| {
                    let input = input.clone();
                    let owner = owner.clone();
                    let field = field.clone();
                    vec![
                        crate::feedback::footer_actions()
                            .child(crate::feedback::close_button(cx))
                            .child(
                                Button::new("apply-column-width")
                                    .primary()
                                    .icon(IconName::Check)
                                    .label(t(cx, "grid.width.apply"))
                                    .on_click(move |_, window, cx| {
                                        let Some(width) =
                                            input.read(cx).value().parse::<f32>().ok().filter(
                                                |n| n.is_finite() && (MIN..=MAX).contains(n),
                                            )
                                        else {
                                            crate::feedback::error(t(cx, "grid.width.hint"), cx);
                                            return;
                                        };
                                        let _ = owner.update(cx, |this, cx| {
                                            this.apply_width(&field, width, cx)
                                        });
                                        window.close_dialog(cx);
                                    }),
                            ),
                    ]
                })
        });
    }
}

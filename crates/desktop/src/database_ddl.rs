use super::*;
use crate::locale::t;

#[derive(Default)]
pub(super) struct DdlState {
    origin: Option<Uuid>,
    preferred: Option<(Uuid, String)>,
    context: Option<(Uuid, String, u64)>,
    objects: Vec<Object>,
    selected: Option<Object>,
    text: String,
    notice: String,
    loaded: bool,
    pub(super) pending: bool,
    filter: Option<Entity<InputState>>,
    subscription: Option<Subscription>,
}

impl DatabaseView {
    pub(crate) fn set_ddl_context(&mut self, event: feedback::DdlContext, cx: &mut Context<Self>) {
        if event.scope.is_none() && event.origin.is_some() && event.origin != self.ddl.origin {
            return;
        }
        self.ddl.origin = event.origin;
        if self.ddl.preferred == event.scope && event.object.is_none() {
            return;
        }
        self.ddl.preferred = event.scope;
        self.ddl.context = None;
        self.ddl.selected = event.object;
        cx.notify();
    }
    pub(super) fn receive_ddl(
        &mut self,
        result: Result<OperationResult, String>,
        cx: &mut Context<Self>,
    ) {
        if self
            .ddl
            .context
            .as_ref()
            .is_none_or(|(id, _, generation)| self.note_generation(*id) != *generation)
        {
            cx.notify();
            return;
        }
        match result {
            Ok(OperationResult::Workbench(reply)) => match *reply {
                Reply::Catalog { objects, .. } => {
                    self.ddl.objects = objects
                        .into_iter()
                        .filter(|o| matches!(o.kind.as_str(), "table" | "view"))
                        .collect();
                    self.ddl.loaded = true;
                }
                Reply::Schema(schema) => {
                    self.ddl.text = schema.ddl;
                    if self.ddl.text.is_empty() {
                        self.ddl.notice = t(cx, "ddl.empty").to_string();
                    }
                }
                _ => {}
            },
            Err(error) => {
                self.ddl.loaded = true;
                self.ddl.notice = crate::locale::message(cx, &error).to_string();
            }
            _ => {}
        }
        cx.notify();
    }
    fn request_ddl(&mut self, action: Action, cx: &mut Context<Self>) -> bool {
        let Some((id, database, generation)) = self.ddl.context.clone() else {
            return false;
        };
        if self.active_operation.is_some() || self.note_generation(id) != generation {
            return false;
        }
        self.ddl.pending = true;
        self.ddl.pending = self.request_operation(
            id,
            OperationKind::Workbench(Box::new(workbench::Request { database, action })),
            cx,
        );
        self.ddl.pending
    }
    pub fn render_objects(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let scope = self
            .ddl
            .preferred
            .clone()
            .or_else(|| self.selected_database.clone());
        let context = scope
            .filter(|(id, _)| self.note_connection_state(*id) == Some(true))
            .map(|(id, db)| (id, db, self.note_generation(id)));
        if context != self.ddl.context && !self.ddl.pending {
            if self.ddl.context.is_some() {
                self.ddl.selected = None;
            }
            self.ddl.context = context;
            self.ddl.objects.clear();
            self.ddl.text.clear();
            self.ddl.notice.clear();
            self.ddl.loaded = false;
        }
        if self.ddl.filter.is_none() {
            let filter = cx.new(|cx| InputState::new(window, cx).placeholder(t(cx, "ddl.filter")));
            self.ddl.subscription = Some(cx.observe(&filter, |_, _, cx| cx.notify()));
            self.ddl.filter = Some(filter);
        }
        let filter = self.ddl.filter.as_ref().unwrap().clone();
        let search = filter.read(cx).value().to_lowercase();
        let mut body = div().v_flex().gap_2().min_w_0().size_full();
        let Some((id, database, _)) = self.ddl.context.clone() else {
            return body.child(t(cx, "ddl.choose_scope")).into_any_element();
        };
        if self
            .note_profile(id)
            .is_some_and(|p| p.kind == DatabaseKind::Redis)
        {
            return body.child(t(cx, "ddl.unsupported")).into_any_element();
        }
        if !self.ddl.loaded && !self.ddl.pending {
            cx.defer_in(window, |this, _, cx| {
                this.request_ddl(Action::Catalog, cx);
            });
        } else if self.ddl.loaded
            && self.ddl.text.is_empty()
            && self.ddl.notice.is_empty()
            && !self.ddl.pending
            && let Some(object) = self.ddl.selected.clone()
        {
            cx.defer_in(window, move |this, _, cx| {
                this.request_ddl(Action::Ddl(object), cx);
            });
        }
        let objects = self
            .ddl
            .objects
            .iter()
            .filter(|o| o.name.to_lowercase().contains(&search))
            .cloned()
            .collect::<Vec<_>>();
        let name = self
            .note_profile(id)
            .map(|p| p.name.as_str())
            .unwrap_or_default();
        body = body
            .child(
                div()
                    .text_sm()
                    .whitespace_normal()
                    .child(format!("{name} / {database}")),
            )
            .child(Input::new(&filter).cleanable(true))
            .child(
                Button::new("ddl-table")
                    .w_full()
                    .label(
                        self.ddl
                            .selected
                            .as_ref()
                            .map(|o| o.name.clone())
                            .unwrap_or_else(|| t(cx, "ddl.select_table").to_string()),
                    )
                    .dropdown_caret(true)
                    .dropdown_menu({
                        let owner = cx.entity().downgrade();
                        move |mut menu, _, _| {
                            for object in &objects {
                                let owner = owner.clone();
                                let object = object.clone();
                                menu = menu.item(
                                    PopupMenuItem::new(if object.schema.is_empty() {
                                        object.name.clone()
                                    } else {
                                        format!("{}.{}", object.schema, object.name)
                                    })
                                    .on_click(
                                        move |_, _, cx| {
                                            let _ = owner.update(cx, |this, cx| {
                                                if this.ddl.pending {
                                                    return;
                                                }
                                                this.ddl.selected = Some(object.clone());
                                                this.ddl.text.clear();
                                                this.ddl.notice.clear();
                                                cx.notify();
                                            });
                                        },
                                    ),
                                );
                            }
                            menu.scrollable(true).max_h(px(320.))
                        }
                    }),
            )
            .when(self.ddl.pending, |el| el.child(t(cx, "ddl.loading")))
            .when(!self.ddl.notice.is_empty(), |el| {
                el.child(div().whitespace_normal().child(self.ddl.notice.clone()))
            })
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(
                        Button::new("ddl-refresh")
                            .ghost()
                            .icon(crate::assets::refresh_icon())
                            .tooltip(t(cx, "dbwork.refresh"))
                            .disabled(self.ddl.pending)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.ddl.loaded = false;
                                this.ddl.text.clear();
                                this.ddl.notice.clear();
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("ddl-copy")
                            .ghost()
                            .icon(IconName::Copy)
                            .tooltip(t(cx, "dbwork.copy.ddl"))
                            .disabled(self.ddl.text.is_empty())
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(
                                    this.ddl.text.clone(),
                                ));
                                feedback::success(t(cx, "feedback.copied"), cx);
                            })),
                    ),
            );
        if !self.ddl.text.is_empty() {
            let fence = "~".repeat(
                self.ddl
                    .text
                    .lines()
                    .map(|l| l.chars().take_while(|c| *c == '~').count())
                    .max()
                    .unwrap_or(0)
                    .max(2)
                    + 1,
            );
            body = body.child(
                gpui_component::text::TextView::markdown(
                    "database-ddl",
                    format!("{fence}sql\n{}\n{fence}", self.ddl.text),
                    window,
                    cx,
                )
                .selectable(true),
            );
        }
        body.into_any_element()
    }
}

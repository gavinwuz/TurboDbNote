use super::*;
use std::sync::Arc;
use turbodbn_core::mongo::{self, Mutation, Page};
impl TableView {
    pub(super) fn mongo_pending(&self) -> bool {
        self.mongo_dirty
    }
    pub(super) fn receive_mongo(&mut self, page: Page, cx: &mut Context<Self>) {
        if self.mongo_pending() {
            crate::feedback::error(t(cx, "mongo.document.pending"), cx);
            return;
        }
        self.mongo_epoch += 1;
        let epoch = self.mongo_epoch;
        self.busy = true;
        let previous = self
            .mongo
            .as_ref()
            .map(|d| d.fields.clone())
            .unwrap_or_default();
        cx.spawn(async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let projection = mongo::project(&page, &previous)?;
                    Ok::<_, &'static str>((page, projection))
                })
                .await;
            let _ = view.update(cx, |this, cx| {
                if this.mongo_epoch != epoch {
                    return;
                }
                this.busy = false;
                if this.source.upgrade().is_none_or(|source| {
                    !source
                        .read(cx)
                        .mongo_context_current(this.target.connection, this.target.generation)
                }) {
                    cx.notify();
                    return;
                }
                match result {
                    Ok((page, projection)) => {
                        let projection = Arc::new(projection);
                        this.rows = Rows {
                            columns: projection.fields.clone(),
                            values: projection.cells.clone(),
                        };
                        this.schema = Some(Schema {
                            columns: projection
                                .fields
                                .iter()
                                .zip(&projection.types)
                                .map(|(name, ty)| workbench::Column {
                                    not_null: None,
                                    auto_increment: None,
                                    extra: String::new(),
                                    name: name.clone(),
                                    data_type: ty.clone(),
                                    comment: String::new(),
                                    primary: false,
                                })
                                .collect(),
                            ..Default::default()
                        });
                        this.offset = page.offset;
                        this.options.offset = page.offset;
                        this.last_options = this.options.clone();
                        this.more = page.more;
                        this.elapsed_ms = page.elapsed_ms;
                        this.sync_pager_inputs = true;
                        this.selected = None;
                        this.notice.clear();
                        if projection.limited {
                            this.notice = t(cx, "mongo.document.fields_limited").into();
                            crate::feedback::notify(
                                gpui_component::notification::NotificationType::Warning,
                                this.notice.clone(),
                                cx,
                            );
                        }
                        let writable = this.source.upgrade().is_some_and(|s| {
                            s.read(cx)
                                .mongo_write_allowed(this.target.connection, this.target.generation)
                        }) && this.target.object.kind != "view"
                            && !this.target.object.name.starts_with("system.");
                        let tree = cx.new(|_| {
                            crate::mongo_tree::MongoTree::new(
                                projection.clone(),
                                this.target.object.name.clone(),
                                writable,
                            )
                        });
                        this.mongo_subscription = Some(cx.subscribe(
                            &tree,
                            |this, tree, event: &crate::mongo_tree::Event, cx| match event {
                                crate::mongo_tree::Event::Changed => {
                                    this.mongo_dirty = tree.read(cx).dirty();
                                    cx.notify();
                                }
                                crate::mongo_tree::Event::Submit(change) => {
                                    let change = change.clone();
                                    this.mongo_preview = Some(change);
                                    cx.notify();
                                }
                            },
                        ));
                        this.mongo = Some(projection);
                        this.mongo_tree = Some(tree);
                        this.sync_grid(cx);
                    }
                    Err(key) => {
                        this.notice = t(cx, key).into();
                        crate::feedback::error(this.notice.clone(), cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn select_mongo_node(
        &mut self,
        row: usize,
        col: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Some(tree) = &self.mongo_tree {
            let path = self
                .rows
                .columns
                .get(col)
                .map(|s| mongo::child_path("", s))
                .unwrap_or_default();
            tree.update(cx, |tree, cx| tree.select(row, path, cx))
        } else {
            true
        }
    }
    pub(super) fn open_mongo_node(&mut self, row: usize, col: usize, cx: &mut Context<Self>) {
        self.select_mongo_node(row, col, cx);
        self.mongo_inspect = true;
        cx.notify();
    }
    pub(super) fn preview_mongo(
        &mut self,
        change: Mutation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy || !self.mongo_pending() {
            return;
        }
        let label = self
            .source
            .upgrade()
            .map(|s| s.read(cx).mongo_context_label(self.target.connection))
            .unwrap_or_default();
        let target = format!(
            "{} / {} / {}",
            label, self.target.database, self.target.object.name
        );
        let summary = match (
            mongo::parse(&change.original),
            mongo::parse(&change.replacement),
        ) {
            (Ok(old), Ok(new)) => {
                let mut keys = old
                    .as_object()
                    .unwrap()
                    .keys()
                    .chain(new.as_object().unwrap().keys())
                    .collect::<Vec<_>>();
                keys.sort();
                keys.dedup();
                keys.into_iter()
                    .filter(|key| old.get(*key) != new.get(*key))
                    .take(16)
                    .map(|key| {
                        format!(
                            "{}: {} → {}",
                            key.chars().take(64).collect::<String>(),
                            old.get(key)
                                .map(mongo::summary)
                                .unwrap_or_else(|| "∅".into()),
                            new.get(key)
                                .map(mongo::summary)
                                .unwrap_or_else(|| "∅".into())
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" · ")
            }
            _ => String::new(),
        };
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .code_editor("json")
                .default_value(format!(
                    "{}\n\n{}\n\n{}\n{}",
                    t(cx, "mongo.document.original"),
                    change.original,
                    t(cx, "mongo.document.replacement"),
                    change.replacement
                ))
        });
        let owner = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let owner = owner.clone();
            let change = change.clone();
            let target = target.clone();
            dialog
                .title(t(cx, "mongo.document.preview"))
                .w(px(800.))
                .overlay_closable(false)
                .child(crate::locale::tr(
                    cx,
                    "mongo.document.changes",
                    &[("fields", &summary)],
                ))
                .child(
                    div()
                        .w(px(750.))
                        .v_flex()
                        .gap_3()
                        .child(target.clone())
                        .child(crate::feedback::editable_multiline(&input, true, cx).h(px(330.))),
                )
                .footer(move |_, _, _, cx| {
                    let owner = owner.clone();
                    let change = change.clone();
                    let target = target.clone();
                    vec![
                        crate::feedback::footer_actions()
                            .child(crate::feedback::close_button(cx))
                            .child(
                                Button::new("mongo-confirm-preview")
                                    .primary()
                                    .icon(IconName::Check)
                                    .label(t(cx, "mongo.document.confirm"))
                                    .on_click(move |_, window, cx| {
                                        window.close_dialog(cx);
                                        let owner = owner.clone();
                                        let change = change.clone();
                                        crate::feedback::confirm(
                                            t(cx, "mongo.document.confirm"),
                                            crate::locale::tr(
                                                cx,
                                                "mongo.document.confirm_body",
                                                &[("target", &target)],
                                            ),
                                            window,
                                            cx,
                                            move |_, cx| {
                                                let _ = owner.update(cx, |this, cx| {
                                                    if this.busy
                                                        || !this.mongo_pending()
                                                        || this.target.object.name
                                                            != change.collection
                                                        || this.mongo_tree.as_ref().is_none_or(
                                                            |tree| !tree.read(cx).matches(&change),
                                                        )
                                                    {
                                                        return;
                                                    }
                                                    if let Some(tree) = &this.mongo_tree {
                                                        tree.update(cx, |tree, cx| {
                                                            tree.busy(true, cx)
                                                        });
                                                    }
                                                    this.committing = true;
                                                    this.request(
                                                        Action::MongoWrite(change.clone()),
                                                        cx,
                                                    );
                                                    if !this.busy {
                                                        this.committing = false;
                                                        if let Some(tree) = &this.mongo_tree {
                                                            tree.update(cx, |tree, cx| {
                                                                tree.busy(false, cx)
                                                            });
                                                        }
                                                    }
                                                });
                                            },
                                        );
                                    }),
                            ),
                    ]
                })
        });
    }
}

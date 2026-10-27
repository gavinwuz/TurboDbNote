use super::*;
type Target = (String, String, String, String);
fn target(grant: &Grant) -> Target {
    (
        grant.scope.clone(),
        grant.database.clone(),
        grant.object.clone(),
        grant.column.clone(),
    )
}
fn grant_delta(before: &[Grant], after: &[Grant]) -> (usize, usize, usize) {
    let same = |a: &Grant, b: &Grant| target(a) == target(b) && a.privilege == b.privilege;
    let added = after
        .iter()
        .filter(|a| !before.iter().any(|b| same(a, b)))
        .count();
    let removed = before
        .iter()
        .filter(|b| !after.iter().any(|a| same(a, b)))
        .count();
    let changed = after
        .iter()
        .filter(|a| {
            before
                .iter()
                .any(|b| same(a, b) && a.grantable != b.grantable)
        })
        .count();
    (added, removed, changed)
}
fn change_scope(draft: &mut Vec<Grant>, key: &Target, enable: bool) {
    if !enable {
        draft.retain(|g| target(g) != *key);
        return;
    }
    let grantable = key.0 != "column" && draft.iter().any(|g| target(g) == *key && g.grantable);
    for p in account_admin::privileges(&key.0) {
        if !draft.iter().any(|g| target(g) == *key && g.privilege == *p) {
            draft.push(Grant {
                scope: key.0.clone(),
                database: key.1.clone(),
                object: key.2.clone(),
                column: key.3.clone(),
                privilege: p.to_string(),
                grantable,
            });
        }
    }
}
pub(super) fn database_target(database: &str) -> Target {
    (
        "database".into(),
        account_admin::literal_database_scope(database),
        "*".into(),
        String::new(),
    )
}
fn grant_database(draft: &mut Vec<Grant>, database: &str) -> Target {
    let key = database_target(database);
    let before = draft.len();
    change_scope(draft, &key, true);
    for grant in &mut draft[before..] {
        grant.grantable = false;
    }
    key
}
fn grant_database_label(grant: &Grant) -> String {
    if grant.scope == "database" {
        account_admin::database_scope_label(&grant.database)
    } else {
        grant.database.clone()
    }
}
fn permission_description(privilege: &str) -> &'static str {
    match privilege {
        "ALTER" => "accounts.description.alter",
        "ALTER ROUTINE" => "accounts.description.alter_routine",
        "CREATE" => "accounts.description.create",
        "CREATE ROUTINE" => "accounts.description.create_routine",
        "CREATE TEMPORARY TABLES" => "accounts.description.create_temporary_tables",
        "CREATE USER" => "accounts.description.create_user",
        "CREATE VIEW" => "accounts.description.create_view",
        "DELETE" => "accounts.description.delete",
        "DROP" => "accounts.description.drop",
        "EVENT" => "accounts.description.event",
        "EXECUTE" => "accounts.description.execute",
        "FILE" => "accounts.description.file",
        "INDEX" => "accounts.description.index",
        "INSERT" => "accounts.description.insert",
        "LOCK TABLES" => "accounts.description.lock_tables",
        "PROCESS" => "accounts.description.process",
        "REFERENCES" => "accounts.description.references",
        "RELOAD" => "accounts.description.reload",
        "REPLICATION CLIENT" => "accounts.description.replication_client",
        "REPLICATION SLAVE" => "accounts.description.replication_slave",
        "SELECT" => "accounts.description.select",
        "SHOW DATABASES" => "accounts.description.show_databases",
        "SHOW VIEW" => "accounts.description.show_view",
        "SHUTDOWN" => "accounts.description.shutdown",
        "SUPER" => "accounts.description.super",
        "TRIGGER" => "accounts.description.trigger",
        "UPDATE" => "accounts.description.update",
        _ => "accounts.description.other",
    }
}
impl Privileges {
    fn writable(&self, cx: &App) -> bool {
        self.source
            .read_with(cx, |db, _| db.account_capability(&self.scope, true))
            .unwrap_or(false)
    }
    fn stage(&mut self, key: Target, privilege: &str, cx: &mut Context<Self>) {
        let Some(draft) = &mut self.draft else {
            return;
        };
        if let Some(index) = draft
            .iter()
            .position(|grant| target(grant) == key && grant.privilege == privilege)
        {
            draft.remove(index);
        } else {
            let grantable = key.0 != "column"
                && draft.iter().any(|grant| {
                    grant.scope == key.0
                        && grant.database == key.1
                        && grant.object == key.2
                        && grant.grantable
                });
            draft.push(Grant {
                scope: key.0,
                database: key.1,
                object: key.2,
                column: key.3,
                privilege: privilege.into(),
                grantable,
            });
        }
        cx.notify();
    }
    fn preview_grants(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || !self.writable(cx) {
            return;
        }
        let Some(after) = self.draft.clone() else {
            return;
        };
        let plan = GrantPlan {
            user: self.account.schema.clone(),
            host: self.account.parent.clone(),
            before: self.grants.clone(),
            after,
        };
        let sql = match account_admin::grant_sql(&plan) {
            Ok(sql) if !sql.is_empty() => sql.join("\n"),
            Ok(_) => {
                feedback::error(t(cx, "accounts.grants.no_changes"), cx);
                return;
            }
            Err(error) => {
                feedback::error(crate::locale::message(cx, &error), cx);
                return;
            }
        };
        let expected = sql.clone();
        let owner = cx.entity().downgrade();
        let connection = self
            .source
            .read_with(cx, |db, _| {
                db.note_profile(self.scope.id)
                    .map(|p| format!("{} · {}", p.name, p.environment))
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        let target = tr(
            cx,
            "accounts.grants.confirm",
            &[("account", &self.account.name), ("connection", &connection)],
        )
        .to_string();
        feedback::sql_preview(sql, target, window, cx, move |sql, window, cx| {
            if sql != expected {
                feedback::error(t(cx, "accounts.preview_changed"), cx);
                return false;
            }
            owner
                .update(cx, |this, cx| {
                    if this.busy || !this.writable(cx) || this.draft.as_ref() != Some(&plan.after) {
                        feedback::error(t(cx, "accounts.stale"), cx);
                        return false;
                    }
                    this.busy = true;
                    this.applying = true;
                    cx.notify();
                    let receiver = this.source.update(cx, |db, cx| {
                        db.mcp_request(
                            this.scope.id,
                            this.scope.generation,
                            this.scope.database.clone(),
                            Action::AccountApplyGrants {
                                plan: plan.clone(),
                                confirmed: true,
                            },
                            cx,
                        )
                    });
                    let Ok(receiver) = receiver else {
                        this.busy = false;
                        this.applying = false;
                        return false;
                    };
                    cx.spawn_in(window, async move |owner, cx| {
                        let result = receiver.recv().await;
                        let _ = owner.update_in(cx, |this, window, cx| {
                            this.busy = false;
                            this.applying = false;
                            if matches!(result, Ok(Ok(Reply::Executed))) {
                                this.draft = None;
                                feedback::success(t(cx, "accounts.grants.saved"), cx);
                                this.load(0, false, window, cx);
                            } else {
                                this.error = true;
                                feedback::error(t(cx, "accounts.grants.failed"), cx);
                            }
                            cx.notify();
                        });
                    })
                    .detach();
                    true
                })
                .unwrap_or(false)
        });
    }

    pub(crate) fn dirty(&self) -> bool {
        self.draft
            .as_ref()
            .is_some_and(|draft| draft != &self.grants)
    }
    pub(crate) fn running(&self) -> bool {
        self.applying
    }
    pub(crate) fn discard(&mut self, cx: &mut Context<Self>) {
        self.draft = None;
        cx.notify();
    }
    pub(crate) fn bookmark(&self) -> PermissionBookmark {
        PermissionBookmark {
            key: self.key.clone(),
            connection: self.scope.id,
            user: self.account.schema.clone(),
            host: self.account.parent.clone(),
        }
    }
    pub(crate) fn key(&self) -> &str {
        &self.key
    }
    fn begin_edit(&mut self) {
        if self.draft.is_none() {
            self.draft = Some(self.grants.clone());
        }
    }
    fn toggle_option(&mut self, key: &Target, cx: &mut Context<Self>) {
        self.begin_edit();
        let draft = self.draft.as_mut().unwrap();
        let enabled = draft
            .iter()
            .any(|g| g.scope == key.0 && g.database == key.1 && g.object == key.2 && g.grantable);
        for g in draft
            .iter_mut()
            .filter(|g| g.scope == key.0 && g.database == key.1 && g.object == key.2)
        {
            g.grantable = !enabled;
        }
        cx.notify();
    }
    fn request_refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if self.dirty() {
            let owner = cx.entity().downgrade();
            feedback::confirm_labels(
                t(cx, "accounts.refresh"),
                t(cx, "accounts.workbench.discard"),
                t(cx, "dbwork.draft.keep"),
                t(cx, "dbwork.draft.discard"),
                window,
                cx,
                move |window, cx| {
                    let _ = owner.update(cx, |this, cx| {
                        this.draft = None;
                        this.refresh_current(window, cx);
                    });
                },
            );
        } else {
            self.refresh_current(window, cx);
        }
    }
    fn refresh_current(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Ok(generation) = self
            .source
            .read_with(cx, |db, _| db.note_generation(self.scope.id))
        {
            self.scope.generation = generation;
        }
        self.load(0, false, window, cx);
    }
    fn load_local_objects(&mut self, append: bool, window: &mut Window, cx: &mut Context<Self>) {
        let context = (
            self.selected_db.clone(),
            self.object_filter.read(cx).value().to_lowercase(),
        );
        let cache = self
            .source
            .read_with(cx, |db, _| {
                db.note_completion_cache(self.scope.id, &context.0)
            })
            .ok()
            .flatten();
        self.object_context = Some(context.clone());
        self.object_task = None;
        if !append {
            self.object_names.clear();
            self.objects_more = false;
        }
        let Some(cache) = cache else {
            return;
        };
        let offset = if append { self.object_names.len() } else { 0 };
        let scope = self.scope.clone();
        let query = context.1.clone();
        let work = cx
            .background_executor()
            .spawn(async move { cache.read(&query, offset, 200) });
        self.object_task = Some(cx.spawn_in(window, async move |view, cx| {
            let page = work.await;
            let _ = view.update(cx, |this, cx| {
                if this.object_context.as_ref() != Some(&context) {
                    return;
                }
                this.object_task = None;
                if !this
                    .source
                    .read_with(cx, |db, _| db.account_capability(&scope, false))
                    .unwrap_or(false)
                {
                    return;
                }
                if let Ok(Some(page)) = page {
                    this.object_names.extend(page.objects);
                    this.objects_more = page.more;
                }
                cx.notify();
            });
        }));
    }
    pub(super) fn render_grants(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use gpui_component::{
            checkbox::Checkbox,
            menu::{DropdownMenu, PopupMenuItem},
            resizable::{h_resizable, resizable_panel},
        };
        let object_context = (
            self.selected_db.clone(),
            self.object_filter.read(cx).value().to_lowercase(),
        );
        if self.object_context.as_ref() != Some(&object_context) {
            self.load_local_objects(false, window, cx);
        }
        let writable = self.writable(cx) && !self.error && !self.busy;
        if self.selected_target.0 == "database" && self.selected_target.1 == self.selected_db {
            self.selected_target = database_target(&self.selected_db);
        }
        let query = self.filter.read(cx).value().to_lowercase();
        let object_query = self.object_filter.read(cx).value().to_lowercase();
        let values = self.draft.as_ref().unwrap_or(&self.grants);
        let mut directories = self
            .source
            .read_with(cx, |db, _| db.note_databases(self.scope.id))
            .unwrap_or_default();
        directories.extend(
            self.grants
                .iter()
                .chain(values)
                .filter(|g| g.scope != "global")
                .map(grant_database_label),
        );
        if !self.selected_db.is_empty() {
            directories.push(self.selected_db.clone());
        }
        directories.sort();
        directories.dedup();
        let mut dirs = div()
            .v_flex()
            .gap_1()
            .child(Input::new(&self.filter).cleanable(true).w_full());
        dirs = dirs.child(
            Button::new("grant-global")
                .ghost()
                .justify_start()
                .icon(IconName::Globe)
                .label(t(cx, "accounts.global"))
                .selected(self.selected_db.is_empty())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.selected_db.clear();
                    this.selected_target = ("global".into(), "*".into(), "*".into(), String::new());
                    cx.notify();
                })),
        );
        let directories = directories
            .into_iter()
            .filter(|db| query.is_empty() || db.to_lowercase().contains(&query))
            .collect::<Vec<_>>();
        let directories_more = directories.len() > self.directory_limit;
        for (i, db) in directories
            .into_iter()
            .take(self.directory_limit)
            .enumerate()
        {
            let selected = self.selected_db == db;
            dirs = dirs.child(
                Button::new(("permission-db", i))
                    .ghost()
                    .justify_start()
                    .icon(IconName::Folder)
                    .label(db.clone())
                    .tooltip(db.clone())
                    .selected(selected)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected_db = db.clone();
                        this.selected_target = database_target(&db);
                        cx.notify();
                    })),
            );
        }
        if directories_more {
            dirs = dirs.child(
                Button::new("permission-more-databases")
                    .ghost()
                    .label(t(cx, "browser.more"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.directory_limit += 200;
                        cx.notify();
                    })),
            );
        }
        let mut targets = std::collections::BTreeSet::<Target>::new();
        if self.selected_db.is_empty() {
            targets.insert(("global".into(), "*".into(), "*".into(), String::new()));
        }
        if !self.selected_db.is_empty() {
            targets.insert(database_target(&self.selected_db));
            for g in self
                .grants
                .iter()
                .chain(values)
                .filter(|g| grant_database_label(g) == self.selected_db)
            {
                targets.insert(target(g));
            }
            if let Some(key) = &self.editing_target
                && key.1 == self.selected_db
            {
                targets.insert(key.clone());
            }
            for object in &self.object_names {
                targets.insert((
                    "table".into(),
                    self.selected_db.clone(),
                    object.name.clone(),
                    String::new(),
                ));
            }
        }
        let narrow_targets = targets.clone();
        let mut objects = div()
            .v_flex()
            .gap_1()
            .child(Input::new(&self.object_filter).cleanable(true).w_full());
        for (i, key) in targets
            .into_iter()
            .filter(|k| {
                object_query.is_empty()
                    || k.2 == "*"
                    || format!("{} {}", k.2, k.3)
                        .to_lowercase()
                        .contains(&object_query)
            })
            .enumerate()
        {
            let label = if key.0 == "database"
                && key.1 != account_admin::literal_database_scope(&self.selected_db)
            {
                tr(
                    cx,
                    "accounts.workbench.existing_pattern",
                    &[("pattern", &key.1)],
                )
                .to_string()
            } else if key.2 == "*" {
                t(cx, "accounts.workbench.all_objects").to_string()
            } else if key.3.is_empty() {
                key.2.clone()
            } else {
                format!("{} · {}", key.2, key.3)
            };
            let selected = self.selected_target == key;
            objects = objects.child(
                Button::new(("permission-object", i))
                    .ghost()
                    .justify_start()
                    .label(label.clone())
                    .tooltip(label)
                    .selected(selected)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected_target = key.clone();
                        cx.notify();
                    })),
            );
        }
        if self.objects_more {
            objects =
                objects.child(
                    Button::new("permission-more-objects")
                        .ghost()
                        .label(t(cx, "browser.more"))
                        .disabled(self.object_task.is_some())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.load_local_objects(true, window, cx)
                        })),
                );
        }
        let key = self.selected_target.clone();
        let mut privileges = account_admin::privileges(&key.0)
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        privileges.extend(
            self.grants
                .iter()
                .chain(values)
                .filter(|g| target(g) == key)
                .map(|g| g.privilege.clone()),
        );
        privileges.sort();
        privileges.dedup();
        let mut rows = div().v_flex().gap_1().child(
            div()
                .h_flex()
                .gap_3()
                .child(
                    div()
                        .w(px(170.))
                        .flex_shrink_0()
                        .child(t(cx, "accounts.privilege")),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(160.))
                        .child(t(cx, "accounts.workbench.description")),
                )
                .child(
                    div()
                        .w(px(if self.narrow { 56. } else { 80. }))
                        .child(t(cx, "accounts.workbench.granted")),
                )
                .child(
                    div()
                        .w(px(if self.narrow { 56. } else { 80. }))
                        .child(t(cx, "accounts.workbench.grantable")),
                ),
        );
        for (i, privilege) in privileges.into_iter().enumerate() {
            let before = self
                .grants
                .iter()
                .find(|g| target(g) == key && g.privilege == privilege);
            let current = values
                .iter()
                .find(|g| target(g) == key && g.privilege == privilege);
            if self.only_granted && current.is_none() && before.is_none() {
                continue;
            }
            let checked = current.is_some();
            let option = current.is_some_and(|g| g.grantable);
            let changed = before != current;
            let supported = account_admin::privileges(&key.0).contains(&privilege.as_str());
            let k = key.clone();
            let p = privilege.clone();
            let opt = key.clone();
            let description = t(cx, permission_description(&privilege));
            rows = rows.child(
                div()
                    .h_flex()
                    .gap_3()
                    .items_center()
                    .py_1()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .w(px(170.))
                            .flex_shrink_0()
                            .child(privilege)
                            .when(changed, |el| {
                                el.child(div().text_xs().child(t(
                                    cx,
                                    if checked && before.is_some() {
                                        "accounts.workbench.pending_change"
                                    } else if checked {
                                        "accounts.grants.pending_add"
                                    } else {
                                        "accounts.grants.pending_remove"
                                    },
                                )))
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(160.))
                            .text_xs()
                            .whitespace_normal()
                            .child(description),
                    )
                    .child(
                        div().w(px(if self.narrow { 56. } else { 80. })).child(
                            Checkbox::new(("permission-enabled", i))
                                .checked(checked)
                                .disabled(!writable || !supported)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.begin_edit();
                                    this.stage(k.clone(), &p, cx);
                                    cx.emit(gpui_component::dock::PanelEvent::LayoutChanged);
                                })),
                        ),
                    )
                    .child(
                        div().w(px(if self.narrow { 56. } else { 80. })).child(
                            Checkbox::new(("permission-option", i))
                                .checked(option)
                                .disabled(!writable || !checked || key.0 == "column" || !supported)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.toggle_option(&opt, cx);
                                })),
                        ),
                    ),
            );
        }
        if self.only_granted && !self.grants.iter().chain(values).any(|g| target(g) == key) {
            rows = rows.child(t(cx, "accounts.workbench.empty_scope"));
        }
        let label = if key.0 == "global" {
            t(cx, "accounts.global").to_string()
        } else {
            format!(
                "{} / {}{}",
                if key.0 == "database" {
                    account_admin::database_scope_label(&key.1)
                } else {
                    key.1.clone()
                },
                key.2,
                if key.3.is_empty() {
                    String::new()
                } else {
                    format!(" / {}", key.3)
                }
            )
        };
        let mut permissions = div().v_flex().gap_3().child(label.clone()).child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(t(cx, "accounts.workbench.option_hint")),
        );
        if !self.selected_db.is_empty() {
            permissions = permissions.child(
                Button::new("grant-database-all-tables")
                    .ghost()
                    .small()
                    .icon(IconName::Check)
                    .label(t(cx, "accounts.workbench.grant_database_all"))
                    .tooltip(t(cx, "accounts.workbench.grant_database_hint"))
                    .disabled(!writable)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.begin_edit();
                        let key = grant_database(this.draft.as_mut().unwrap(), &this.selected_db);
                        this.selected_target = key;
                        feedback::success(t(cx, "accounts.workbench.bulk_staged"), cx);
                        cx.notify();
                    })),
            );
        }
        permissions = permissions.child(rows);
        let bulk = key.clone();
        permissions = permissions.child(
            div().h_flex().gap_2().children(
                [
                    (true, "accounts.workbench.select_all"),
                    (false, "accounts.workbench.clear_all"),
                ]
                .into_iter()
                .map(|(enable, label)| {
                    let key = bulk.clone();
                    Button::new(if enable { "grant-all" } else { "revoke-all" })
                        .ghost()
                        .small()
                        .label(t(cx, label))
                        .disabled(!writable)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.begin_edit();
                            change_scope(this.draft.as_mut().unwrap(), &key, enable);
                            cx.notify();
                        }))
                }),
            ),
        );
        let narrow = self.narrow;
        if narrow {
            let owner = cx.entity().downgrade();
            let label = label.clone();
            permissions = div()
                .v_flex()
                .gap_2()
                .child(Input::new(&self.object_filter).cleanable(true).w_full())
                .child(
                    Button::new("permission-select-object")
                        .ghost()
                        .label(label)
                        .dropdown_menu(move |mut menu, _, _| {
                            for key in &narrow_targets {
                                let key = key.clone();
                                let owner = owner.clone();
                                let label = if key.2 == "*" {
                                    "*".to_string()
                                } else {
                                    format!("{} {}", key.2, key.3)
                                };
                                menu = menu.item(PopupMenuItem::new(label).on_click(
                                    move |_, _, cx| {
                                        let _ = owner.update(cx, |this, cx| {
                                            this.selected_target = key.clone();
                                            cx.notify();
                                        });
                                    },
                                ));
                            }
                            menu.scrollable(true).max_h(px(300.))
                        }),
                )
                .child(permissions);
        }
        let panes = h_resizable("permission-panes")
            .child(
                resizable_panel()
                    .size(px(if narrow { 160. } else { 210. }))
                    .size_range(px(120.)..px(420.))
                    .child(grant_scroll(
                        "permission-directories",
                        dirs,
                        &self.directory_scroll,
                    )),
            )
            .when(!narrow, |group| {
                group.child(
                    resizable_panel()
                        .size(px(220.))
                        .size_range(px(140.)..px(480.))
                        .child(grant_scroll(
                            "permission-objects",
                            objects,
                            &self.object_scroll,
                        )),
                )
            })
            .child(resizable_panel().child(grant_scroll(
                "permission-values",
                permissions,
                &self.scroll,
            )));
        let mut header = div()
            .h_flex()
            .gap_3()
            .items_center()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(self.account.name.clone()),
            )
            .child(self.scope_editor(cx))
            .child(
                Button::new("permission-only-granted")
                    .ghost()
                    .small()
                    .icon(IconName::Check)
                    .label(if narrow {
                        SharedString::default()
                    } else {
                        t(cx, "accounts.workbench.only_granted")
                    })
                    .tooltip(t(cx, "accounts.workbench.only_granted"))
                    .selected(self.only_granted)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.only_granted = !this.only_granted;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("refresh-grants")
                    .ghost()
                    .icon(crate::assets::refresh_icon())
                    .label(if narrow {
                        SharedString::default()
                    } else {
                        t(cx, "accounts.refresh")
                    })
                    .tooltip(t(cx, "accounts.refresh"))
                    .disabled(self.busy)
                    .on_click(cx.listener(|this, _, w, cx| this.request_refresh(w, cx))),
            );
        if !narrow
            && let Ok(Some(profile)) = self
                .source
                .read_with(cx, |db, _| db.note_profile(self.scope.id).cloned())
        {
            header = header.child(
                div()
                    .text_xs()
                    .child(format!("{} · {}", profile.name, profile.environment)),
            );
        }
        let (add, remove, changed) = grant_delta(&self.grants, values);
        let footer = div()
            .h_flex()
            .flex_shrink_0()
            .gap_3()
            .items_center()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_xs()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(tr(
                        cx,
                        "accounts.workbench.changes",
                        &[
                            ("add", &add.to_string()),
                            ("remove", &remove.to_string()),
                            ("changed", &changed.to_string()),
                        ],
                    )),
            )
            .child(
                feedback::footer_actions()
                    .flex_shrink_0()
                    .child(
                        Button::new("discard-grants")
                            .ghost()
                            .label(t(cx, "accounts.grants.cancel"))
                            .disabled(!self.dirty() || self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.discard(cx))),
                    )
                    .child(
                        Button::new("preview-grants")
                            .primary()
                            .icon(IconName::Check)
                            .label(t(cx, "accounts.preview"))
                            .disabled(!self.dirty() || !writable)
                            .on_click(cx.listener(|this, _, w, cx| this.preview_grants(w, cx))),
                    ),
            );
        let owner = cx.entity().downgrade();
        div()
            .relative()
            .child(
                canvas(
                    move |bounds, _, cx| {
                        let narrow = f32::from(bounds.size.width) < 850.;
                        if owner
                            .read_with(cx, |v, _| v.narrow != narrow)
                            .unwrap_or(false)
                        {
                            let owner = owner.clone();
                            cx.defer(move |cx| {
                                let _ = owner.update(cx, |v, cx| {
                                    v.narrow = narrow;
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
            .v_flex()
            .size_full()
            .min_h_0()
            .min_w_0()
            .p_3()
            .gap_3()
            .track_focus(&self.focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    crate::tabs::layout(crate::tabs::LayoutAction::Activate(this.key.clone()), cx)
                }),
            )
            .child(header)
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(t(cx, "accounts.permissions_hint")),
            )
            .when(self.busy, |body| {
                body.child(t(cx, "accounts.workbench.loading"))
            })
            .when(self.error, |body| {
                body.child(t(cx, "accounts.workbench.stale"))
            })
            .child(div().flex_1().min_h_0().child(panes))
            .child(footer)
            .into_any_element()
    }
    fn scope_editor(&self, cx: &mut Context<Self>) -> AnyElement {
        Button::new("permission-add-range")
            .ghost()
            .icon(IconName::Plus)
            .label(if self.narrow {
                SharedString::default()
            } else {
                t(cx, "accounts.grants.add_scope")
            })
            .tooltip(t(cx, "accounts.grants.add_scope"))
            .disabled(self.busy || self.error || !self.writable(cx))
            .on_click(cx.listener(|this, _, window, cx| {
                let db = this.grant_db.clone();
                let table = this.grant_table.clone();
                let column = this.grant_column.clone();
                let owner = cx.entity().downgrade();
                db.update(cx, |input, cx| {
                    input.set_value(this.selected_db.clone(), window, cx)
                });
                table.update(cx, |input, cx| input.set_value("", window, cx));
                column.update(cx, |input, cx| input.set_value("", window, cx));
                window.open_dialog(cx, move |dialog, _, cx| {
                    let db = db.clone();
                    let table = table.clone();
                    let column = column.clone();
                    let owner = owner.clone();
                    dialog
                        .title(t(cx, "accounts.grants.add_scope"))
                        .w(px(520.))
                        .overlay_closable(false)
                        .child(
                            div()
                                .v_flex()
                                .gap_3()
                                .child(Input::new(&db).cleanable(true))
                                .child(Input::new(&table).cleanable(true))
                                .child(Input::new(&column).cleanable(true))
                                .child(t(cx, "accounts.workbench.scope_hint")),
                        )
                        .footer(move |_, _, _, cx| {
                            let db = db.clone();
                            let table = table.clone();
                            let column = column.clone();
                            let owner = owner.clone();
                            vec![
                                feedback::footer_actions()
                                    .child(feedback::close_button(cx))
                                    .child(
                                        Button::new("confirm-permission-range")
                                            .primary()
                                            .icon(IconName::Plus)
                                            .label(t(cx, "accounts.grants.add_scope"))
                                            .on_click(move |_, window, cx| {
                                                let database =
                                                    db.read(cx).value().trim().to_owned();
                                                let object =
                                                    table.read(cx).value().trim().to_owned();
                                                let column =
                                                    column.read(cx).value().trim().to_owned();
                                                if database.is_empty() {
                                                    feedback::error(
                                                        t(cx, "accounts.grants.invalid"),
                                                        cx,
                                                    );
                                                    return;
                                                }
                                                let key = (
                                                    if !column.is_empty() {
                                                        "column"
                                                    } else if object.is_empty() || object == "*" {
                                                        "database"
                                                    } else {
                                                        "table"
                                                    }
                                                    .into(),
                                                    database.clone(),
                                                    if object.is_empty() {
                                                        "*".into()
                                                    } else {
                                                        object
                                                    },
                                                    column,
                                                );
                                                let _ = owner.update(cx, |this, cx| {
                                                    this.selected_db = database;
                                                    this.selected_target = key.clone();
                                                    this.editing_target = Some(key);
                                                    cx.notify();
                                                });
                                                window.close_dialog(cx);
                                            }),
                                    )
                                    .into_any_element(),
                            ]
                        })
                });
            }))
            .into_any_element()
    }
}
fn grant_scroll(id: &'static str, content: Div, handle: &ScrollHandle) -> AnyElement {
    crate::content_scroll::view(id, handle, content.w_full())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn grant(database: &str, privilege: &str, grantable: bool) -> Grant {
        Grant {
            scope: "database".into(),
            database: database.into(),
            object: "*".into(),
            column: String::new(),
            privilege: privilege.into(),
            grantable,
        }
    }
    #[::core::prelude::v1::test]
    fn database_shortcut_is_literal_and_preserves_existing_grants() {
        let mut before = vec![grant("other", "SELECT", true)];
        let existing = grant("account\\_center", "SELECT", true);
        before.push(existing.clone());
        let mut after = before.clone();
        let key = grant_database(&mut after, "account_center");
        assert_eq!(key.1, "account\\_center");
        assert!(before.iter().all(|g| after.contains(g)));
        assert!(after.iter().skip(before.len()).all(|g| !g.grantable));
        let sql = account_admin::grant_sql(&GrantPlan {
            user: "fixture".into(),
            host: "localhost".into(),
            before: before.clone(),
            after,
        })
        .unwrap();
        assert!(sql.iter().all(
            |sql| sql.contains("ON `account\\_center`.*") && !sql.contains("WITH GRANT OPTION")
        ));
        let mut draft = vec![];
        grant_database(&mut draft, "account_center");
        assert!(
            account_admin::grant_sql(&GrantPlan {
                user: "fixture".into(),
                host: "localhost".into(),
                before: vec![],
                after: draft
            })
            .is_ok()
        );
    }
    #[::core::prelude::v1::test]
    fn scope_bulk_preserves_unknown_privileges_options_and_other_scopes() {
        let original = vec![
            grant("a", "SELECT", true),
            grant("a", "FUTURE_PRIVILEGE", true),
            grant("b", "UPDATE", false),
        ];
        let key = target(&original[0]);
        let mut draft = original.clone();
        change_scope(&mut draft, &key, true);
        assert!(original.iter().all(|g| draft.contains(g)));
        assert!(
            draft
                .iter()
                .filter(|g| g.database == "a")
                .all(|g| g.grantable)
        );
        change_scope(&mut draft, &key, false);
        assert_eq!(draft, vec![original[2].clone()]);
    }
    #[::core::prelude::v1::test]
    fn bulk_column_grants_do_not_invent_column_grant_options() {
        let old = Grant {
            scope: "column".into(),
            database: "db".into(),
            object: "items".into(),
            column: "value".into(),
            privilege: "SELECT".into(),
            grantable: true,
        };
        let mut draft = vec![old.clone()];
        change_scope(&mut draft, &target(&old), true);
        assert!(draft.contains(&old));
        assert!(
            draft
                .iter()
                .filter(|g| g.privilege != "SELECT")
                .all(|g| !g.grantable)
        );
        assert!(
            account_admin::grant_sql(&GrantPlan {
                user: "fixture".into(),
                host: "localhost".into(),
                before: vec![old],
                after: draft
            })
            .is_ok()
        );
    }
    #[::core::prelude::v1::test]
    fn permission_delta_separates_new_removed_and_option_changes() {
        let before = vec![
            grant("a", "SELECT", false),
            grant("a", "UPDATE", false),
            grant("b", "INSERT", false),
        ];
        let after = vec![
            grant("a", "SELECT", true),
            grant("a", "DELETE", false),
            grant("b", "INSERT", false),
        ];
        assert_eq!(grant_delta(&before, &after), (1, 1, 1));
        assert_eq!(grant_delta(&before, &before), (0, 0, 0));
    }
}

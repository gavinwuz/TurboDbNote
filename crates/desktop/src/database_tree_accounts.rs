//! Read-only, explicitly expanded account directories; never part of object search.
use super::*;
pub(super) fn supported(profile: &Profile) -> bool {
    match profile.kind {
        DatabaseKind::MySql => matches!(
            profile.driver_id.as_str(),
            "" | "mysql" | "mariadb" | "tidb"
        ),
        DatabaseKind::PostgreSql | DatabaseKind::ClickHouse => true,
        _ => false,
    }
}
impl DatabaseView {
    pub(super) fn tree_databases_row(&self, scope: Scope, cx: &mut Context<Self>) -> AnyElement {
        let key = scope.key("databases-closed");
        let open = if self.tree_object_query(cx).is_empty() {
            !self.navigation.tree_nodes.contains(&key)
        } else {
            !self.tree.filter.collapsed.contains(&key)
        };
        Button::new(SharedString::from(key.clone()))
            .ghost()
            .w_full()
            .justify_start()
            .pl(px(26.))
            .icon(if open {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            })
            .child(
                gpui_component::Icon::default()
                    .path("icons/database.svg")
                    .size_4(),
            )
            .child(t(cx, "database.branch.databases"))
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.tree_object_query(cx).is_empty() {
                    this.tree_toggle(key.clone(), cx);
                } else if !this.tree.filter.collapsed.remove(&key) {
                    this.tree.filter.collapsed.insert(key.clone());
                    this.tree.revision += 1;
                    cx.notify();
                } else {
                    this.tree.revision += 1;
                    cx.notify();
                }
            }))
            .into_any_element()
    }
    pub(super) fn tree_users_row(&self, scope: Scope, cx: &mut Context<Self>) -> AnyElement {
        let menu_scope = scope.clone();
        let key = scope.key("users");
        let open = self.navigation.tree_nodes.contains(&key);
        let busy = self
            .tree
            .pages
            .get(&page_key(&scope, "user"))
            .is_some_and(|page| !page.busy.is_empty());
        let refresh = scope.clone();
        let row = div()
            .h_flex()
            .w_full()
            .child(
                Button::new(SharedString::from(key.clone()))
                    .ghost()
                    .flex_1()
                    .min_w_0()
                    .justify_start()
                    .pl(px(26.))
                    .icon(if open {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .child(gpui_component::Icon::new(IconName::User).size_4())
                    .child(t(cx, "database.branch.users"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.tree_toggle(key.clone(), cx)
                            && this
                                .tree
                                .pages
                                .get(&page_key(&scope, "user"))
                                .is_none_or(|page| !page.loaded && page.error.is_none())
                        {
                            this.tree_users_load(scope.clone(), false, cx);
                        }
                    })),
            )
            .when(open, |row| {
                row.child(
                    Button::new(SharedString::from(refresh.key("users-refresh")))
                        .ghost()
                        .icon(crate::assets::refresh_icon())
                        .disabled(busy)
                        .tooltip(t(cx, "database.users.refresh"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.tree_users_load(refresh.clone(), false, cx)
                        })),
                )
            })
            .into_any_element();
        self.tree_account_menu(menu_scope, None, row, cx)
    }
    pub(super) fn tree_users_page_row(&self, scope: Scope, cx: &mut Context<Self>) -> AnyElement {
        let page = self.tree.pages.get(&page_key(&scope, "user"));
        let busy = page.is_some_and(|page| !page.busy.is_empty());
        let error = page.and_then(|page| page.error.as_ref());
        Button::new(SharedString::from(scope.key("users-page")))
            .ghost()
            .justify_start()
            .pl(px(60.))
            .child(t(
                cx,
                if busy {
                    "ddl.loading"
                } else if error.is_some() {
                    "database.users.retry"
                } else {
                    "browser.more"
                },
            ))
            .child(
                gpui_component::Icon::new(if busy {
                    IconName::LoaderCircle
                } else {
                    IconName::Plus
                })
                .size_4(),
            )
            .tooltip(t(
                cx,
                if error.is_some() {
                    "database.users.failed"
                } else {
                    "database.users.refresh"
                },
            ))
            .disabled(busy)
            .on_click(
                cx.listener(move |this, _, _, cx| this.tree_users_load(scope.clone(), true, cx)),
            )
            .into_any_element()
    }
    pub(super) fn tree_users_load(&mut self, scope: Scope, more: bool, cx: &mut Context<Self>) {
        if !self.tree_scope_current(&scope) {
            return;
        }
        let key = page_key(&scope, "user");
        let page = self.tree.pages.entry(key.clone()).or_default();
        if !page.busy.is_empty() {
            return;
        }
        let append = more && page.loaded;
        let offset = if append { page.objects.len() } else { 0 };
        if offset >= MAX_OBJECTS {
            feedback::error(t(cx, "database.users.limit"), cx);
            return;
        }
        page.busy.insert("accounts".into());
        page.error = None;
        self.tree.revision += 1;
        let task = Uuid::new_v4();
        let started = std::time::Instant::now();
        self.tree_cache_log(task, scope.id, started, None, false, cx);
        let receiver = self.mcp_request(
            scope.id,
            scope.generation,
            scope.database.clone(),
            Action::Accounts { offset, limit: 200 },
            cx,
        );
        cx.spawn(async move |view, cx| {
            let result = receiver
                .recv()
                .await
                .map_err(|_| "mcp.stale".to_owned())
                .and_then(|reply| reply)
                .and_then(|reply| match reply {
                    Reply::CatalogPage { objects, more } => Ok((objects, more)),
                    _ => Err("database.tree.unsupported".into()),
                });
            let _ = view.update(cx, |this, cx| {
                let current = this.tree_scope_current(&scope);
                this.tree_cache_log(
                    task,
                    scope.id,
                    started,
                    if !current {
                        Some("mcp.stale")
                    } else {
                        result.as_ref().err().map(String::as_str)
                    },
                    true,
                    cx,
                );
                if !current {
                    this.tree.pages.remove(&key);
                    return;
                }
                let page = this.tree.pages.entry(key).or_default();
                page.busy.clear();
                match result {
                    Ok((objects, more)) => {
                        if !append {
                            page.objects.clear();
                        }
                        page.objects.extend(objects);
                        page.loaded = true;
                        page.more = more;
                    }
                    Err(_) => page.error = Some("database.users.failed".into()),
                }
                this.tree.revision += 1;
                cx.notify();
            });
        })
        .detach();
    }
}

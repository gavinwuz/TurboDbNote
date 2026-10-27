//! Account actions reuse clipped menus, frozen SQL previews and read-only result grids.
use super::*;
use gpui_component::WindowExt;
use std::{cell::Cell, rc::Rc};
use turbodbn_services::database::workbench::account_admin::{self, Draft};

#[derive(Clone)]
struct Creation {
    scope: Scope,
    draft: Draft,
    mode: String,
}

impl DatabaseView {
    fn account_capability(&self, scope: &Scope, create: bool) -> bool {
        self.tree_scope_current(scope)
            && self.note_profile(scope.id).is_some_and(|profile| {
                accounts::supported(profile)
                    && (!create || profile.supports_mutation())
                    && account_admin::supported(
                        profile.kind,
                        self.info
                            .get(&scope.id)
                            .map(|i| i.version.as_str())
                            .unwrap_or_default(),
                        create,
                    )
            })
    }
    pub(super) fn tree_user_row(
        &self,
        scope: Scope,
        object: Object,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.tree_user_row_at(scope, object, 60., cx)
    }
    pub(super) fn tree_user_row_at(
        &self,
        scope: Scope,
        object: Object,
        indent: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let row = Button::new(SharedString::from(
            scope.key(&format!("account-{}", Self::object_key(&object))),
        ))
        .ghost()
        .w_full()
        .justify_start()
        .pl(px(indent))
        .icon(IconName::User)
        .label(object.name.clone())
        .tooltip(object.name.clone())
        .into_any_element();
        self.tree_account_menu(scope, Some(object), row, cx)
    }
    pub(super) fn tree_account_menu(
        &self,
        scope: Scope,
        object: Option<Object>,
        row: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let owner = cx.entity().downgrade();
        let create = self.account_capability(&scope, true);
        let read = self.account_capability(&scope, false);
        crate::context_menu::scoped(
            SharedString::from(scope.key(&format!(
                "account-menu:{}",
                object.as_ref().map(Self::object_key).unwrap_or_else(||"account-root".into())
            ))),
            row,
            move |mut menu, _, cx| {
                let source = owner.clone();
                let target = scope.clone();
                menu = menu.item(
                    PopupMenuItem::new(t(cx, "accounts.new"))
                        .icon(IconName::Plus)
                        .disabled(!create)
                        .on_click(move |_, window, cx| {
                            let _ = source.update(cx, |this, cx| {
                                this.account_create_dialog(target.clone(), window, cx)
                            });
                        }),
                );
                if let Some(account) = object.clone() {
                    let source = owner.clone();
                    let target = scope.clone();
                    menu = menu.item(
                        PopupMenuItem::new(t(cx, "accounts.permissions"))
                            .icon(IconName::User)
                            .disabled(!read)
                            .on_click(move |_, window, cx| {
                                let _ = source.update(cx, |this, cx| {
                                    if !this.account_capability(&target, false) {
                                        feedback::error(t(cx, "accounts.stale"), cx);
                                        return;
                                    }
                                    let source = cx.entity().downgrade();
                                    let view = cx.new(|cx| {
                                        Privileges::new(
                                            source,
                                            target.clone(),
                                            account.clone(),
                                            window,
                                            cx,
                                        )
                                    });
                                    cx.emit(OpenPrivileges(view));
                                });
                            }),
                    );
                }
                if !read || !create {
                    menu = menu.separator().item(
                        PopupMenuItem::new(t(cx, "accounts.capabilities")).on_click(|_, _, cx| {
                            feedback::error(t(cx, "accounts.unsupported"), cx)
                        }),
                    );
                }
                menu
            },
        )
        .into_any_element()
    }
    fn account_create_dialog(&self, scope: Scope, window: &mut Window, cx: &mut Context<Self>) {
        if !self.account_capability(&scope, true) {
            feedback::error(t(cx, "accounts.unsupported"), cx);
            return;
        }
        let source = cx.entity().downgrade();
        let user = cx.new(|cx| InputState::new(window, cx));
        let host = cx.new(|cx| InputState::new(window, cx).default_value("localhost"));
        let password = cx.new(|cx| InputState::new(window, cx).masked(true));
        let busy = Rc::new(Cell::new(false));
        window.open_dialog(cx, move |dialog, _, cx| {
            let user = user.clone();
            let host = host.clone();
            let password = password.clone();
            let owner = source.clone();
            let target = scope.clone();
            let running = busy.clone();
            dialog
                .title(t(cx, "accounts.new"))
                .w(px(520.))
                .overlay_closable(false)
                .child(
                    div()
                        .v_flex()
                        .gap_3()
                        .child(t(cx, "accounts.user"))
                        .child(Input::new(&user).cleanable(true).disabled(busy.get()))
                        .child(t(cx, "accounts.host"))
                        .child(Input::new(&host).cleanable(true).disabled(busy.get()))
                        .child(t(cx, "accounts.password"))
                        .child(Input::new(&password).cleanable(true).disabled(busy.get()))
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(t(cx, "accounts.create_hint")),
                        )
                        .child(
                            feedback::actions().child(feedback::close_button(cx)).child(
                                Button::new("prepare-account")
                                    .primary()
                                    .icon(IconName::Check)
                                    .label(t(cx, "accounts.preview"))
                                    .disabled(busy.get())
                                    .on_click(move |_, window, cx| {
                                        if running.get() {
                                            return;
                                        }
                                        let draft = Draft {
                                            user: user.read(cx).value().trim().into(),
                                            host: host.read(cx).value().trim().into(),
                                            password: password.read(cx).value().to_string(),
                                        };
                                        if account_admin::create_sql(&draft, "", true).is_err() {
                                            feedback::error(t(cx, "accounts.invalid"), cx);
                                            return;
                                        }
                                        let _ = owner.update(cx, |this, cx| {
                                            this.account_prepare(
                                                target.clone(),
                                                draft,
                                                running.clone(),
                                                password.clone(),
                                                window,
                                                cx,
                                            )
                                        });
                                    }),
                            ),
                        ),
                )
        });
    }
    fn account_prepare(
        &mut self,
        scope: Scope,
        draft: Draft,
        busy: Rc<Cell<bool>>,
        password: Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.account_capability(&scope, true) {
            feedback::error(t(cx, "accounts.stale"), cx);
            return;
        }
        busy.set(true);
        cx.refresh_windows();
        let receiver = self.mcp_request(
            scope.id,
            scope.generation,
            scope.database.clone(),
            Action::AccountMode,
            cx,
        );
        cx.spawn_in(window, async move |view, cx| {
            let result = receiver.recv().await;
            let _ = cx.update(|window, cx| {
                busy.set(false);
                cx.refresh_windows();
                let _ = view.update(cx, |this, cx| {
                    if !this.account_capability(&scope, true) {
                        feedback::error(t(cx, "accounts.stale"), cx);
                        return;
                    }
                    let Ok(Ok(Reply::Page { rows, .. })) = result else {
                        feedback::error(t(cx, "accounts.prepare_failed"), cx);
                        return;
                    };
                    let Some(mode) = rows
                        .values
                        .first()
                        .and_then(|row| row.first())
                        .and_then(Clone::clone)
                    else {
                        feedback::error(t(cx, "accounts.prepare_failed"), cx);
                        return;
                    };
                    let Ok(preview) = account_admin::create_sql(&draft, &mode, true) else {
                        feedback::error(t(cx, "accounts.invalid"), cx);
                        return;
                    };
                    let profile = this.note_profile(scope.id).unwrap();
                    let target = tr(
                        cx,
                        "accounts.confirm_target",
                        &[
                            ("connection", &profile.name),
                            ("environment", &profile.environment),
                            ("account", &format!("{}@{}", draft.user, draft.host)),
                        ],
                    )
                    .to_string();
                    let owner = cx.entity().downgrade();
                    let expected = preview.clone();
                    let plan = Creation { scope, draft, mode };
                    feedback::sql_preview(preview, target, window, cx, move |sql, window, cx| {
                        if sql != expected {
                            feedback::error(t(cx, "accounts.preview_changed"), cx);
                            return false;
                        }
                        owner
                            .update(cx, |this, cx| {
                                this.account_execute(
                                    plan.clone(),
                                    busy.clone(),
                                    password.clone(),
                                    window,
                                    cx,
                                )
                            })
                            .unwrap_or(false)
                    });
                });
            });
        })
        .detach();
    }
    fn account_execute(
        &mut self,
        plan: Creation,
        busy: Rc<Cell<bool>>,
        password: Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Creation { scope, draft, mode } = plan;
        if busy.get() || !self.account_capability(&scope, true) {
            feedback::error(t(cx, "accounts.stale"), cx);
            return false;
        }
        busy.set(true);
        cx.refresh_windows();
        let receiver = self.mcp_request(
            scope.id,
            scope.generation,
            scope.database.clone(),
            Action::AccountCreate {
                draft,
                sql_mode: mode,
                confirmed: true,
            },
            cx,
        );
        cx.spawn_in(window, async move |view, cx| {
            let result = receiver.recv().await;
            let _ = cx.update(|window, cx| {
                busy.set(false);
                cx.refresh_windows();
                let _ = view.update(cx, |this, cx| {
                    if matches!(result, Ok(Ok(Reply::Executed))) {
                        password.update(cx, |input, cx| input.set_value("", window, cx));
                        feedback::success(t(cx, "accounts.created"), cx);
                        if this.tree_scope_current(&scope) {
                            this.tree_users_load(scope.clone(), false, cx);
                        }
                    } else {
                        feedback::error(t(cx, "accounts.create_failed"), cx);
                    }
                });
            });
        })
        .detach();
        true
    }
}

#[path = "account_grants_ui.rs"]
mod grants_ui;
use account_admin::{Grant, GrantPlan};
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct PermissionBookmark {
    pub key: String,
    pub connection: Uuid,
    pub user: String,
    pub host: String,
}
pub(crate) struct OpenPrivileges(pub Entity<Privileges>);
impl EventEmitter<OpenPrivileges> for DatabaseView {}
pub(crate) struct Privileges {
    source: WeakEntity<DatabaseView>,
    scope: Scope,
    account: Object,
    key: String,
    focus: FocusHandle,
    selected_db: String,
    selected_target: (String, String, String, String),
    object_filter: Entity<InputState>,
    only_granted: bool,
    narrow: bool,
    object_names: Vec<Object>,
    object_context: Option<(String, String)>,
    object_task: Option<Task<()>>,
    objects_more: bool,
    directory_scroll: ScrollHandle,
    object_scroll: ScrollHandle,
    filter: Entity<InputState>,
    grants: Vec<Grant>,
    draft: Option<Vec<Grant>>,
    editing_target: Option<(String, String, String, String)>,
    grant_db: Entity<InputState>,
    grant_table: Entity<InputState>,
    grant_column: Entity<InputState>,
    scroll: ScrollHandle,
    busy: bool,
    applying: bool,
    directory_limit: usize,
    loaded: bool,
    error: bool,

    applied: String,
    _input: Vec<Subscription>,
    _source: Option<Subscription>,
}
impl Privileges {
    fn new(
        source: WeakEntity<DatabaseView>,
        scope: Scope,
        account: Object,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t(cx, "accounts.workbench.directory_filter"))
        });
        let input = cx.subscribe_in(&filter, window, |this, _, event, _window, cx| {
            if matches!(event, gpui_component::input::InputEvent::PressEnter { .. }) {
                this.applied = this.filter.read(cx).value().to_string();
                cx.notify();
            }
        });
        let object_filter =
            cx.new(|cx| InputState::new(window, cx).placeholder(t(cx, "accounts.object")));
        let object_input = cx.subscribe(
            &object_filter,
            |_, _, _: &gpui_component::input::InputEvent, cx| cx.notify(),
        );
        let filter_changed = cx.subscribe(
            &filter,
            |this, _, _: &gpui_component::input::InputEvent, cx| {
                this.directory_limit = 200;
                cx.notify();
            },
        );
        let observed = source.upgrade().map(|database| {
            cx.observe(&database, |this, database, cx| {
                if !database.read(cx).tree_scope_current(&this.scope) {
                    this.error = true;
                    cx.notify();
                }
            })
        });
        let key = serde_json::to_string(&(
            "account-permissions",
            scope.id,
            &account.schema,
            &account.parent,
        ))
        .expect("permission identity");
        Self {
            key,
            focus: cx.focus_handle(),
            selected_db: scope.database.clone(),
            selected_target: if scope.database.is_empty() {
                ("global".into(), "*".into(), "*".into(), String::new())
            } else {
                grants_ui::database_target(&scope.database)
            },
            object_filter,
            only_granted: false,
            narrow: false,
            object_names: vec![],
            object_context: None,
            object_task: None,
            objects_more: false,
            directory_scroll: ScrollHandle::new(),
            object_scroll: ScrollHandle::new(),
            source,
            scope,
            account,
            filter,
            grants: vec![],
            draft: None,
            editing_target: None,
            grant_db: cx
                .new(|cx| InputState::new(window, cx).placeholder(t(cx, "accounts.database"))),
            grant_table: cx
                .new(|cx| InputState::new(window, cx).placeholder(t(cx, "accounts.object"))),
            grant_column: cx
                .new(|cx| InputState::new(window, cx).placeholder(t(cx, "accounts.column"))),
            scroll: ScrollHandle::new(),
            busy: false,
            applying: false,
            directory_limit: 200,
            loaded: false,
            error: false,

            applied: String::new(),
            _input: vec![input, object_input, filter_changed],
            _source: observed,
        }
    }
    fn load(&mut self, _offset: usize, paging: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let query = if paging {
            self.applied.clone()
        } else {
            self.filter.read(cx).value().trim().to_owned()
        };
        let receiver = self
            .source
            .update(cx, |source, cx| {
                if !source.account_capability(&self.scope, false) {
                    return None;
                }
                Some(source.mcp_request(
                    self.scope.id,
                    self.scope.generation,
                    self.scope.database.clone(),
                    Action::AccountGrants {
                        user: self.account.schema.clone(),
                        host: self.account.parent.clone(),
                    },
                    cx,
                ))
            })
            .ok()
            .flatten();
        let Some(receiver) = receiver else {
            self.error = true;
            cx.notify();
            return;
        };
        self.busy = true;
        self.error = false;
        cx.notify();
        cx.spawn_in(window, async move |view, cx| {
            let result = receiver.recv().await;
            let _ = cx.update(|_window, cx| {
                let _ = view.update(cx, |this, cx| {
                    this.busy = false;
                    let current = this
                        .source
                        .read_with(cx, |source, _| {
                            source.account_capability(&this.scope, false)
                        })
                        .unwrap_or(false);
                    if !current {
                        this.grants.clear();
                        this.draft = None;
                        this.error = true;
                        cx.notify();
                        return;
                    }
                    match result {
                        Ok(Ok(Reply::AccountGrants(grants))) => {
                            this.loaded = true;
                            this.grants = grants;
                            this.draft = None;

                            this.applied = query;
                            this.object_context = None;
                        }
                        _ => this.error = true,
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }
}
impl Render for Privileges {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.loaded && !self.busy && !self.error {
            self.load(0, false, window, cx);
        }
        self.render_grants(window, cx)
    }
}

impl Privileges {
    pub(crate) fn restored(
        source: WeakEntity<DatabaseView>,
        saved: PermissionBookmark,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let generation = source
            .read_with(cx, |db, _| db.note_generation(saved.connection))
            .unwrap_or_default();
        let mut view = Self::new(
            source,
            Scope {
                id: saved.connection,
                database: String::new(),
                generation,
            },
            Object {
                name: format!("{}@{}", saved.user, saved.host),
                schema: saved.user,
                parent: saved.host,
                kind: "user".into(),
                comment: String::new(),
            },
            window,
            cx,
        );
        view.key = saved.key;
        view.error = true;
        view
    }
}
impl EventEmitter<gpui_component::dock::PanelEvent> for Privileges {}
impl Focusable for Privileges {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Privileges {
    pub(crate) fn tab_identity(&self, cx: &App) -> (String, String, String, bool) {
        let profile = self
            .source
            .upgrade()
            .and_then(|source| source.read(cx).note_profile(self.scope.id).cloned());
        (
            format!("{} · {}", t(cx, "accounts.permissions"), self.account.name),
            profile
                .as_ref()
                .map(|p| p.group.clone())
                .unwrap_or_default(),
            profile
                .map(|p| format!("{} · {}:{}", p.name, p.host, p.port))
                .unwrap_or_default(),
            self.dirty(),
        )
    }
}
impl gpui_component::dock::Panel for Privileges {
    fn panel_name(&self) -> &'static str {
        "account-permissions"
    }
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::tabs::title(
            self.key.clone(),
            IconName::User,
            format!(
                "{} · {}{}",
                t(cx, "accounts.permissions"),
                self.account.name,
                if self.dirty() { " ●" } else { "" }
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
    fn dropdown_menu(
        &mut self,
        menu: gpui_component::menu::PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_component::menu::PopupMenu {
        crate::tabs::opened_notes(menu, &self.key, window, cx)
    }
}

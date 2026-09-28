use super::*;

/// Owns only a temporary test session. Dropping the preview cancels its task and session.
pub(super) struct Discovery {
    profile: Profile,
    user: Entity<InputState>,
    database: Entity<InputState>,
    picker: Entity<SelectState<SearchableVec<DatabaseChoice>>>,
    names: Vec<String>,
    tested: Option<(String, TlsMode)>,
    session: Option<Session>,
    task: Option<Task<()>>,
    generation: u64,
    subscriptions: Vec<Subscription>,
}
impl Discovery {
    pub(super) fn new(
        profile: Profile,
        user: Entity<InputState>,
        database: Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let picker = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(Vec::<DatabaseChoice>::new()),
                None,
                window,
                cx,
            )
            .searchable(true)
        });
        let subscriptions = vec![
            cx.subscribe(&user, |this, _, event, cx| {
                if matches!(event, gpui_component::input::InputEvent::Change) {
                    this.cancel();
                    this.names.clear();
                    this.tested = None;
                    cx.notify();
                }
            }),
            cx.observe(&database, |_, _, cx| cx.notify()),
            cx.subscribe_in(
                &picker,
                window,
                |this, _, event: &SelectEvent<SearchableVec<DatabaseChoice>>, window, cx| {
                    if let SelectEvent::Confirm(Some(name)) = event {
                        this.database
                            .update(cx, |input, cx| input.set_value(name.clone(), window, cx));
                        cx.notify();
                    }
                },
            ),
        ];
        Self {
            profile,
            user,
            database,
            picker,
            names: vec![],
            tested: None,
            session: None,
            task: None,
            generation: 0,
            subscriptions,
        }
    }
    fn cancel(&mut self) {
        self.generation += 1;
        self.task = None;
        self.session = None;
    }
    pub(super) fn apply_tested_tls(&self, profile: &mut Profile) {
        if let Some((user, mode)) = &self.tested
            && user == &profile.user
        {
            profile.tls = *mode;
        }
    }
    fn credentials(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut profile = self.profile.clone();
        profile.user = self.user.read(cx).value().trim().into();
        // Postgres needs an accessible maintenance database; preserve an explicit value.
        profile.database = if profile.kind == DatabaseKind::PostgreSql {
            self.database.read(cx).value().trim().into()
        } else {
            String::new()
        };
        profile.list_all_databases = true;
        if let Err(error) = profile.validate() {
            feedback::error(error, cx);
            return;
        }
        let password = cx.new(|cx| InputState::new(window, cx).masked(true));
        let disabled = Rc::new(std::cell::Cell::new(profile.tls == TlsMode::Disabled));
        let view = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let password = password.clone();
            let disabled_toggle = disabled.clone();
            let disabled = disabled.clone();
            let profile = profile.clone();
            let view = view.clone();
            dialog
                .title(crate::locale::t(cx, "import.detect.credentials"))
                .w(px(480.))
                .overlay_closable(false)
                .child(format!(
                    "{} · {}:{}",
                    profile.name, profile.host, profile.port
                ))
                .child(crate::locale::t(cx, "import.detect.password_hint"))
                .child(Input::new(&password).cleanable(true).mask_toggle())
                .child(
                    Switch::new("detect-disable-tls")
                        .label(crate::locale::t(cx, "import.detect.disable_tls"))
                        .checked(disabled.get())
                        .on_click(move |value, _, cx| {
                            disabled_toggle.set(*value);
                            cx.refresh_windows();
                        }),
                )
                .footer(move |_, _, _, cx| {
                    let profile = profile.clone();
                    let password = password.clone();
                    let disabled = disabled.clone();
                    let view = view.clone();
                    vec![
                        feedback::footer_actions()
                            .child(feedback::close_button(cx))
                            .child(
                                Button::new("detect-import-databases")
                                    .primary()
                                    .icon(IconName::Search)
                                    .label(crate::locale::t(cx, "import.detect.action"))
                                    .on_click(move |_, window, cx| {
                                        let mut p = profile.clone();
                                        let secret = password.read(cx).value().to_string();
                                        let downgrade =
                                            disabled.get() && p.tls != TlsMode::Disabled;
                                        p.tls = if disabled.get() {
                                            TlsMode::Disabled
                                        } else if p.tls == TlsMode::Disabled {
                                            TlsMode::VerifyIdentity
                                        } else {
                                            p.tls
                                        };
                                        window.close_dialog(cx);
                                        if downgrade {
                                            let view = view.clone();
                                            feedback::confirm(
                                                crate::locale::t(cx, "feedback.tls.confirm"),
                                                crate::locale::t(cx, "feedback.tls.warning"),
                                                window,
                                                cx,
                                                move |window, cx| {
                                                    let _ = view.update(cx, |this, cx| {
                                                        this.start(
                                                            p.clone(),
                                                            secret.clone(),
                                                            window,
                                                            cx,
                                                        )
                                                    });
                                                },
                                            );
                                        } else {
                                            let _ = view.update(cx, |this, cx| {
                                                this.start(p, secret, window, cx)
                                            });
                                        }
                                    }),
                            ),
                    ]
                })
        });
    }
    fn start(
        &mut self,
        profile: Profile,
        password: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel();
        self.names.clear();
        self.tested = None;
        let generation = self.generation;
        let original_database = self.database.read(cx).value().to_string();
        let (session, events) = Session::start(profile.clone(), password, true);
        self.session = Some(session);
        self.task = Some(cx.spawn_in(window, async move |view, cx| {
            let result = events.recv().await;
            let _ = cx.update(|window, cx| {
                let _ = view.update(cx, |this, cx| {
                    if this.generation != generation {
                        return;
                    }
                    this.session = None;
                    if this.user.read(cx).value().trim() != profile.user
                        || this.database.read(cx).value().as_str() != original_database
                    {
                        feedback::error(crate::locale::t(cx, "import.detect.changed"), cx);
                        cx.notify();
                        return;
                    }
                    match result {
                        Ok(Event::Tested(info)) => {
                            let mut seen = std::collections::HashSet::new();
                            this.names = info
                                .databases
                                .into_iter()
                                .filter(|name| !name.is_empty() && seen.insert(name.clone()))
                                .collect();
                            this.picker.update(cx, |picker, cx| {
                                picker.set_items(
                                    SearchableVec::new(
                                        this.names
                                            .iter()
                                            .cloned()
                                            .map(DatabaseChoice)
                                            .collect::<Vec<_>>(),
                                    ),
                                    window,
                                    cx,
                                )
                            });
                            this.tested = Some((profile.user.clone(), profile.tls));
                            this.profile.tls = profile.tls;
                            if this.names.len() == 1 {
                                let name = this.names[0].clone();
                                this.database
                                    .update(cx, |input, cx| input.set_value(name, window, cx));
                            }
                            if let Some(error) = info.database_list_error {
                                feedback::error(error, cx);
                            } else if this.names.is_empty() {
                                feedback::error(crate::locale::t(cx, "import.detect.empty"), cx);
                            } else {
                                feedback::success(
                                    crate::locale::tr(
                                        cx,
                                        "import.detect.done",
                                        &[("count", &this.names.len().to_string())],
                                    ),
                                    cx,
                                );
                            }
                            if this.names.len() > 1 {
                                let picker = this.picker.clone();
                                let row = cx.entity().downgrade();
                                window.on_next_frame(move |window, cx| {
                                    let Some(row) = row.upgrade() else {
                                        return;
                                    };
                                    if row.read(cx).generation != generation
                                        || row.read(cx).names.len() < 2
                                    {
                                        return;
                                    }
                                    picker.update(cx, |picker, cx| picker.focus(window, cx));
                                    if let Ok(action) = cx.build_action("ui::SelectDown", None) {
                                        window.dispatch_action(action, cx);
                                    }
                                });
                            }
                        }
                        Ok(Event::Failed(error)) => feedback::error(error, cx),
                        _ => feedback::error(crate::locale::t(cx, "import.detect.failed"), cx),
                    }
                    cx.notify();
                });
            });
        }));
        cx.notify();
    }
}

impl Render for Discovery {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _keep = &self.subscriptions;
        let busy = self.session.is_some();
        div().v_flex().gap_1().child(
            div()
                .h_flex()
                .gap_2()
                .child(
                    Input::new(&self.database).cleanable(true).flex_1().suffix(
                        Select::new(&self.picker)
                            .appearance(false)
                            .placeholder("")
                            .w(px(28.))
                            .menu_width(px(320.))
                            .search_placeholder(crate::locale::t(cx, "dbwork.database.filter"))
                            .disabled(self.names.is_empty() || busy),
                    ),
                )
                .child(
                    Button::new("import-detect")
                        .icon(if busy {
                            IconName::Close
                        } else {
                            IconName::Search
                        })
                        .label(crate::locale::t(
                            cx,
                            if busy {
                                "import.detect.cancel"
                            } else {
                                "import.detect.action"
                            },
                        ))
                        .on_click(cx.listener(|this, _, window, cx| {
                            if this.session.is_some() {
                                this.cancel();
                                cx.notify();
                            } else {
                                this.credentials(window, cx);
                            }
                        })),
                ),
        )
    }
}

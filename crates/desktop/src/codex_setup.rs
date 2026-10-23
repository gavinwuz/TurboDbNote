//! Explicit CLI installation and authentication, independent of inference sessions.
use super::*;
use crate::{
    feedback,
    locale::{t, tr},
    operation_log::{Kind, Phase, Record},
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use turbodbn_services::{codex_install, updates::Progress};

enum InstallEvent {
    Progress(Progress),
    Finished(Result<Candidate, String>),
}
pub(super) struct Setup {
    owner: WeakEntity<ChatView>,
    path: Entity<InputState>,
    log: Entity<AiLog>,
    _path_observer: Subscription,
    cancel: Option<Arc<AtomicBool>>,
    install_task: Option<Task<()>>,
    login: Option<Connection>,
    login_task: Option<Task<()>>,
    revision: u64,
    status: String,
    url: Option<String>,
    code: Option<String>,
    progress: Option<Progress>,
    verified_path: Option<String>,
    installing: bool,
    authenticating: bool,
    task_id: uuid::Uuid,
    started: Instant,
}
impl Drop for Setup {
    fn drop(&mut self) {
        if let Some(flag) = &self.cancel {
            flag.store(true, Ordering::Relaxed);
        }
    }
}
impl Setup {
    pub(super) fn new(
        owner: WeakEntity<ChatView>,
        path: Entity<InputState>,
        log: Entity<AiLog>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut previous = path.read(cx).value();
        Self {
            owner,
            log,
            _path_observer: cx.observe(&path, move |this, path, cx| {
                let value = path.read(cx).value();
                if value == previous {
                    return;
                }
                previous = value.clone();
                if this.authenticating {
                    this.cancel(cx);
                }
                this.url = None;
                this.code = None;
                if !this.installing && this.verified_path.as_deref() != Some(value.as_ref()) {
                    this.status.clear();
                    this.verified_path = None;
                }
                cx.notify();
            }),
            path,
            cancel: None,
            install_task: None,
            login: None,
            login_task: None,
            revision: 0,
            status: String::new(),
            url: None,
            code: None,
            progress: None,
            verified_path: None,
            installing: false,
            authenticating: false,
            task_id: uuid::Uuid::new_v4(),
            started: Instant::now(),
        }
    }
    fn record(&self, kind: Kind, phase: Phase, error: Option<&str>, cx: &mut Context<Self>) {
        let mut record = Record::new(self.task_id, kind, phase);
        record.context = t(
            cx,
            if matches!(kind, Kind::Driver) {
                "agents.setup.install"
            } else {
                "agents.setup.login"
            },
        )
        .to_string();
        record.elapsed_ms = self.started.elapsed().as_millis() as u64;
        if let Some(progress) = self.progress {
            record.bytes = progress.bytes;
        }
        if let Some(error) = error {
            record.failure = crate::operation_log::failure_class(error);
        }
        self.log.update(cx, |log, cx| log.operation(record, cx));
    }
    pub(super) fn cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(flag) = &self.cancel {
            flag.store(true, Ordering::Relaxed);
        }
        if self.authenticating {
            self.record(Kind::Connection, Phase::Cancelled, None, cx);
        }
        self.revision += 1;
        self.login = None;
        self.login_task = None;
        self.authenticating = false;
        self.url = None;
        self.code = None;
        // Keep the download receiver alive until its worker acknowledges cancellation.
        self.status = "agents.setup.cancelled".into();
        cx.notify();
    }
    pub(super) fn busy(&self) -> bool {
        self.installing || self.authenticating
    }
    fn scope_current(&self, before: &str, cx: &App) -> bool {
        self.owner.upgrade().is_some_and(|owner| {
            let owner = owner.read(cx);
            owner.engine == Engine::Codex
                && owner.connection.is_none()
                && owner.path.read(cx).value().as_ref() == before
        })
    }
    fn allowed(&self, cx: &App) -> bool {
        self.owner.upgrade().is_some_and(|owner| {
            let owner = owner.read(cx);
            owner.engine == Engine::Codex
                && owner.connection.is_none()
                && !owner.saving
                && !owner.busy
        })
    }
    fn root() -> Result<PathBuf, String> {
        Preferences::path()
            .ok()
            .and_then(|p| p.parent().map(|p| p.join("agents/codex")))
            .ok_or_else(|| "agents.setup.error.disk".into())
    }
    fn install(&mut self, supplied: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        if self.installing || self.authenticating || !self.allowed(cx) {
            return;
        }
        let root = match Self::root() {
            Ok(root) => root,
            Err(error) => {
                feedback::error(t(cx, &error), cx);
                return;
            }
        };
        let before = self.path.read(cx).value().to_string();
        let _ = self.owner.update(cx, |owner, _| {
            owner.automatic.suppress(Engine::Codex);
            owner.discovery_task = None;
            owner.detecting = false;
        });
        self.installing = true;
        self.status = "agents.setup.downloading".into();
        self.progress = None;
        self.task_id = uuid::Uuid::new_v4();
        self.started = Instant::now();
        self.record(Kind::Driver, Phase::Started, None, cx);
        let flag = Arc::new(AtomicBool::new(false));
        self.cancel = Some(flag.clone());
        let (send, receive) = async_channel::bounded(32);
        let worker = cx.background_executor().spawn(async move {
            let result = codex_install::install(&root, supplied.as_deref(), &flag, |progress| {
                let _ = send.try_send(InstallEvent::Progress(progress));
            });
            if result.is_ok() {
                let _ = std::fs::create_dir_all(root.parent().unwrap().join("workspace"));
            }
            let _ = send.send_blocking(InstallEvent::Finished(result));
        });
        self.install_task = Some(cx.spawn_in(window, async move |view, cx| {
            while let Ok(event) = receive.recv().await {
                let done = matches!(event, InstallEvent::Finished(_));
                let _ = cx.update(|window, cx| {
                    view.update(cx, |this, cx| {
                        match event {
                            InstallEvent::Progress(progress) => {
                                this.progress = Some(progress);
                                this.record(Kind::Driver, Phase::Progress, None, cx);
                            }
                            InstallEvent::Finished(result) => {
                                let cancelled = this
                                    .cancel
                                    .as_ref()
                                    .is_some_and(|f| f.load(Ordering::Relaxed));
                                this.installing = false;
                                this.cancel = None;
                                match result {
                                    Ok(candidate) if !cancelled => {
                                        this.record(Kind::Driver, Phase::Succeeded, None, cx);
                                        let path = candidate.path.display().to_string();
                                        this.verified_path = Some(path.clone());
                                        let bound = this
                                            .owner
                                            .update(cx, |owner, cx| {
                                                if owner.engine != Engine::Codex
                                                    || owner.connection.is_some()
                                                    || owner.path.read(cx).value().as_ref()
                                                        != before
                                                {
                                                    return false;
                                                }
                                                owner.automatic.suppress(Engine::Codex);
                                                owner.path.update(cx, |field, cx| {
                                                    field.set_value(path, window, cx)
                                                });
                                                if owner.cwd.read(cx).value().trim().is_empty()
                                                    && let Ok(root) = Self::root()
                                                {
                                                    let cwd =
                                                        root.parent().unwrap().join("workspace");
                                                    owner.cwd.update(cx, |field, cx| {
                                                        field.set_value(
                                                            cwd.display().to_string(),
                                                            window,
                                                            cx,
                                                        )
                                                    });
                                                }
                                                owner
                                                    .candidates
                                                    .retain(|c| c.path != candidate.path);
                                                owner.candidates.insert(0, candidate);
                                                owner.discovery_status.clear();
                                                cx.notify();
                                                true
                                            })
                                            .unwrap_or(false);
                                        this.status = if bound {
                                            "agents.setup.bound"
                                        } else {
                                            "agents.setup.binding_changed"
                                        }
                                        .into();
                                        feedback::success(t(cx, &this.status), cx);
                                    }
                                    Ok(_) => {
                                        this.status = "agents.setup.cancelled".into();
                                        this.record(Kind::Driver, Phase::Cancelled, None, cx);
                                    }
                                    Err(error) => {
                                        this.status = if cancelled {
                                            "agents.setup.cancelled".into()
                                        } else {
                                            error.clone()
                                        };
                                        this.record(
                                            Kind::Driver,
                                            if cancelled {
                                                Phase::Cancelled
                                            } else {
                                                Phase::Failed
                                            },
                                            Some(&error),
                                            cx,
                                        );
                                        if !cancelled {
                                            feedback::error(crate::locale::message(cx, &error), cx);
                                        }
                                    }
                                }
                            }
                        }
                        cx.notify();
                    })
                });
                if done {
                    break;
                }
            }
            worker.await;
        }));
        cx.notify();
    }
    fn offline(&self, window: &mut Window, cx: &mut Context<Self>) {
        let selected = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t(cx, "agents.setup.offline_hint")),
        });
        cx.spawn_in(window, async move |view, cx| {
            if let Ok(Ok(Some(paths))) = selected.await
                && let Some(path) = paths.first()
            {
                let _ = cx.update(|window, cx| {
                    view.update(cx, |this, cx| this.install(Some(path.clone()), window, cx))
                });
            }
        })
        .detach();
    }
    fn authenticate(&mut self, device: bool, cx: &mut Context<Self>) {
        if self.installing || self.authenticating || !self.allowed(cx) {
            return;
        }
        let _ = self
            .owner
            .update(cx, |owner, _| owner.automatic.suppress(Engine::Codex));
        let path = PathBuf::from(self.path.read(cx).value().as_str());
        let before = path.display().to_string();
        self.revision += 1;
        let revision = self.revision;
        self.authenticating = true;
        self.url = None;
        self.code = None;
        self.progress = None;
        self.status = "agents.setup.checking".into();
        self.task_id = uuid::Uuid::new_v4();
        self.started = Instant::now();
        self.record(Kind::Connection, Phase::Started, None, cx);
        let probe = cx
            .background_executor()
            .spawn(async move { codex_install::verify_executable(&path) });
        self.login_task = Some(cx.spawn(async move |view, cx| {
            let result = probe.await;
            let mut receiver = None;
            let _ = view.update(cx, |this, cx| {
                if this.revision != revision || this.path.read(cx).value().as_ref() != before {
                    return;
                }
                if !this.scope_current(&before, cx) {
                    this.authenticating = false;
                    this.login = None;
                    cx.notify();
                    return;
                }
                match result {
                    Ok(candidate) => {
                        this.verified_path = Some(before.clone());
                        let (connection, events) =
                            Connection::authenticate(candidate.path, std::env::temp_dir(), device);
                        this.login = Some(connection);
                        receiver = Some(events);
                        this.status = "agents.setup.login_wait".into();
                    }
                    Err(error) => {
                        this.authenticating = false;
                        this.status = error.clone();
                        this.record(Kind::Connection, Phase::Failed, Some(&error), cx);
                        feedback::error(crate::locale::message(cx, &error), cx);
                    }
                }
                cx.notify();
            });
            let Some(events) = receiver else {
                return;
            };
            while let Ok(event) = events.recv().await {
                let mut finished = false;
                let _ = view.update(cx, |this, cx| {
                    if this.revision != revision {
                        finished = true;
                        return;
                    }
                    if !this.scope_current(&before, cx) {
                        this.login = None;
                        this.authenticating = false;
                        this.url = None;
                        this.code = None;
                        this.status = "agents.setup.cancelled".into();
                        this.record(Kind::Connection, Phase::Cancelled, None, cx);
                        finished = true;
                        cx.notify();
                        return;
                    }
                    match event {
                        Event::Login { url, code } => {
                            this.url = Some(url.clone());
                            this.code = code;
                            this.status = "agents.setup.login_wait".into();
                            this.record(Kind::Connection, Phase::Prepared, None, cx);
                            if !device {
                                cx.open_url(&url);
                            }
                        }
                        Event::LoginFinished { ready: true } => {
                            this.status = "agents.setup.authenticated".into();
                            this.record(Kind::Connection, Phase::Succeeded, None, cx);
                            feedback::success(t(cx, &this.status), cx);
                            finished = true;
                        }
                        Event::Error(_)
                        | Event::Disconnected
                        | Event::LoginFinished { ready: false } => {
                            this.status = "agents.setup.error.login".into();
                            this.record(Kind::Connection, Phase::Failed, Some(&this.status), cx);
                            feedback::error(t(cx, &this.status), cx);
                            finished = true;
                        }
                        _ => {}
                    }
                    if finished {
                        this.login = None;
                        this.authenticating = false;
                        this.url = None;
                        this.code = None;
                    }
                    cx.notify();
                });
                if finished {
                    break;
                }
            }
        }));
        cx.notify();
    }
}
impl Render for Setup {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.installing || self.authenticating;
        let allowed = self.allowed(cx);
        let bound = !self.path.read(cx).value().trim().is_empty();
        let (detecting, found) = self
            .owner
            .upgrade()
            .map(|owner| {
                let owner = owner.read(cx);
                (owner.detecting, !owner.candidates.is_empty())
            })
            .unwrap_or_default();
        let message = if self.status.is_empty() {
            if bound {
                "agents.setup.selected"
            } else if detecting {
                "agents.discovery.scanning"
            } else if found {
                "agents.setup.unbound"
            } else {
                "agents.setup.missing"
            }
        } else {
            &self.status
        };
        div()
            .v_flex()
            .gap_2()
            .p_3()
            .border_1()
            .border_color(cx.theme().border)
            .rounded_md()
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(IconName::Bot)
                    .child(crate::locale::message(cx, message)),
            )
            .child(
                div()
                    .h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new("install-codex")
                            .icon(IconName::ArrowDown)
                            .label(t(cx, "agents.setup.install"))
                            .disabled(busy || !allowed)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.install(None, window, cx)),
                            ),
                    )
                    .child(
                        Button::new("codex-setup-more")
                            .ghost()
                            .icon(IconName::Ellipsis)
                            .label(t(cx, "grid.toolbar.more"))
                            .disabled(busy || !allowed)
                            .dropdown_menu({
                                let owner = cx.entity().downgrade();
                                move |menu, _, cx| {
                                    let offline = owner.clone();
                                    menu.item(
                                        PopupMenuItem::new(t(cx, "agents.setup.guide"))
                                            .on_click(|_, _, cx| cx.open_url(codex_install::GUIDE)),
                                    )
                                    .item(
                                        PopupMenuItem::new(t(cx, "agents.setup.offline")).on_click(
                                            move |_, window, cx| {
                                                let _ = offline.update(cx, |this, cx| {
                                                    this.offline(window, cx)
                                                });
                                            },
                                        ),
                                    )
                                }
                            }),
                    )
                    .when(bound, |el| {
                        el.child(
                            Button::new("codex-login")
                                .icon(IconName::CircleUser)
                                .label(t(cx, "agents.setup.login"))
                                .disabled(busy || !allowed)
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.authenticate(false, cx)),
                                ),
                        )
                        .child(
                            Button::new("codex-device-login")
                                .ghost()
                                .icon(IconName::SquareTerminal)
                                .label(t(cx, "agents.setup.device"))
                                .disabled(busy || !allowed)
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.authenticate(true, cx)),
                                ),
                        )
                    })
                    .when(busy, |el| {
                        el.child(
                            Button::new("cancel-codex-setup")
                                .icon(IconName::Close)
                                .label(t(cx, "components.cancel"))
                                .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
                        )
                    }),
            )
            .when_some(self.progress, |el, p| {
                el.child(div().text_sm().child(tr(
                    cx,
                    if p.verifying {
                        "agents.setup.verifying"
                    } else {
                        "agents.setup.progress"
                    },
                    &[
                        ("done", &(p.bytes / 1024 / 1024).to_string()),
                        ("total", &(p.total / 1024 / 1024).to_string()),
                    ],
                )))
            })
            .when_some(self.url.clone(), |el, url| {
                el.child(
                    Button::new("open-codex-login")
                        .icon(IconName::ExternalLink)
                        .label(t(cx, "agents.setup.open_login"))
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                )
            })
            .when_some(self.code.clone(), |el, code| {
                el.child(
                    div()
                        .h_flex()
                        .gap_2()
                        .child(t(cx, "agents.setup.code"))
                        .child(code),
                )
            })
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(tr(
                        cx,
                        "agents.setup.version",
                        &[("version", codex_install::VERSION)],
                    )),
            )
    }
}

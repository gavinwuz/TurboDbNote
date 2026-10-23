//! One bounded, disposable read-only metadata channel for the selected browser.
use super::*;
use crate::locale::t;
use crate::object_browser::Browser;
use std::time::{Duration, Instant};

impl DatabaseView {
    pub(super) fn close_metadata(&mut self) {
        self.browser.session = None;
        self.browser.worker = None;
        self.browser.deadline = None;
        self.browser.idle = None;
        self.browser.ready = false;
        self.browser.sent = false;
    }

    pub(super) fn cancel_stale_metadata(&mut self, query: &str, cx: &mut Context<Self>) {
        if matches!(&self.browser.pending, Some(Action::BrowseCatalog {search, ..}) if search != query)
            && self.browser.session.is_some()
        {
            self.metadata_log(crate::operation_log::Phase::Cancelled, None, cx);
            self.close_metadata();
            self.browser.pending = None;
            self.browser.revision += 1;
        }
    }

    pub(super) fn prioritize_metadata_columns(&mut self, cx: &mut Context<Self>) {
        if matches!(&self.browser.pending, Some(Action::BrowseCatalog { .. }))
            && self.browser.session.is_some()
        {
            let pending = self.browser.pending.take();
            self.metadata_log(crate::operation_log::Phase::Cancelled, None, cx);
            self.close_metadata();
            if let Some(action) = pending {
                self.browser.push(action);
            }
        }
    }

    fn metadata_log(
        &self,
        phase: crate::operation_log::Phase,
        error: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let Some((id, _)) = self.selected_database.as_ref() else {
            return;
        };
        let mut record = crate::operation_log::Record::new(
            self.browser.task,
            crate::operation_log::Kind::Metadata,
            phase,
        );
        record.source = Some(*id);
        record.elapsed_ms = self
            .browser
            .started
            .map_or(0, |at| at.elapsed().as_millis() as u64);
        record.queued_ms = Some(self.browser.queued_ms);
        record.execution_ms = self.browser.sent.then_some(record.elapsed_ms);
        record.context = t(
            cx,
            if self.browser.sent {
                "browser.execution"
            } else {
                "browser.waiting"
            },
        )
        .to_string();
        if let Some(error) = error {
            record.failure = crate::operation_log::failure_class(error);
        }
        if let Some(log) = &self.output {
            log.update(cx, |log, cx| log.operation(record, cx));
        }
    }

    pub(super) fn metadata_failed(&mut self, error: &str, cx: &mut Context<Self>) {
        if self.browser.session.is_none()
            && self.browser.pending.is_none()
            && self.browser.queue.is_empty()
        {
            return;
        }
        self.metadata_log(crate::operation_log::Phase::Failed, Some(error), cx);
        self.close_metadata();
        // A failed channel must terminate every spinner, without automatic reconnect loops.
        for action in self
            .browser
            .pending
            .take()
            .into_iter()
            .chain(self.browser.queue.drain(..))
        {
            self.browser
                .errors
                .insert(Browser::token(&action), error.into());
        }
        self.browser.queued.clear();
        self.browser.revision += 1;
        cx.notify();
    }

    pub(super) fn pump_metadata(&mut self, cx: &mut Context<Self>) {
        if self.browser.pending.is_some() || !self.browser.supported {
            return;
        }
        let Some((id, db)) = self.selected_database.clone() else {
            return;
        };
        if self.states.get(&id).is_none_or(|s| s != "已连接") {
            self.metadata_failed("browser.connection_failed", cx);
            return;
        }
        while let Some(action) = self.browser.queue.pop_front() {
            if matches!(&action, Action::BrowseCatalog {search, ..} if *search != self.browser.query)
            {
                self.browser.queued.remove(&Browser::token(&action));
                continue;
            }
            self.browser.idle = None;
            self.browser.started = Some(
                self.browser
                    .queued
                    .remove(&Browser::token(&action))
                    .unwrap_or_else(Instant::now),
            );
            self.browser.task = Uuid::new_v4();
            self.browser.queued_ms = self
                .browser
                .started
                .map_or(0, |at| at.elapsed().as_millis() as u64);
            self.metadata_log(crate::operation_log::Phase::Started, None, cx);
            self.browser.pending = Some(action);
            self.browser.revision += 1;
            if self.browser.session.is_some() {
                self.send_metadata(cx);
                return;
            }
            let Some(mut profile) = self
                .connected_profiles
                .get(&id)
                .map(Profile::resolve_timeouts)
            else {
                self.metadata_failed("browser.connection_failed", cx);
                return;
            };
            let password = self
                .temporary_passwords
                .get(&id)
                .and_then(|p| p.value_for(&profile));
            let Some(password) = password.or_else(|| profile.is_file_database().then(String::new))
            else {
                self.metadata_failed("browser.credentials", cx);
                return;
            };
            // Preserve the successful connection's TLS/driver options, not an editor draft.
            if profile.kind == DatabaseKind::PostgreSql {
                profile.database.clone_from(&db);
            }
            self.request_sequence += 1;
            let channel = self.request_sequence;
            let generation = self.note_generation(id);
            self.browser.channel = channel;
            if profile.kind == DatabaseKind::Sqlite {
                profile.sqlite_read_only = true;
            }
            let connect_timeout =
                Duration::from_secs(profile.resolve_timeouts().timeout_seconds.clamp(1, 60) + 2);
            self.browser.execution_timeout = profile.execution_timeout();
            let (session, events) = Session::start(profile, password, false);
            self.browser.session = Some(session);
            self.browser.worker = Some(cx.spawn(async move |view, cx| {
                while let Ok(event) = events.recv().await {
                    let live = view
                        .update(cx, |this, cx| {
                            if this.browser.channel != channel
                                || this.browser.session.is_none()
                                || this.note_generation(id) != generation
                                || this.selected_database != Some((id, db.clone()))
                            {
                                return false;
                            }
                            match event {
                                Event::Connected(_) => {
                                    this.browser.ready = true;
                                    this.send_metadata(cx);
                                }
                                Event::Operation {
                                    id: request,
                                    result,
                                } if request == channel => {
                                    if result.as_ref().err().is_some_and(|e| {
                                        e.contains("超时")
                                            || e.to_ascii_lowercase().contains("timeout")
                                    }) {
                                        this.metadata_failed("browser.timeout", cx);
                                        return false;
                                    }
                                    this.metadata_log(
                                        if result.is_ok() {
                                            crate::operation_log::Phase::Succeeded
                                        } else {
                                            crate::operation_log::Phase::Failed
                                        },
                                        result.as_ref().err().map(String::as_str),
                                        cx,
                                    );
                                    this.browser.deadline = None;
                                    this.browser.sent = false;
                                    if let Some(action) = this.browser.pending.clone() {
                                        let kind = OperationKind::Workbench(Box::new(
                                            workbench::Request {
                                                database: db.clone(),
                                                action,
                                            },
                                        ));
                                        this.receive_browser(id, &kind, &result, cx);
                                    }
                                    this.arm_metadata_idle(cx);
                                    this.pump_browser(cx);
                                }
                                Event::Failed(_) | Event::Disconnected => {
                                    this.metadata_failed("browser.connection_failed", cx);
                                    return false;
                                }
                                _ => {}
                            }
                            cx.notify();
                            true
                        })
                        .unwrap_or(false);
                    if !live {
                        return;
                    }
                }
                let _ = view.update(cx, |this, cx| {
                    if this.browser.channel == channel && this.browser.session.is_some() {
                        this.metadata_failed("browser.connection_failed", cx);
                    }
                });
            }));
            self.arm_metadata_deadline(connect_timeout, cx);
            return;
        }
    }

    fn send_metadata(&mut self, cx: &mut Context<Self>) {
        if !self.browser.ready || self.browser.sent {
            return;
        }
        let (Some(session), Some(action), Some((_, db))) = (
            &self.browser.session,
            &self.browser.pending,
            &self.selected_database,
        ) else {
            return;
        };
        let result = session.request(Operation {
            id: self.browser.channel,
            kind: OperationKind::Workbench(Box::new(workbench::Request {
                database: db.clone(),
                action: action.clone(),
            })),
        });
        if result.is_err() {
            self.metadata_failed("browser.connection_failed", cx);
            return;
        }
        self.browser.queued_ms = self
            .browser
            .started
            .map_or(0, |at| at.elapsed().as_millis() as u64);
        self.metadata_log(crate::operation_log::Phase::Prepared, None, cx);
        self.browser.started = Some(Instant::now());
        self.browser.sent = true;
        let broad = matches!(&self.browser.pending, Some(Action::BrowseCatalog {category, include_comments, ..}) if *include_comments || category == "index");
        let budget = if broad {
            self.browser.execution_timeout
        } else {
            self.browser.execution_timeout.min(Duration::from_secs(15))
        };
        self.arm_metadata_deadline(budget, cx);
    }

    fn arm_metadata_deadline(&mut self, timeout: Duration, cx: &mut Context<Self>) {
        let channel = self.browser.channel;
        self.browser.deadline = Some(cx.spawn(async move |view, cx| {
            gpui::Timer::after(timeout).await;
            let _ = view.update(cx, |this, cx| {
                if this.browser.channel == channel && this.browser.pending.is_some() {
                    this.metadata_failed("browser.timeout", cx);
                }
            });
        }));
    }

    fn arm_metadata_idle(&mut self, cx: &mut Context<Self>) {
        let channel = self.browser.channel;
        self.browser.idle = Some(cx.spawn(async move |view, cx| {
            gpui::Timer::after(Duration::from_secs(30)).await;
            let _ = view.update(cx, |this, _| {
                if this.browser.channel == channel && !this.browser.sent {
                    this.close_metadata();
                }
            });
        }));
    }
}

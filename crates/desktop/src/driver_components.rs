use crate::{
    chat::AiLog,
    feedback,
    locale::{t, tr},
    preferences::Preferences,
};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Disableable, IconName, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    menu::{DropdownMenu, PopupMenuItem},
    switch::Switch,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};
use turbodbn_services::{DatabaseKind, driver_catalog, driver_packages as packages};

#[derive(Default, Clone, Serialize, Deserialize)]
struct Policy {
    disabled: Option<Vec<String>>,
}
pub struct DriverComponents {
    pub output: Option<Entity<AiLog>>,
    root: PathBuf,
    policy_file: PathBuf,
    policy: Policy,
    probes: HashMap<String, String>,
    group: &'static str,
    show_unavailable: bool,
    expanded: std::collections::HashSet<String>,
    notice_expanded: bool,
    loading: bool,
    busy: bool,
    installing: bool,
    writable: bool,
    notice: SharedString,
    task: Option<Task<()>>,
    save_task: Option<Task<()>>,
    cancel: Arc<AtomicBool>,
    epoch: u64,
}
impl DriverComponents {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let file = Preferences::path()
            .unwrap_or_else(|_| PathBuf::from("connections.db"))
            .with_file_name("driver-components.json");
        let root = file.with_file_name("drivers").join("components");
        let loaded = crate::app_store::load::<Policy>(&file, "driver-components", 16384, |p| {
            p.disabled.as_ref().is_none_or(|v| {
                v.len() <= driver_catalog::DRIVERS.len()
                    && v.iter()
                        .all(|id| driver_catalog::DRIVERS.iter().any(|d| d.id == id))
            })
        });
        let writable = loaded.is_ok();
        let mut policy = loaded.unwrap_or_default();
        let initial = policy.disabled.is_none();
        let bundled_host =
            turbodbn_services::external_driver::bundled_directory().is_some_and(|p| {
                p.join(turbodbn_services::external_driver::executable_name())
                    .is_file()
            }) || root.join("active.json").is_file();
        if initial {
            policy.disabled = Some(
                driver_catalog::DRIVERS
                    .iter()
                    .filter(|d| !writable || d.kind == DatabaseKind::External && !bundled_host)
                    .map(|d| d.id.to_string())
                    .collect(),
            );
        }
        packages::configure(root.clone(), policy.disabled.as_deref().unwrap_or_default());
        let mut view = Self {
            output: None,
            root,
            policy_file: file,
            policy,
            probes: HashMap::new(),
            group: "all",
            show_unavailable: false,
            expanded: Default::default(),
            notice_expanded: false,
            loading: false,
            busy: false,
            installing: false,
            writable,
            notice: if writable {
                "".into()
            } else {
                t(cx, "components.error.state")
            },
            task: None,
            save_task: None,
            cancel: Arc::new(AtomicBool::new(false)),
            epoch: 0,
        };
        if initial && writable {
            view.persist(view.policy.clone(), false, cx);
        }
        view
    }
    fn record(
        &self,
        job: uuid::Uuid,
        phase: crate::operation_log::Phase,
        elapsed: u64,
        bytes: u64,
        error: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        if let Some(output) = &self.output {
            let mut r =
                crate::operation_log::Record::new(job, crate::operation_log::Kind::Driver, phase);
            r.elapsed_ms = elapsed;
            r.bytes = bytes;
            if let Some(e) = error {
                r.failure = crate::operation_log::failure_class(e);
            }
            output.update(cx, |output, cx| output.operation(r, cx));
        }
    }
    fn persist(&mut self, policy: Policy, announce: bool, cx: &mut Context<Self>) {
        if !self.writable || self.busy {
            return;
        }
        self.busy = true;
        let file = self.policy_file.clone();
        let saved = policy.clone();
        self.save_task = Some(cx.spawn(async move |owner, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(600))
                .await;
            let result =
                cx.background_executor()
                    .spawn(async move {
                        crate::app_store::save(&file, "driver-components", &saved, 16384)
                    })
                    .await;
            let _ = owner.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(()) => {
                        this.policy = policy;
                        packages::configure(
                            this.root.clone(),
                            this.policy.disabled.as_deref().unwrap_or_default(),
                        );
                        if announce {
                            feedback::success(t(cx, "components.policy_saved"), cx);
                        }
                    }
                    Err(_) => {
                        this.notice = t(cx, "components.error.state");
                        feedback::error(this.notice.clone(), cx);
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn toggle(&mut self, id: &'static str, enabled: bool, cx: &mut Context<Self>) {
        let mut policy = self.policy.clone();
        let disabled = policy.disabled.get_or_insert_with(Vec::new);
        disabled.retain(|x| x != id);
        if !enabled {
            disabled.push(id.to_string());
        }
        self.persist(policy, true, cx);
    }
    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.loading && self.task.is_some() {
            return;
        }
        self.epoch += 1;
        let epoch = self.epoch;
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        let flag = self.cancel.clone();
        self.loading = true;
        let job = uuid::Uuid::new_v4();
        let start = Instant::now();
        self.record(job, crate::operation_log::Phase::Started, 0, 0, None, cx);
        let work = cx.background_executor().spawn(async move {
            let dir = packages::runtime_directory()?;
            let mut states = HashMap::new();
            for d in driver_catalog::DRIVERS
                .iter()
                .filter(|d| d.kind == DatabaseKind::External)
            {
                let status = if let Some(dir) = &dir {
                    match packages::probe(dir, d.id, &flag) {
                        Ok(probe) if probe.ready => "components.ready".into(),
                        Ok(probe) => probe.reason,
                        Err(e) => e,
                    }
                } else {
                    "drivers.runtime_missing".into()
                };
                states.insert(d.id.to_string(), status);
            }
            Ok::<_, String>(states)
        });
        self.task = Some(cx.spawn(async move |owner, cx| {
            let result = work.await;
            let _ = owner.update(cx, |this, cx| {
                if this.epoch != epoch {
                    return;
                }
                this.loading = false;
                match result {
                    Ok(states) => {
                        this.probes = states;
                        if this.writable && !this.installing {
                            this.notice = "".into();
                            this.notice_expanded = false;
                        }
                        this.record(
                            job,
                            crate::operation_log::Phase::Succeeded,
                            start.elapsed().as_millis() as u64,
                            0,
                            None,
                            cx,
                        );
                    }
                    Err(e) => {
                        this.probes = driver_catalog::DRIVERS
                            .iter()
                            .filter(|d| d.kind == DatabaseKind::External)
                            .map(|d| (d.id.to_owned(), "components.unavailable".into()))
                            .collect();
                        this.notice = crate::locale::message(cx, &e);
                        this.record(
                            job,
                            crate::operation_log::Phase::Failed,
                            start.elapsed().as_millis() as u64,
                            0,
                            Some(&e),
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.loading {
            return;
        }
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t(cx, "components.import")),
        });
        cx.spawn_in(window, async move |owner, cx| {
            if let Ok(Ok(Some(paths))) = picker.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = owner.update_in(cx, |_, window, cx| {
                    let weak = owner.clone();
                    feedback::confirm(
                        t(cx, "components.import"),
                        t(cx, "components.import_warning"),
                        window,
                        cx,
                        move |_, cx| {
                            let _ =
                                weak.update(cx, |this, cx| this.install(Some(path.clone()), cx));
                        },
                    );
                });
            }
        })
        .detach();
    }
    fn install(&mut self, archive: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.busy || self.loading {
            return;
        }
        self.busy = true;
        self.installing = true;
        self.epoch += 1;
        let epoch = self.epoch;
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        let flag = self.cancel.clone();
        let root = self.root.clone();
        let job = uuid::Uuid::new_v4();
        let start = Instant::now();
        self.record(job, crate::operation_log::Phase::Started, 0, 0, None, cx);
        self.notice = t(cx, "components.installing");
        let (send, recv) = async_channel::bounded(8);
        let progress_send = send.clone();
        let work = cx.background_executor().spawn(async move {
            let archive = if let Some(path) = archive {
                path
            } else {
                turbodbn_services::updates::download_components(
                    env!("CARGO_PKG_VERSION"),
                    &root.join("downloads"),
                    &flag,
                    |p| {
                        let _ = progress_send.try_send(p.bytes);
                    },
                )?
            };
            packages::install(
                &root,
                &archive,
                env!("CARGO_PKG_VERSION"),
                packages::target(),
                &flag,
                |bytes| {
                    let _ = progress_send.try_send(bytes);
                },
            )
        });
        let weak = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            while let Ok(bytes) = recv.recv().await {
                let _ = weak.update(cx, |this, cx| {
                    if this.epoch == epoch {
                        this.notice = tr(
                            cx,
                            "components.progress",
                            &[("size", &format!("{:.1}", bytes as f64 / 1048576.))],
                        );
                        this.record(
                            job,
                            crate::operation_log::Phase::Progress,
                            start.elapsed().as_millis() as u64,
                            bytes,
                            None,
                            cx,
                        );
                        cx.notify();
                    }
                });
            }
        })
        .detach();
        drop(send);
        self.task = Some(cx.spawn(async move |owner, cx| {
            let result = work.await;
            let _ = owner.update(cx, |this, cx| {
                if this.epoch != epoch {
                    return;
                }
                this.busy = false;
                this.installing = false;
                match result {
                    Ok(_) => {
                        this.notice = t(cx, "components.installed");
                        this.record(
                            job,
                            crate::operation_log::Phase::Succeeded,
                            start.elapsed().as_millis() as u64,
                            0,
                            None,
                            cx,
                        );
                        feedback::success(this.notice.clone(), cx);
                        this.notice = "".into();
                        this.refresh(cx);
                    }
                    Err(e) => {
                        this.notice = crate::locale::message(cx, &e);
                        this.record(
                            job,
                            if e == "components.cancelled" || e == "updates.status.cancelled" {
                                crate::operation_log::Phase::Cancelled
                            } else {
                                crate::operation_log::Phase::Failed
                            },
                            start.elapsed().as_millis() as u64,
                            0,
                            Some(&e),
                            cx,
                        );
                        feedback::error(this.notice.clone(), cx);
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
}
impl Drop for DriverComponents {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
impl DriverComponents {
    fn status(&self, driver: &driver_catalog::DriverSpec) -> &str {
        if driver.kind != DatabaseKind::External {
            "settings2.builtin"
        } else {
            self.probes
                .get(driver.id)
                .map(String::as_str)
                .unwrap_or("components.detecting")
        }
    }
    fn ready(&self, driver: &driver_catalog::DriverSpec) -> bool {
        driver.kind != DatabaseKind::External || self.status(driver) == "components.ready"
    }
    fn detail(&self, driver: &driver_catalog::DriverSpec, cx: &App) -> SharedString {
        let status = self.status(driver);
        if status == "drivers.odbc_missing" {
            t(
                cx,
                if driver.id == "access" {
                    "components.access_missing"
                } else {
                    "components.db2_missing"
                },
            )
        } else {
            crate::locale::message(cx, status)
        }
    }
    fn driver_row(
        &self,
        driver: &'static driver_catalog::DriverSpec,
        cx: &Context<Self>,
    ) -> AnyElement {
        let id = driver.id;
        let ready = self.ready(driver);
        let detail = self.detail(driver, cx);
        let expanded = self.expanded.contains(id);
        let key = if driver.kind != DatabaseKind::External {
            "components.builtin_short"
        } else if ready {
            "components.ready_short"
        } else if self.status(driver) == "components.detecting" {
            "components.detecting"
        } else {
            "components.unavailable_short"
        };
        let row=div().v_flex().w_full().min_w_0().border_b_1().border_color(cx.theme().border)
            .child(div().h_flex().w_full().min_w_0().items_center().gap_3().px_3().py_2()
                .child(div().w(px(170.)).min_w(px(170.)).max_w(px(170.)).flex_shrink_0().overflow_hidden().text_ellipsis().child(driver.name))
                .child(div().flex_1().min_w_0().h_flex().justify_start().child(Button::new(SharedString::from(format!("component-details-{id}"))).ghost().small()
                    .icon(if expanded{IconName::ChevronDown}else{IconName::ChevronRight}).label(t(cx,key)).tooltip(detail.clone())
                    .on_click(cx.listener(move |this,_,_,cx|{if !this.expanded.remove(id){this.expanded.insert(id.into());}cx.notify();}))))
                .child(Switch::new(SharedString::from(format!("component-enable-{id}"))).checked(ready && packages::enabled(id))
                    .tooltip(t(cx,if ready && packages::enabled(id){"components.turn_off"}else{"components.turn_on"}))
                    .disabled(!ready||self.busy||self.loading||!self.writable).on_click(cx.listener(move |this,value,_,cx|this.toggle(id,*value,cx)))))
            .when(expanded,|row|{
                let mut detail_row=div().v_flex().w_full().min_w_0().gap_2().pl(px(182.)).pr_3().pb_3().text_sm().text_color(cx.theme().muted_foreground).whitespace_normal().child(detail.clone());
                if let Some(url)=match id {
                    "oracle"=>Some("https://www.oracle.com/database/technologies/instant-client/downloads.html"),
                    "access"=>Some("https://support.microsoft.com/en-us/access/download-and-install-microsoft-365-access-runtime"),
                    "db2"=>Some("https://www.ibm.com/support/pages/db2-odbc-cli-driver-download-and-installation-information"),_=>None,
                } {
                    detail_row=detail_row.child(Button::new(SharedString::from(format!("component-prerequisite-{id}"))).ghost().small().icon(IconName::ExternalLink)
                        .label(t(cx,"components.prerequisites")).on_click(move |_,_,cx|cx.open_url(url)));
                }
                if id == "db2" {
                    detail_row = detail_row.child(Button::new("db2-offline-client").ghost().small().icon(IconName::FolderOpen)
                        .label(t(cx,"odbc.db2_offline")).tooltip(t(cx,"odbc.db2_offline_tip"))
                        .on_click(|_,_,cx| { cx.spawn(async move |cx| {
                            let result = cx.background_executor().spawn(async { packages::runtime_directory().ok().flatten().map(|p|p.join("db2-offline")) }).await;
                            let _ = cx.update(|cx| {
                                if let Some(path) = result.filter(|p|p.join(turbodbn_services::db2_bundle::ARCHIVE).is_file()) { cx.reveal_path(&path.join("README.txt")); }
                                else { feedback::notify(gpui_component::notification::NotificationType::Warning,t(cx,"odbc.db2_offline_missing"),cx); }
                            });
                        }).detach(); }));
                }
                if self.status(driver).starts_with("components.mismatch:") {
                    detail_row=detail_row.child(Button::new(SharedString::from(format!("component-fix-{id}"))).ghost().small().icon(IconName::FolderOpen)
                        .label(t(cx,"components.import_matching")).disabled(self.busy||self.loading).on_click(cx.listener(|this,_,window,cx|this.import(window,cx))));
                }
                row.child(detail_row)
            });
        row.into_any_element()
    }
}
impl Render for DriverComponents {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.probes.is_empty() && !self.loading && self.task.is_none() {
            let weak = cx.entity().downgrade();
            cx.defer(move |cx| {
                let _ = weak.update(cx, |this, cx| this.refresh(cx));
            });
        }
        let mut body = div().v_flex().w_full().max_w(px(920.)).gap_2().child(
            div()
                .h_flex()
                .gap_2()
                .child(
                    Button::new("component-group")
                        .ghost()
                        .label(t(
                            cx,
                            driver_catalog::GROUPS
                                .iter()
                                .find(|(id, _)| *id == self.group)
                                .map(|(_, key)| *key)
                                .unwrap_or("drivers.group.all"),
                        ))
                        .dropdown_caret(true)
                        .dropdown_menu({
                            let weak = cx.entity().downgrade();
                            move |mut menu, _, cx| {
                                for &(id, key) in driver_catalog::GROUPS {
                                    let weak = weak.clone();
                                    menu = menu.item(PopupMenuItem::new(t(cx, key)).on_click(
                                        move |_, _, cx| {
                                            let _ = weak.update(cx, |this, cx| {
                                                this.group = id;
                                                cx.notify();
                                            });
                                        },
                                    ));
                                }
                                menu
                            }
                        }),
                )
                .child(div().flex_1())
                .child(
                    Button::new("component-download")
                        .icon(IconName::ArrowDown)
                        .label(t(cx, "components.download"))
                        .disabled(self.busy || self.loading)
                        .on_click(cx.listener(|this, _, _, cx| this.install(None, cx))),
                )
                .child(
                    Button::new("component-import")
                        .icon(IconName::FolderOpen)
                        .label(t(cx, "components.import_matching"))
                        .disabled(self.busy || self.loading)
                        .on_click(cx.listener(|this, _, window, cx| this.import(window, cx))),
                )
                .child(
                    Button::new("component-refresh")
                        .icon(crate::assets::refresh_icon())
                        .tooltip(t(cx, "components.detect"))
                        .disabled(self.busy || self.loading)
                        .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                ),
        );
        if self.loading {
            body = body.child(t(cx, "components.detecting"));
        }
        if !self.notice.is_empty() {
            body = body.child(
                Button::new("component-notice")
                    .ghost()
                    .small()
                    .icon(IconName::Info)
                    .label(t(
                        cx,
                        if self.installing {
                            "components.installing"
                        } else {
                            "components.notice"
                        },
                    ))
                    .tooltip(self.notice.clone())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.notice_expanded = !this.notice_expanded;
                        cx.notify();
                    })),
            );
            if self.notice_expanded || self.installing {
                body = body.child(
                    div()
                        .text_sm()
                        .whitespace_normal()
                        .child(self.notice.clone()),
                );
            }
        }
        if self.installing {
            body = body.child(
                Button::new("component-cancel")
                    .ghost()
                    .icon(IconName::Close)
                    .label(t(cx, "components.cancel"))
                    .on_click(
                        cx.listener(|this, _, _, _| this.cancel.store(true, Ordering::Relaxed)),
                    ),
            );
        }
        let drivers = driver_catalog::DRIVERS
            .iter()
            .filter(|d| d.in_group(self.group))
            .collect::<Vec<_>>();
        let unavailable = drivers
            .iter()
            .filter(|d| !self.ready(d) && self.status(d) != "components.detecting")
            .copied()
            .collect::<Vec<_>>();
        for driver in drivers
            .iter()
            .filter(|d| self.ready(d) || self.status(d) == "components.detecting")
        {
            body = body.child(self.driver_row(driver, cx));
        }
        if !unavailable.is_empty() {
            body = body.child(
                Button::new("component-unavailable-list")
                    .ghost()
                    .icon(if self.show_unavailable {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .label(tr(
                        cx,
                        "components.unavailable_count",
                        &[("count", &unavailable.len().to_string())],
                    ))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_unavailable = !this.show_unavailable;
                        cx.notify();
                    })),
            );
            if self.show_unavailable {
                for driver in unavailable {
                    body = body.child(self.driver_row(driver, cx));
                }
            }
        }
        body.child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(t(cx, "components.on_demand")),
        )
    }
}

//! Update UI; transport and package verification live in services.
use crate::{
    locale::{t, tr},
    operation_log::{Kind, Phase, Record},
};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Disableable, Icon, IconName, StyledExt, WindowExt,
    button::{Button, ButtonVariants},
    notification::{Notification, NotificationType},
};
use serde::{Deserialize, Serialize};
#[cfg(target_os = "windows")]
use std::path::PathBuf;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};
use turbodbn_services::updates::{
    self as service, Check, Download, PackageKind, Progress, Release,
};

#[derive(Default, Clone, Serialize, Deserialize)]
struct Preferences {
    last_check: Option<i64>,
    skipped: Option<String>,
}
impl Preferences {
    fn load() -> Result<Self, String> {
        let path = crate::preferences::Preferences::path().map_err(|_| "updates.error.disk")?;
        crate::app_store::load(
            &path.with_file_name("updates-state.json"),
            "updates",
            4096,
            |_| true,
        )
    }
    fn save(&self) -> Result<(), String> {
        let path = crate::preferences::Preferences::path().map_err(|_| "updates.error.disk")?;
        crate::app_store::save(&path, "updates", self, 4096)
    }
    fn due(&self, now: i64) -> bool {
        self.last_check
            .is_none_or(|last| now < last || now.saturating_sub(last) >= 86400)
    }
}
pub struct Install(pub Download);
pub struct UpdateView {
    preferences: Preferences,
    release: Option<Release>,
    download: Option<Download>,
    checking: bool,
    downloading: bool,
    progress: Option<Progress>,
    status: SharedString,
    cancel: Arc<AtomicBool>,
    task: Option<Task<()>>,
    preference_save: Option<Task<()>>,
    output: Option<Entity<crate::chat::AiLog>>,
}
impl EventEmitter<Install> for UpdateView {}
impl UpdateView {
    pub fn new(output: Option<Entity<crate::chat::AiLog>>, cx: &mut Context<Self>) -> Self {
        Self {
            preferences: Preferences::default(),
            release: None,
            download: None,
            checking: false,
            downloading: false,
            progress: None,
            status: t(cx, "updates.status.idle"),
            cancel: Arc::new(AtomicBool::new(false)),
            task: None,
            preference_save: None,
            output,
        }
    }
    pub fn open(&self, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            dialog
                .title(t(cx, "updates.title"))
                .w(px(600.))
                .child(view.clone())
        });
    }
    pub(crate) fn log(
        &self,
        task: uuid::Uuid,
        phase: Phase,
        started: Instant,
        bytes: u64,
        failure: &'static str,
        cx: &mut Context<Self>,
    ) {
        if let Some(output) = &self.output {
            let mut record = Record::new(task, Kind::Update, phase);
            record.elapsed_ms = started.elapsed().as_millis() as u64;
            record.bytes = bytes;
            record.failure = failure;
            output.update(cx, |output, cx| output.operation(record, cx));
        }
    }
    fn notify_update(&self, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity().downgrade();
        window.push_notification(
            Notification::new()
                .with_type(NotificationType::Info)
                .autohide(false)
                .id1::<UpdateView>("new-version")
                .message(self.status.clone())
                .action(move |_, _, cx| {
                    let view = view.clone();
                    Button::new("view-update")
                        .icon(IconName::ArrowDown)
                        .label(t(cx, "updates.action.view"))
                        .on_click(move |_, window, cx| {
                            let _ = view.update(cx, |this, cx| this.open(window, cx));
                        })
                }),
            cx,
        );
    }
    pub fn check(&mut self, automatic: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.checking || self.downloading {
            if !automatic {
                self.open(window, cx);
            }
            return;
        }
        self.checking = true;
        turbodbn_diagnostics::track(turbodbn_diagnostics::Event::UpdateChecked);
        self.status = t(cx, "common.status.loading");
        if !automatic {
            self.open(window, cx);
        }
        let id = uuid::Uuid::new_v4();
        let started = Instant::now();
        self.log(id, Phase::Started, started, 0, "-", cx);
        let pending_save = self.preference_save.take();
        let work = cx.background_executor().spawn(async move {
            if let Some(pending_save) = pending_save {
                pending_save.await;
            }
            let mut preferences = Preferences::load()?;
            let now = chrono::Utc::now().timestamp();
            if automatic && !preferences.due(now) {
                return Ok((preferences, None, false));
            }
            let result = service::latest(env!("CARGO_PKG_VERSION"));
            preferences.last_check = Some(now);
            let saved = preferences.save().is_ok();
            Ok::<_, String>((preferences, Some(result), saved))
        });
        self.task = Some(cx.spawn_in(window, async move |owner, cx| {
            let result = work.await;
            let _ = owner.update_in(cx, |this, window, cx| {
                this.checking = false;
                match result {
                    Ok((preferences, None, _)) => {
                        this.preferences = preferences;
                        this.status = t(cx, "updates.status.idle");
                        this.log(id, Phase::Succeeded, started, 0, "-", cx);
                    }
                    Ok((preferences, Some(result), saved)) => {
                        this.preferences = preferences;
                        this.log(
                            id,
                            if result.is_ok() {
                                Phase::Succeeded
                            } else {
                                Phase::Failed
                            },
                            started,
                            0,
                            if result.is_ok() { "-" } else { "update_check" },
                            cx,
                        );
                        if !saved {
                            crate::feedback::error(t(cx, "updates.error.preferences"), cx);
                        }
                        match result {
                            Ok(Check::Available(release)) => {
                                if this
                                    .release
                                    .as_ref()
                                    .is_none_or(|old| old.tag_name != release.tag_name)
                                {
                                    this.download = None;
                                    this.progress = None;
                                }
                                this.status = tr(
                                    cx,
                                    "updates.status.available",
                                    &[("version", &release.tag_name)],
                                );
                                this.release = Some(release);
                                if !automatic
                                    || this.preferences.skipped.as_deref()
                                        != this
                                            .release
                                            .as_ref()
                                            .map(|release| release.tag_name.as_str())
                                {
                                    this.notify_update(window, cx);
                                }
                            }
                            Ok(Check::Current) => {
                                this.release = None;
                                this.download = None;
                                this.status = tr(
                                    cx,
                                    "updates.status.current",
                                    &[("version", env!("CARGO_PKG_VERSION"))],
                                );
                                if !automatic {
                                    crate::feedback::success(this.status.clone(), cx);
                                }
                            }
                            Ok(Check::NoRelease) => {
                                this.release = None;
                                this.download = None;
                                this.status = t(cx, "updates.status.no_release");
                            }
                            Err(error) => {
                                this.status = crate::locale::message(cx, &error);
                                if !automatic {
                                    crate::feedback::error(this.status.clone(), cx);
                                }
                            }
                        }
                    }
                    Err(_) => {
                        this.log(id, Phase::Failed, started, 0, "update_preferences", cx);
                        this.status = t(cx, "updates.error.preferences");
                        if !automatic {
                            crate::feedback::error(this.status.clone(), cx);
                        }
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn skip(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.preferences.skipped = self
            .release
            .as_ref()
            .map(|release| release.tag_name.clone());
        let preferences = self.preferences.clone();
        let work = cx
            .background_executor()
            .spawn(async move { preferences.save() });
        self.preference_save = Some(cx.spawn(async move |_, cx| {
            if work.await.is_err() {
                let _ =
                    cx.update(|cx| crate::feedback::error(t(cx, "updates.error.preferences"), cx));
            }
        }));
        window.close_dialog(cx);
        window.remove_notification::<UpdateView>(cx);
    }
    fn start_download(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.downloading || self.checking {
            return;
        }
        let Some(release) = self.release.clone() else {
            return;
        };
        let kind = package_kind();
        let Some(kind) = kind else {
            self.status = t(cx, "updates.error.platform");
            cx.notify();
            return;
        };
        self.downloading = true;
        self.download = None;
        self.progress = None;
        self.status = t(cx, "updates.status.downloading");
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let id = uuid::Uuid::new_v4();
        let started = Instant::now();
        self.log(id, Phase::Started, started, 0, "-", cx);
        let (sender, receiver) = async_channel::bounded(4);
        let work = cx.background_executor().spawn(async move {
            let result = dirs::cache_dir()
                .ok_or_else(|| "updates.error.disk".to_string())
                .and_then(|root| {
                    service::download(
                        &release,
                        kind,
                        &root.join("TurboDbNote/updates").join(&release.tag_name),
                        &cancel,
                        |progress| {
                            let _ = sender.try_send(progress);
                        },
                    )
                });
            drop(sender);
            result
        });
        self.task = Some(cx.spawn_in(window, async move |owner, cx| {
            while let Ok(progress) = receiver.recv().await {
                if owner
                    .update(cx, |this, cx| {
                        this.progress = Some(progress);
                        this.log(id, Phase::Progress, started, progress.bytes, "-", cx);
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
            let result = work.await;
            let _ = owner.update_in(cx, |this, window, cx| {
                this.downloading = false;
                let bytes = this.progress.map_or(0, |progress| progress.bytes);
                match result {
                    Ok(download) => {
                        this.log(id, Phase::Succeeded, started, download.bytes, "-", cx);
                        this.download = Some(download);
                        this.status = t(cx, "updates.status.ready");
                        crate::feedback::success(this.status.clone(), cx);
                        this.notify_update(window, cx);
                    }
                    Err(error) => {
                        let cancelled = error == "updates.status.cancelled";
                        this.log(
                            id,
                            if cancelled {
                                Phase::Cancelled
                            } else {
                                Phase::Failed
                            },
                            started,
                            bytes,
                            if cancelled { "-" } else { "update_download" },
                            cx,
                        );
                        this.status = crate::locale::message(cx, &error);
                        if !cancelled {
                            crate::feedback::error(this.status.clone(), cx);
                        }
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
}
impl Drop for UpdateView {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
impl Render for UpdateView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.checking || self.downloading;
        let version = self
            .release
            .as_ref()
            .map(|release| release.tag_name.clone())
            .unwrap_or_default();
        let url = self
            .release
            .as_ref()
            .map(|release| release.html_url.clone())
            .unwrap_or_else(|| service::RELEASES.into());
        let notes: String = self
            .release
            .as_ref()
            .and_then(|release| release.body.as_deref())
            .unwrap_or_default()
            .chars()
            .take(8000)
            .collect();
        div()
            .v_flex()
            .gap_3()
            .child(
                div()
                    .h_flex()
                    .gap_3()
                    .child(Icon::new(IconName::Info))
                    .child(self.status.clone()),
            )
            .when(!version.is_empty(), |el| {
                el.child(tr(
                    cx,
                    "updates.version",
                    &[("current", env!("CARGO_PKG_VERSION")), ("latest", &version)],
                ))
            })
            .when(!notes.is_empty(), |el| {
                el.child(
                    div()
                        .id("update-notes")
                        .max_h(px(240.))
                        .overflow_y_scroll()
                        .whitespace_normal()
                        .text_color(cx.theme().muted_foreground)
                        .child(notes),
                )
            })
            .when(self.downloading, |el| {
                let status = if self.cancel.load(Ordering::Relaxed) {
                    t(cx, "updates.status.cancelling")
                } else if let Some(progress) = self.progress {
                    if progress.verifying {
                        t(cx, "updates.status.verifying")
                    } else {
                        tr(
                            cx,
                            "updates.progress",
                            &[
                                (
                                    "percent",
                                    &format!(
                                        "{:.0}",
                                        progress.bytes as f64 * 100. / progress.total.max(1) as f64
                                    ),
                                ),
                                (
                                    "downloaded",
                                    &format!("{:.1}", progress.bytes as f64 / 1048576.),
                                ),
                                ("total", &format!("{:.1}", progress.total as f64 / 1048576.)),
                            ],
                        )
                    }
                } else {
                    t(cx, "updates.status.downloading")
                };
                el.child(status)
            })
            .when(self.download.is_some(), |el| {
                el.child(t(
                    cx,
                    if self
                        .download
                        .as_ref()
                        .is_some_and(|download| download.kind == PackageKind::WindowsInstaller)
                    {
                        "updates.install.description"
                    } else {
                        "updates.manual.description"
                    },
                ))
            })
            .child(
                crate::feedback::actions()
                    .flex_wrap()
                    .child(crate::feedback::close_button(cx).label(t(cx, "updates.action.later")))
                    .when(
                        self.release.is_some() && !busy && self.download.is_none(),
                        |el| {
                            el.child(
                                Button::new("skip-update")
                                    .icon(IconName::Close)
                                    .label(t(cx, "updates.action.skip"))
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.skip(window, cx)),
                                    ),
                            )
                        },
                    )
                    .child(
                        Button::new("update-release-page")
                            .icon(IconName::ExternalLink)
                            .label(t(cx, "updates.action.notes"))
                            .on_click(move |_, _, cx| cx.open_url(&url)),
                    )
                    .when(self.release.is_none(), |el| {
                        el.child(
                            Button::new("retry-update-check")
                                .icon(crate::assets::refresh_icon())
                                .label(t(cx, "help.action.check_updates"))
                                .disabled(busy)
                                .on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.check(false, window, cx)
                                    }),
                                ),
                        )
                    })
                    .when(self.downloading, |el| {
                        el.child(
                            Button::new("cancel-update-download")
                                .icon(IconName::Close)
                                .label(t(cx, "updates.action.cancel"))
                                .disabled(self.cancel.load(Ordering::Relaxed))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.cancel.store(true, Ordering::Relaxed);
                                    cx.notify();
                                })),
                        )
                    })
                    .when(
                        self.release.is_some() && !busy && self.download.is_none(),
                        |el| {
                            el.child(
                                Button::new("download-update")
                                    .primary()
                                    .icon(IconName::ArrowDown)
                                    .label(t(cx, "updates.action.download"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.start_download(window, cx)
                                    })),
                            )
                        },
                    )
                    .when_some(self.download.clone(), |el, download| {
                        let path = download.path.clone();
                        el.child(
                            Button::new("reveal-update")
                                .icon(IconName::FolderOpen)
                                .label(t(cx, "updates.action.folder"))
                                .on_click(move |_, _, cx| cx.reveal_path(&path)),
                        )
                        .when(
                            download.kind == PackageKind::WindowsInstaller,
                            |el| {
                                el.child(
                                    Button::new("install-update")
                                        .primary()
                                        .icon(IconName::ArrowDown)
                                        .label(t(cx, "updates.action.install"))
                                        .disabled(busy)
                                        .on_click(cx.listener(move |_, _, window, cx| {
                                            window.close_dialog(cx);
                                            cx.emit(Install(download.clone()));
                                        })),
                                )
                            },
                        )
                    }),
            )
    }
}
fn package_kind() -> Option<PackageKind> {
    if std::env::consts::ARCH != "x86_64" {
        return None;
    }
    match std::env::consts::OS {
        "windows" => Some(if installed() {
            PackageKind::WindowsInstaller
        } else {
            PackageKind::WindowsPortable
        }),
        "linux" => Some(PackageKind::Linux),
        _ => None,
    }
}
fn installed() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|path| path.to_path_buf()))
        .is_some_and(|directory| {
            !directory.join("portable.flag").exists() && directory.join("unins000.exe").is_file()
        })
}

#[cfg(target_os = "windows")]
#[derive(Serialize, Deserialize)]
struct InstallerRequest {
    path: PathBuf,
    sha256: String,
    parent: u32,
    #[serde(default = "default_install_failure")]
    failure_message: String,
}

#[cfg(target_os = "windows")]
fn default_install_failure() -> String {
    crate::locale_catalog::parse_pack(include_str!("../assets/locales/en.json"))
        .expect("validated bundled pack")
        .strings["updates.error.install_after_exit"]
        .clone()
}

#[cfg(target_os = "windows")]
pub fn launch_installer_helper(
    download: &Download,
    failure_message: String,
) -> Result<std::process::Child, String> {
    use std::{
        io::{Read, Write},
        os::windows::process::CommandExt,
        process::{Command, Stdio},
    };
    if download.kind != PackageKind::WindowsInstaller {
        return Err("updates.error.install".into());
    }
    let mut child = Command::new(std::env::current_exe().map_err(|_| "updates.error.install")?)
        .arg("--install-verified-update")
        .creation_flags(0x08000000)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "updates.error.install")?;
    let request = InstallerRequest {
        path: download.path.clone(),
        sha256: download.sha256.clone(),
        parent: std::process::id(),
        failure_message,
    };
    let result = (|| {
        let bytes = serde_json::to_vec(&request).map_err(|_| "updates.error.install")?;
        child
            .stdin
            .take()
            .ok_or("updates.error.install")?
            .write_all(&bytes)
            .map_err(|_| "updates.error.install")?;
        let mut ready = [0];
        let mut stdout = child.stdout.take().ok_or("updates.error.install")?;
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let result = stdout.read_exact(&mut ready).map(|()| ready);
            let _ = sender.send(result);
        });
        let ready = receiver
            .recv_timeout(std::time::Duration::from_secs(60))
            .map_err(|_| "updates.error.install")?
            .map_err(|_| "updates.error.install")?;
        if ready != [1] {
            return Err("updates.error.install".into());
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result.map(|()| child)
}
#[cfg(not(target_os = "windows"))]
pub fn launch_installer_helper(_: &Download, _: String) -> Result<std::process::Child, String> {
    Err("updates.error.platform".into())
}

/// This mode runs before GPUI, diagnostics and the application lifetime mutex.
#[cfg(target_os = "windows")]
pub fn run_installer_helper() -> bool {
    if std::env::args_os().nth(1).as_deref()
        != Some(std::ffi::OsStr::new("--install-verified-update"))
    {
        return false;
    }
    let mut failure = default_install_failure();
    if installer_helper(&mut failure).is_err() {
        use std::io::Write;
        let _ = std::io::stdout().write_all(&[0]);
        let _ = std::io::stdout().flush();
        use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
        let text: Vec<u16> = failure.encode_utf16().chain(std::iter::once(0)).collect();
        let title: Vec<u16> = "TurboDBNote\0".encode_utf16().collect();
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                text.as_ptr(),
                title.as_ptr(),
                MB_OK | MB_ICONERROR,
            );
        }
    }
    true
}
#[cfg(target_os = "windows")]
fn installer_helper(failure: &mut String) -> Result<(), String> {
    use std::{
        io::{Read, Write},
        os::windows::{fs::OpenOptionsExt, process::CommandExt},
    };
    use windows_sys::Win32::{
        Foundation::{CloseHandle, WAIT_OBJECT_0},
        System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
    };
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(16385)
        .read_to_end(&mut bytes)
        .map_err(|_| "updates.error.install")?;
    if bytes.len() > 16384 {
        return Err("updates.error.install".into());
    }
    let request: InstallerRequest =
        serde_json::from_slice(&bytes).map_err(|_| "updates.error.install")?;
    *failure = request.failure_message;
    let root = dirs::cache_dir()
        .ok_or("updates.error.install")?
        .join("TurboDbNote/updates")
        .canonicalize()
        .map_err(|_| "updates.error.install")?;
    let path = request
        .path
        .canonicalize()
        .map_err(|_| "updates.error.install")?;
    if !path.starts_with(root)
        || !path.file_name().is_some_and(|name| {
            name.to_string_lossy().starts_with("TurboDBNote-v")
                && name.to_string_lossy().ends_with("-windows-x64-setup.exe")
        })
    {
        return Err("updates.error.install".into());
    }
    // Keep a read handle that denies modification/deletion until the installer starts.
    let _locked_package = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&path)
        .map_err(|_| "updates.error.install")?;
    let cancel = AtomicBool::new(false);
    service::verify(&path, &request.sha256, &cancel)?;
    let parent = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, request.parent) };
    if parent.is_null() {
        return Err("updates.error.install".into());
    }
    let mut stdout = std::io::stdout().lock();
    let ready = stdout.write_all(&[1]).and_then(|()| stdout.flush());
    drop(stdout);
    let wait = if ready.is_ok() {
        unsafe { WaitForSingleObject(parent, 300_000) }
    } else {
        u32::MAX
    };
    unsafe {
        CloseHandle(parent);
    }
    if wait != WAIT_OBJECT_0 {
        return Err("updates.error.install".into());
    }
    service::verify(&path, &request.sha256, &cancel)?;
    std::process::Command::new(&path)
        .creation_flags(0x08000000)
        .spawn()
        .map_err(|_| "updates.error.install")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Preferences;
    #[test]
    fn automatic_checks_are_throttled_and_clock_rollback_recovers() {
        let prefs = Preferences {
            last_check: Some(100000),
            skipped: Some("v0.2.0".into()),
        };
        assert!(!prefs.due(100001));
        assert!(prefs.due(186400));
        assert!(prefs.due(99999));
        assert!(Preferences::default().due(100000));
    }
    #[test]
    fn update_preferences_use_an_independent_sqlite_namespace() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("connections.db");
        let prefs = Preferences {
            last_check: Some(100000),
            skipped: Some("v0.2.0".into()),
        };
        crate::app_store::save(&file, "updates", &prefs, 4096).unwrap();
        let loaded: Preferences = crate::app_store::load(
            &dir.path().join("updates-state.json"),
            "updates",
            4096,
            |_| true,
        )
        .unwrap();
        assert_eq!(loaded.last_check, prefs.last_check);
        assert_eq!(loaded.skipped, prefs.skipped);
        assert!(!dir.path().join("updates-state.json").exists());
        let db = rusqlite::Connection::open(&file).unwrap();
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM connections", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        db.execute(
            "UPDATE app_documents SET version=2 WHERE name='updates'",
            [],
        )
        .unwrap();
        assert!(crate::app_store::save(&file, "updates", &prefs, 4096).is_err());
    }
}

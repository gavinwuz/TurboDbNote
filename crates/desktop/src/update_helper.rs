//! Standalone Windows updater: no GPUI, application settings or diagnostics.
use std::{
    io::{Read, Write},
    os::windows::fs::OpenOptionsExt,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use turbodbn_services::updates::{self as service, InstallEvent, InstallRequest};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, GetLastError, WAIT_OBJECT_0,
    },
    System::{
        LibraryLoader::GetModuleHandleW,
        Threading::{
            CreateMutexW, OpenMutexW, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
            PROCESS_SYNCHRONIZE, QueryFullProcessImageNameW, WaitForSingleObject,
        },
    },
    UI::{
        Controls::{
            ICC_PROGRESS_CLASS, INITCOMMONCONTROLSEX, InitCommonControlsEx, PBM_SETMARQUEE,
            PBM_SETPOS, PBS_MARQUEE,
        },
        WindowsAndMessaging::*,
    },
};
static CANCEL_REQUESTED: AtomicBool = AtomicBool::new(false);
// Win32 SS_ICON; keep the helper independent of the SystemServices bindings.
const STATIC_ICON_STYLE: u32 = 3;
unsafe extern "system" fn window_proc(
    window: windows_sys::Win32::Foundation::HWND,
    message: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    if message == WM_CLOSE || (message == WM_COMMAND && wparam & 0xffff == 1) {
        CANCEL_REQUESTED.store(true, Ordering::Relaxed);
        return 0;
    }
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
struct Text(serde_json::Value);
impl Text {
    fn new(language: &str) -> Self {
        let source = if language == "zh-CN" {
            include_str!("../assets/locales/zh-CN.json")
        } else {
            include_str!("../assets/locales/en.json")
        };
        Self(serde_json::from_str(source).expect("bundled language"))
    }
    fn get(&self, key: &str) -> &str {
        self.0["strings"][key]
            .as_str()
            .unwrap_or("Update could not continue.")
    }
}
fn emit(value: InstallEvent) {
    if let Ok(mut bytes) = serde_json::to_vec(&value) {
        bytes.push(b'\n');
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(&bytes);
        let _ = out.flush();
    }
}
struct Handle(windows_sys::Win32::Foundation::HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
enum Status {
    Verify(u64, u64),
    Waiting,
    Others,
    Installing,
    Done(Result<(), String>, bool),
}
fn prepare(
    request: &InstallRequest,
    cancel: &AtomicBool,
    sender: &mpsc::Sender<Status>,
) -> Result<(PathBuf, std::fs::File, Handle), String> {
    let root = dirs::cache_dir()
        .ok_or("updates.error.disk")?
        .join("TurboDbNote/updates")
        .canonicalize()
        .map_err(|_| "updates.error.disk")?;
    let path = request
        .path
        .canonicalize()
        .map_err(|_| "updates.error.disk")?;
    if !path.starts_with(root)
        || path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(service::installer_version)
            .is_none()
    {
        return Err("updates.error.package_identity".into());
    }
    let application = request
        .application
        .canonicalize()
        .map_err(|_| "updates.error.target")?;
    if application.file_name().and_then(|s| s.to_str()) != Some("turbodbnote.exe")
        || !application
            .parent()
            .is_some_and(|p| p.join("unins000.exe").is_file() && !p.join("portable.flag").exists())
    {
        return Err("updates.error.target".into());
    }
    let parent = Handle(unsafe {
        OpenProcess(
            PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            request.parent,
        )
    });
    if parent.0.is_null() {
        return Err("updates.error.parent".into());
    }
    let mut name = vec![0u16; 32768];
    let mut len = name.len() as u32;
    if unsafe { QueryFullProcessImageNameW(parent.0, 0, name.as_mut_ptr(), &mut len) } == 0
        || PathBuf::from(String::from_utf16_lossy(&name[..len as usize]))
            .canonicalize()
            .ok()
            != Some(application)
    {
        return Err("updates.error.parent".into());
    }
    let locked = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&path)
        .map_err(|_| "updates.error.disk")?;
    service::verify_progress(&path, &request.sha256, cancel, &|p| {
        let _ = sender.send(Status::Verify(p.bytes, p.total));
    })?;
    Ok((path, locked, parent))
}
fn work(
    request: InstallRequest,
    cancel: Arc<AtomicBool>,
    sender: mpsc::Sender<Status>,
) -> Result<(), String> {
    let update_guard = Handle(unsafe {
        CreateMutexW(
            std::ptr::null(),
            0,
            wide("TurboDbNote.UpdateInProgress").as_ptr(),
        )
    });
    let existing = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if update_guard.0.is_null() {
        return Err("updates.error.helper_start".into());
    }
    if existing {
        return Err("updates.error.in_progress".into());
    }
    let (path, _lock, parent) = prepare(&request, &cancel, &sender)?;
    if cancel.load(Ordering::Relaxed) {
        return Err("updates.status.cancelled".into());
    }
    emit(InstallEvent::Ready);
    let _ = sender.send(Status::Waiting);
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("updates.status.cancelled".into());
        }
        if Instant::now() >= deadline {
            return Err("updates.helper.wait_timeout".into());
        }
        if unsafe { WaitForSingleObject(parent.0, 100) } == WAIT_OBJECT_0 {
            break;
        }
    }
    drop(parent);
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("updates.status.cancelled".into());
        }
        let mutex = unsafe { OpenMutexW(0x00100000, 0, wide("TurboDbNote.Running").as_ptr()) };
        if mutex.is_null() {
            if unsafe { GetLastError() } == ERROR_FILE_NOT_FOUND {
                break;
            }
            return Err("updates.error.parent".into());
        }
        unsafe {
            CloseHandle(mutex);
        }
        let _ = sender.send(Status::Others);
        if Instant::now() >= deadline {
            return Err("updates.helper.wait_timeout".into());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    service::verify(&path, &request.sha256, &cancel)?;
    if cancel.load(Ordering::Relaxed) {
        return Err("updates.status.cancelled".into());
    }
    let _ = sender.send(Status::Installing);
    // Inno owns real installation progress and cancellation; never silently install.
    let target = request.application.parent().ok_or("updates.error.target")?;
    let status = Command::new(&path)
        .arg("/SP-")
        .arg("/NORESTART")
        .arg("/TURBODBNUPDATE=1")
        .arg(format!("/DIR={}", target.display()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "updates.error.install")?
        .wait()
        .map_err(|_| "updates.error.install_after_exit")?;
    if status.code() != Some(0) {
        return Err(if status.code() == Some(2) {
            "updates.helper.install_cancelled"
        } else {
            "updates.error.install_after_exit"
        }
        .into());
    }
    drop(_lock);
    if request.restart {
        let mut child = Command::new(&request.application)
            .arg("--update-completed")
            .current_dir(target)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "updates.helper.restart_failed")?;
        std::thread::sleep(Duration::from_millis(500));
        if child
            .try_wait()
            .map_err(|_| "updates.helper.restart_failed")?
            .is_some_and(|status| !status.success())
        {
            return Err("updates.helper.restart_failed".into());
        }
    }
    Ok(())
}
fn receipt(
    directory: &std::path::Path,
    task: &str,
    phase: &str,
    elapsed: u128,
    error: Option<&str>,
) -> std::io::Result<()> {
    let root = dirs::cache_dir()
        .ok_or_else(|| std::io::Error::other("Cache unavailable"))?
        .join("TurboDbNote/updates");
    std::fs::create_dir_all(&root)?;
    let directory = directory.canonicalize()?;
    if !directory.starts_with(root.canonicalize()?) {
        return Err(std::io::Error::other("Receipt outside update cache"));
    }
    let mut file = tempfile::NamedTempFile::new_in(&directory)?;
    let data = serde_json::json!({"task":task,"operation":"update_install","phase":phase,"elapsed_ms":elapsed,"error":error});
    file.write_all(data.to_string().as_bytes())?;
    file.as_file().sync_all()?;
    file.persist(directory.join("last-install-result.json"))
        .map_err(|e| e.error)?;
    Ok(())
}

pub fn run() {
    let mut bytes = vec![];
    if std::io::stdin()
        .take(16385)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() > 16384
    {
        emit(InstallEvent::Failed("updates.error.install".into()));
        return;
    }
    let Ok(request) = serde_json::from_slice::<InstallRequest>(&bytes) else {
        emit(InstallEvent::Failed("updates.error.install".into()));
        return;
    };
    let text = Text::new(&request.language);
    unsafe {
        InitCommonControlsEx(&INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_PROGRESS_CLASS,
        });
    }
    let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
    // A native status window keeps this helper small and independent of GPU/UI initialization.
    let window = unsafe {
        CreateWindowExW(
            0,
            wide("STATIC").as_ptr(),
            wide(text.get("updates.helper.title")).as_ptr(),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            520,
            180,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        )
    };
    if window.is_null() {
        emit(InstallEvent::Failed("updates.error.install".into()));
        return;
    }
    unsafe {
        SetWindowLongPtrW(window, GWLP_WNDPROC, window_proc as *const () as isize);
    }
    for (kind, size) in [(ICON_SMALL, 16), (ICON_BIG, 32)] {
        let icon = unsafe { LoadImageW(instance, 1usize as _, IMAGE_ICON, size, size, LR_SHARED) };
        if !icon.is_null() {
            unsafe {
                SendMessageW(window, WM_SETICON, kind as usize, icon as isize);
            }
        }
    }
    let logo = unsafe {
        CreateWindowExW(
            0,
            wide("STATIC").as_ptr(),
            std::ptr::null(),
            WS_CHILD | WS_VISIBLE | STATIC_ICON_STYLE,
            20,
            20,
            48,
            48,
            window,
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        )
    };
    let icon = unsafe { LoadImageW(instance, 1usize as _, IMAGE_ICON, 48, 48, LR_SHARED) };
    if !logo.is_null() && !icon.is_null() {
        unsafe {
            SendMessageW(logo, STM_SETICON, icon as usize, 0);
        }
    }
    let label = unsafe {
        CreateWindowExW(
            0,
            wide("STATIC").as_ptr(),
            wide(text.get("updates.status.verifying")).as_ptr(),
            WS_CHILD | WS_VISIBLE,
            84,
            20,
            406,
            44,
            window,
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        )
    };
    let bar = unsafe {
        CreateWindowExW(
            0,
            wide("msctls_progress32").as_ptr(),
            std::ptr::null(),
            WS_CHILD | WS_VISIBLE | PBS_MARQUEE,
            84,
            75,
            406,
            18,
            window,
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        )
    };
    let button = unsafe {
        CreateWindowExW(
            0,
            wide("BUTTON").as_ptr(),
            wide(text.get("updates.helper.cancel")).as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP,
            365,
            105,
            125,
            28,
            window,
            1isize as _,
            instance,
            std::ptr::null(),
        )
    };
    if label.is_null() || bar.is_null() || button.is_null() {
        emit(InstallEvent::Failed("updates.error.helper_start".into()));
        unsafe {
            DestroyWindow(window);
        }
        return;
    }
    let font = unsafe {
        windows_sys::Win32::Graphics::Gdi::GetStockObject(
            windows_sys::Win32::Graphics::Gdi::DEFAULT_GUI_FONT,
        )
    };
    for control in [label, button] {
        unsafe {
            SendMessageW(control, WM_SETFONT, font as usize, 1);
        }
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let (sender, receiver) = mpsc::channel();
    let stopped = cancel.clone();
    let application = request.application.clone();
    let package = request.path.clone();
    let worker = std::thread::spawn(move || {
        let directory = request
            .path
            .parent()
            .unwrap_or(std::path::Path::new(""))
            .to_owned();
        let task = uuid::Uuid::new_v4().to_string();
        let start = Instant::now();
        let began = receipt(&directory, &task, "started", 0, None).is_ok();
        let result = work(request, stopped, sender.clone());
        let phase = match &result {
            Ok(()) => "completed",
            Err(key)
                if key == "updates.status.cancelled"
                    || key == "updates.helper.install_cancelled" =>
            {
                "cancelled"
            }
            Err(_) => "failed",
        };
        let logged = receipt(
            &directory,
            &task,
            phase,
            start.elapsed().as_millis(),
            result.as_ref().err().map(String::as_str),
        )
        .is_ok()
            && began;
        let _ = sender.send(Status::Done(result, logged));
    });
    let mut prepared = false;
    let mut installing = false;
    let mut finished = false;
    let mut msg: MSG = unsafe { std::mem::zeroed() };
    while !finished {
        if !installing && CANCEL_REQUESTED.load(Ordering::Relaxed) {
            cancel.store(true, Ordering::Relaxed);
        }
        unsafe {
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                if msg.message == WM_COMMAND && msg.hwnd == window && msg.wParam & 0xffff == 1 {
                    cancel.store(true, Ordering::Relaxed);
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            if !installing && IsWindow(window) == 0 {
                cancel.store(true, Ordering::Relaxed);
            }
        }
        while let Ok(status) = receiver.try_recv() {
            let (key, percent) = match status {
                Status::Verify(bytes, total) => {
                    emit(InstallEvent::Progress { bytes, total });
                    (
                        "updates.status.verifying",
                        Some(bytes as f64 * 100. / total.max(1) as f64),
                    )
                }
                Status::Waiting => {
                    prepared = true;
                    ("updates.helper.waiting", None)
                }
                Status::Others => ("updates.helper.others", None),
                Status::Installing => {
                    installing = true;
                    unsafe {
                        ShowWindow(window, SW_HIDE);
                    }
                    ("updates.helper.installing", None)
                }
                Status::Done(result, logged) => {
                    if prepared && !logged {
                        unsafe {
                            MessageBoxW(
                                window,
                                wide(text.get("updates.helper.log_failed")).as_ptr(),
                                wide(text.get("updates.helper.title")).as_ptr(),
                                MB_OK | MB_ICONWARNING,
                            );
                        }
                    }
                    if let Err(ref key) = result {
                        emit(InstallEvent::Failed(key.clone()));
                        if prepared && !cancel.load(Ordering::Relaxed) {
                            unsafe {
                                ShowWindow(window, SW_SHOW);
                            }
                            let message = format!(
                                "{}\n\n{}",
                                text.get(key),
                                text.get(if key == "updates.helper.restart_failed" {
                                    "updates.helper.restart_recovery"
                                } else {
                                    "updates.helper.recovery"
                                })
                            );
                            let open = unsafe {
                                MessageBoxW(
                                    window,
                                    wide(&message).as_ptr(),
                                    wide(text.get("updates.helper.title")).as_ptr(),
                                    MB_YESNO | MB_ICONERROR,
                                )
                            };
                            if open == IDYES
                                && let Some(target) = if key == "updates.helper.restart_failed" {
                                    application.parent()
                                } else {
                                    package.parent()
                                }
                            {
                                let _ = Command::new("explorer.exe").arg(target).spawn();
                            }
                        }
                    }
                    finished = true;
                    continue;
                }
            };
            unsafe {
                let message = if let Some(value) = percent {
                    text.get(key).replace("{percent}", &format!("{value:.0}"))
                } else {
                    text.get(key).to_string()
                };
                SetWindowTextW(label, wide(&message).as_ptr());
                if let Some(percent) = percent {
                    SendMessageW(bar, PBM_SETMARQUEE, 0, 0);
                    SetWindowLongW(bar, GWL_STYLE, (WS_CHILD | WS_VISIBLE) as i32);
                    SendMessageW(bar, PBM_SETPOS, percent.clamp(0., 100.) as usize, 0);
                } else {
                    SetWindowLongW(bar, GWL_STYLE, (WS_CHILD | WS_VISIBLE | PBS_MARQUEE) as i32);
                    SendMessageW(bar, PBM_SETMARQUEE, 1, 30);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    let _ = worker.join();
    unsafe {
        if IsWindow(window) != 0 {
            DestroyWindow(window);
        }
    }
}

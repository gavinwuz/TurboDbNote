//! A disposable Win32 notification-area owner; GPUI owns visibility and exit policy.
use async_channel::{Receiver, Sender};
use gpui::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use windows_sys::Win32::{
    Foundation::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{Shell::*, WindowsAndMessaging::*},
};

const CALLBACK: u32 = WM_APP + 17;
const OPEN: usize = 1;
const EXIT: usize = 2;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Ready,
    Open,
    Exit,
    Failed,
}
pub struct Tray {
    hwnd: Arc<AtomicUsize>,
}
struct State {
    sender: Sender<Event>,
    open: Vec<u16>,
    exit: Vec<u16>,
    icon: NOTIFYICONDATAW,
    restart: u32,
}
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

pub fn hwnd(window: &Window) -> Option<HWND> {
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(raw) => Some(raw.hwnd.get() as HWND),
        _ => None,
    }
}
pub fn hide(window: &Window) {
    if let Some(hwnd) = hwnd(window) {
        // SAFETY: live GPUI window, on its UI thread. Hide retains all entities.
        unsafe {
            ShowWindow(hwnd, SW_HIDE);
        }
    }
}
pub fn restore(window: &mut Window) {
    if let Some(hwnd) = hwnd(window) {
        // SAFETY: live GPUI window. SHOW preserves maximized state; RESTORE is
        // only used when minimized. Windows can decline foreground activation.
        unsafe {
            ShowWindow(
                hwnd,
                if IsIconic(hwnd) != 0 {
                    SW_RESTORE
                } else {
                    SW_SHOW
                },
            );
            SetForegroundWindow(hwnd);
        }
    }
    window.activate_window();
}
impl Tray {
    pub fn start(open: &str, exit: &str) -> (Self, Receiver<Event>) {
        let (sender, receiver) = async_channel::bounded(8);
        let hwnd = Arc::new(AtomicUsize::new(0));
        let owned = hwnd.clone();
        let open = wide(open);
        let exit = wide(exit);
        let failure = sender.clone();
        if std::thread::Builder::new()
            .name("notification-area".into())
            .spawn(move || {
                // SAFETY: this thread exclusively owns the class, HWND, state and
                // message loop. The Box remains valid until DestroyWindow returns.
                unsafe {
                    run(sender, owned, open, exit);
                }
            })
            .is_err()
        {
            let _ = failure.try_send(Event::Failed);
        }
        (Self { hwnd }, receiver)
    }
}
impl Drop for Tray {
    fn drop(&mut self) {
        let hwnd = self.hwnd.swap(usize::MAX, Ordering::AcqRel);
        if hwnd != 0 && hwnd != usize::MAX {
            // SAFETY: this icon is identified by its owner HWND and ID. Delete
            // it synchronously before app quit, then let the owner free its HWND.
            unsafe {
                let icon = NOTIFYICONDATAW {
                    cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                    hWnd: hwnd as HWND,
                    uID: 1,
                    ..std::mem::zeroed()
                };
                Shell_NotifyIconW(NIM_DELETE, &icon);
                PostMessageW(hwnd as HWND, WM_CLOSE, 0, 0);
            }
        }
    }
}
unsafe fn run(sender: Sender<Event>, shared: Arc<AtomicUsize>, open: Vec<u16>, exit: Vec<u16>) {
    // SAFETY: all pointers are local, initialized and live through this loop.
    unsafe {
        let module = GetModuleHandleW(std::ptr::null());
        let class = wide(&format!(
            "TurboDBNote.NotificationArea.{}",
            uuid::Uuid::new_v4()
        ));
        let wc = WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: module,
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        if RegisterClassW(&wc) == 0 {
            let _ = sender.try_send(Event::Failed);
            return;
        }
        let mut state = Box::new(State {
            sender,
            open,
            exit,
            icon: std::mem::zeroed(),
            restart: RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
        });
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            class.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            module,
            (&mut *state as *mut State).cast(),
        );
        if hwnd.is_null() {
            let _ = state.sender.try_send(Event::Failed);
            UnregisterClassW(class.as_ptr(), module);
            return;
        }
        state.icon.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        state.icon.hWnd = hwnd;
        state.icon.uID = 1;
        state.icon.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        state.icon.uCallbackMessage = CALLBACK;
        state.icon.hIcon = LoadImageW(
            module,
            std::ptr::without_provenance(1),
            IMAGE_ICON,
            16,
            16,
            LR_SHARED,
        ) as HICON;
        for (dst, src) in state.icon.szTip.iter_mut().zip(wide("TurboDBNote")) {
            *dst = src;
        }
        if state.icon.hIcon.is_null() || Shell_NotifyIconW(NIM_ADD, &state.icon) == 0 {
            let _ = state.sender.try_send(Event::Failed);
            DestroyWindow(hwnd);
            UnregisterClassW(class.as_ptr(), module);
            return;
        }
        if shared
            .compare_exchange(0, hwnd as usize, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            DestroyWindow(hwnd);
            UnregisterClassW(class.as_ptr(), module);
            return;
        }
        let _ = state.sender.try_send(Event::Ready);
        let mut msg = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        if IsWindow(hwnd) != 0 {
            let _ = state.sender.try_send(Event::Failed);
            DestroyWindow(hwnd);
        }
        shared.store(usize::MAX, Ordering::Release);
        UnregisterClassW(class.as_ptr(), module);
    }
}
unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // SAFETY: WM_NCCREATE supplies CREATESTRUCTW; subsequent messages use the
    // Box owned by run, which outlives DestroyWindow and this callback.
    unsafe {
        if msg == WM_NCCREATE {
            let creation = &*(lp as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, creation.lpCreateParams as isize);
        }
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
        if ptr.is_null() {
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        let state = &*ptr;
        if state.restart != 0 && msg == state.restart {
            if Shell_NotifyIconW(NIM_ADD, &state.icon) == 0 {
                let _ = state.sender.try_send(Event::Failed);
            }
            return 0;
        }
        match msg {
            WM_COMMAND => {
                if let Some(event) = menu_event(wp & 0xffff) {
                    let _ = state.sender.try_send(event);
                }
                0
            }
            CALLBACK => {
                match lp as u32 {
                    WM_LBUTTONDBLCLK => {
                        let _ = state.sender.try_send(Event::Open);
                    }
                    WM_RBUTTONUP | WM_CONTEXTMENU => {
                        let menu = CreatePopupMenu();
                        if !menu.is_null() {
                            AppendMenuW(menu, MF_STRING, OPEN, state.open.as_ptr());
                            AppendMenuW(menu, MF_STRING, EXIT, state.exit.as_ptr());
                            let mut point = POINT { x: 0, y: 0 };
                            GetCursorPos(&mut point);
                            SetForegroundWindow(hwnd);
                            let selected = TrackPopupMenu(
                                menu,
                                TPM_RETURNCMD | TPM_RIGHTBUTTON,
                                point.x,
                                point.y,
                                0,
                                hwnd,
                                std::ptr::null(),
                            ) as usize;
                            DestroyMenu(menu);
                            PostMessageW(hwnd, WM_NULL, 0, 0);
                            if let Some(event) = menu_event(selected) {
                                let _ = state.sender.try_send(event);
                            }
                        }
                    }
                    _ => {}
                }
                0
            }
            WM_CLOSE => {
                DestroyWindow(hwnd);
                0
            }
            WM_DESTROY => {
                Shell_NotifyIconW(NIM_DELETE, &state.icon);
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}
fn menu_event(command: usize) -> Option<Event> {
    match command {
        OPEN => Some(Event::Open),
        EXIT => Some(Event::Exit),
        _ => None,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dismissed_menu_does_not_exit() {
        assert_eq!(menu_event(0), None);
        assert_eq!(menu_event(OPEN), Some(Event::Open));
        assert_eq!(menu_event(EXIT), Some(Event::Exit));
        assert_eq!(menu_event(42), None);
    }
}

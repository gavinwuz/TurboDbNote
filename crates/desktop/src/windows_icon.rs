//! Set both HWND icons explicitly, including consumers that query ICON_SMALL.
use gpui::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::{
    System::LibraryLoader::GetModuleHandleW,
    UI::WindowsAndMessaging::{
        GetSystemMetrics, ICON_BIG, ICON_SMALL, IMAGE_ICON, LR_SHARED, LoadImageW, SM_CXICON,
        SM_CXSMICON, SM_CYICON, SM_CYSMICON, SendMessageW, WM_SETICON,
    },
};

/// Keeps the installer's named mutex alive without restricting multiple windows.
pub struct InstallGuard(windows_sys::Win32::Foundation::HANDLE);

impl Drop for InstallGuard {
    fn drop(&mut self) {
        // SAFETY: this guard owns the handle returned by CreateMutexW.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

pub fn install_guard() -> std::io::Result<InstallGuard> {
    let name: Vec<u16> = "TurboDbNote.Running\0".encode_utf16().collect();
    // SAFETY: null security attributes and a valid NUL-terminated name. We hold
    // a handle, not mutex ownership; Inno checks whether the object exists.
    let handle = unsafe {
        windows_sys::Win32::System::Threading::CreateMutexW(std::ptr::null(), 0, name.as_ptr())
    };
    if handle.is_null() {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(InstallGuard(handle))
    }
}

pub fn install(window: &Window) -> Result<(), Box<dyn std::error::Error>> {
    let raw = HasWindowHandle::window_handle(window)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let RawWindowHandle::Win32(handle) = raw.as_raw() else {
        return Ok(());
    };
    // SAFETY: GPUI supplies a live HWND on its UI thread. Resource 1 is embedded
    // by build.rs. LR_SHARED icons belong to the module and must not be destroyed.
    unsafe {
        let module = GetModuleHandleW(std::ptr::null());
        if module.is_null() {
            return Err(std::io::Error::last_os_error().into());
        }
        for (kind, width, height) in [
            (ICON_SMALL, SM_CXSMICON, SM_CYSMICON),
            (ICON_BIG, SM_CXICON, SM_CYICON),
        ] {
            let icon = LoadImageW(
                module,
                // MAKEINTRESOURCEW(1): an integer resource ID, never dereferenced.
                std::ptr::without_provenance(1),
                IMAGE_ICON,
                GetSystemMetrics(width),
                GetSystemMetrics(height),
                LR_SHARED,
            );
            if icon.is_null() {
                return Err(std::io::Error::last_os_error().into());
            }
            SendMessageW(
                handle.hwnd.get() as _,
                WM_SETICON,
                kind as usize,
                icon as isize,
            );
        }
    }
    Ok(())
}

"""Launch a fresh hidden smoke-test instance and query both HWND icon slots."""
import ctypes as c
from ctypes import wintypes as w
from pathlib import Path
import subprocess
import time
import sys

user = c.WinDLL("user32", use_last_error=True)
callback_type = c.WINFUNCTYPE(w.BOOL, w.HWND, w.LPARAM)
user.EnumWindows.argtypes = [callback_type, w.LPARAM]
user.GetWindowThreadProcessId.argtypes = [w.HWND, c.POINTER(w.DWORD)]
user.GetWindowTextW.argtypes = [w.HWND, w.LPWSTR, c.c_int]
user.SendMessageW.argtypes = [w.HWND, w.UINT, w.WPARAM, w.LPARAM]
user.SendMessageW.restype = c.c_ssize_t
user.PostMessageW.argtypes = [w.HWND, w.UINT, w.WPARAM, w.LPARAM]
user.GetWindowRect.argtypes = [w.HWND, c.POINTER(w.RECT)]
user.ClientToScreen.argtypes = [w.HWND, c.POINTER(w.POINT)]
user.IsZoomed.argtypes = [w.HWND]
startup = subprocess.STARTUPINFO()
startup.dwFlags = subprocess.STARTF_USESHOWWINDOW
startup.wShowWindow = 0
app = subprocess.Popen([str(Path("target/debug/turbodbnote.exe").resolve())],
                       startupinfo=startup, stdout=subprocess.DEVNULL,
                       stderr=subprocess.DEVNULL)
found = []


@callback_type
def visit(hwnd, _):
    owner = w.DWORD()
    user.GetWindowThreadProcessId(hwnd, c.byref(owner))
    text = c.create_unicode_buffer(256)
    user.GetWindowTextW(hwnd, text, 256)
    if owner.value == app.pid and text.value == "TurboDbNote":
        found.append(hwnd)
    return True


try:
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline and app.poll() is None:
        user.EnumWindows(visit, 0)
        if found:
            break
        time.sleep(0.2)
    assert found, "No TurboDbNote window detected within 20 seconds"
    for kind in (0, 1):  # ICON_SMALL, ICON_BIG
        icon = user.SendMessageW(found[0], 0x007F, kind, 0)  # WM_GETICON
        assert icon, f"Missing window icon slot {kind}"
    print("PASS: live TurboDbNote HWND returns both ICON_SMALL and ICON_BIG")
    if "--unified-titlebar" in sys.argv:
        rect, origin = w.RECT(), w.POINT(0, 0)
        assert user.GetWindowRect(found[0], c.byref(rect))
        assert user.ClientToScreen(found[0], c.byref(origin))
        # A native caption adds tens of pixels above the client area. Custom
        # decoration retains only the thin resize border (DPI-dependent).
        assert origin.y - rect.top <= 12, "Native caption still consumes a separate row"
        user.SendMessageW(found[0], 0x0112, 0xF030, 0)  # SC_MAXIMIZE
        time.sleep(0.2)
        assert user.IsZoomed(found[0]), "Maximize failed"
        user.SendMessageW(found[0], 0x0112, 0xF120, 0)  # SC_RESTORE
        time.sleep(0.2)
        assert not user.IsZoomed(found[0]), "Restore failed"
        print("PASS: no separate native caption; maximize/restore work")
finally:
    if app.poll() is None:
        if found:
            user.PostMessageW(found[0], 0x0010, 0, 0)
        try:
            app.wait(timeout=5)
        except subprocess.TimeoutExpired:
            # Only this script's fresh, untouched instance may be terminated.
            app.terminate()
            app.wait(timeout=5)

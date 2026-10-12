"""Open the packaged Windows executable with isolated data and real diagnostics off."""
import argparse
import ctypes
from ctypes import wintypes
import json
import pathlib
import shutil
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser()
parser.add_argument("package", type=pathlib.Path)
package = parser.parse_args().package.resolve()
user = ctypes.WinDLL("user32")
callback = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
user.EnumWindows.argtypes = [callback, wintypes.LPARAM]
user.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
user.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
user.SetWindowPos.argtypes = [wintypes.HWND, wintypes.HWND, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, wintypes.UINT]
with tempfile.TemporaryDirectory(prefix="turbodbnote-package-") as directory:
    sandbox = pathlib.Path(directory)
    shutil.copy2(package / "turbodbnote.exe", sandbox / "turbodbnote.exe")
    (sandbox / "portable.flag").write_text("Isolated package startup")
    (sandbox / "crashsight.local.json").write_text(json.dumps({"enabled": False}))
    (sandbox / "data").mkdir()
    (sandbox / "data/settings.json").write_text(json.dumps({"ai": {"codex": {"auto_connect": False}, "claude": {"auto_connect": False}}}))
    startup = subprocess.STARTUPINFO()
    startup.dwFlags = subprocess.STARTF_USESHOWWINDOW
    startup.wShowWindow = 0
    with (sandbox / "stderr.log").open("w") as log:
        app = subprocess.Popen([str(sandbox / "turbodbnote.exe")], cwd=sandbox, stderr=log, stdout=subprocess.DEVNULL, startupinfo=startup)
        owned = []
        @callback
        def visit(hwnd, _):
            pid = wintypes.DWORD()
            user.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
            rect = wintypes.RECT()
            user.GetWindowRect(hwnd, ctypes.byref(rect))
            if pid.value == app.pid and (rect.right - rect.left) * (rect.bottom - rect.top) > 300000:
                owned.append(hwnd)
            return True
        try:
            for _ in range(100):
                if app.poll() is not None:
                    raise RuntimeError("Packaged application exited before opening a window")
                owned.clear()
                user.EnumWindows(visit, 0)
                if owned:
                    break
                time.sleep(.2)
            if not owned:
                raise RuntimeError("No packaged Windows main window")
            user.SetWindowPos(owned[0], None, -20000, -20000, 1440, 960, 0x54)
            time.sleep(2)
            if app.poll() is not None:
                raise RuntimeError("Packaged window failed after initialization")
            print("Packaged Windows GUI opened with isolated data and diagnostics disabled")
        finally:
            if app.poll() is None:
                app.terminate()
                app.wait(timeout=10)

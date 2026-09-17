"""GUI startup regression using a local fake Codex; never contacts an AI service."""
import ctypes as c
from ctypes import wintypes as w
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import time

source = Path("target/debug/turbodbnote.exe").resolve()
user = c.WinDLL("user32", use_last_error=True)
callback_type = c.WINFUNCTYPE(w.BOOL, w.HWND, w.LPARAM)
user.EnumWindows.argtypes = [callback_type, w.LPARAM]
user.GetWindowThreadProcessId.argtypes = [w.HWND, c.POINTER(w.DWORD)]
user.GetWindowTextW.argtypes = [w.HWND, w.LPWSTR, c.c_int]
user.PostMessageW.argtypes = [w.HWND, w.UINT, w.WPARAM, w.LPARAM]

fixture_source = r'''
use std::io::{BufRead, Write};
fn main() {
    if std::env::args().any(|a|a=="--version") {println!("codex-cli 0.0.0-fixture");return;}
    for line in std::io::stdin().lock().lines() {
        let line=line.unwrap();
        let method=if line.contains("\"method\":\"initialize\""){"initialize"}
            else if line.contains("\"method\":\"initialized\""){"initialized"}
            else if line.contains("account/read"){"account/read"}
            else if line.contains("model/list"){"model/list"}
            else {"UNEXPECTED"};
        let mut log=std::fs::OpenOptions::new().create(true).append(true).open("wire.log").unwrap();
        writeln!(log,"{}",method).unwrap();
        if method=="initialized" {continue;}
        let id=line.split("\"id\":").nth(1).unwrap().trim_start().chars().take_while(|c|c.is_ascii_digit()).collect::<String>();
        if std::path::Path::new("fail.flag").exists() {
            println!("{{\"id\":{},\"error\":{{\"code\":-1,\"message\":\"fixture failure\"}}}}",id);
        } else {
            let result=match method {
                "initialize"=>"{}",
                "account/read"=>"{\"account\":{\"type\":\"chatgpt\"},\"requiresOpenaiAuth\":true}",
                "model/list"=>"{\"data\":[{\"id\":\"test\",\"model\":\"test\",\"displayName\":\"Fixture\",\"isDefault\":true}],\"nextCursor\":null}",
                _=>panic!("Automatic startup must not send a model request"),
            };
            println!("{{\"id\":{},\"result\":{}}}",id,result);
        }
        std::io::stdout().flush().unwrap();
    }
}
'''

with tempfile.TemporaryDirectory(prefix="auto-connect-", dir="target") as folder:
    root = Path(folder).resolve()
    (root / "fixture.rs").write_text(fixture_source, encoding="utf-8")
    subprocess.run(["rustc", "--crate-name", "codex_fixture", str(root / "fixture.rs"), "-o", str(root / "codex.exe")], check=True)
    shutil.copy2(source, root / "turbodbnote.exe")
    (root / "portable.flag").write_text("test")
    (root / "data").mkdir()
    history = {"version": 1, "engine": "codex", "messages": [
        {"item": "old-question", "text": "Previous question", "user": True},
        {"item": "old-answer", "text": "Partial reply", "user": False}],
        "thread": "fixture-thread", "executable": str(root / "codex.exe"), "cwd": str(root),
        "model": "test", "effort": None, "interrupted": True, "elapsed_seconds": 12}
    (root / "data/last-chat.json").write_text(json.dumps(history), encoding="utf-8")

    for case, enabled, fail in [("first-open", True, False), ("restart", True, False),
                                 ("disabled", False, False), ("failure-no-loop", True, True)]:
        prefs = {"ai": {"active": "codex", "codex": {"executable": str(root / "codex.exe"),
                 "working_directory": str(root), "auto_connect": enabled, "restore_last_session": True}}}
        (root / "data/settings.json").write_text(json.dumps(prefs), encoding="utf-8")
        (root / "wire.log").write_text("")
        if fail:
            (root / "fail.flag").write_text("fail")
        startup = subprocess.STARTUPINFO()
        startup.dwFlags = subprocess.STARTF_USESHOWWINDOW
        # Visibility is the trigger under test. A fully hidden window does not
        # render ChatView, so it must not connect. Show without stealing focus.
        startup.wShowWindow = 4  # SW_SHOWNOACTIVATE
        process = subprocess.Popen([str(root / "turbodbnote.exe")], cwd=root,
                                   startupinfo=startup, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            if enabled:
                deadline = time.monotonic() + 15
                while time.monotonic() < deadline:
                    assert process.poll() is None, f"{case}: app exited"
                    lines = (root / "wire.log").read_text().splitlines()
                    if ("initialize" if fail else "model/list") in lines:
                        break
                    time.sleep(0.2)
                else:
                    raise AssertionError(f"{case}: automatic connection not observed")
            time.sleep(2)
            lines = (root / "wire.log").read_text().splitlines()
            assert lines.count("initialize") == int(enabled), (case, lines)
            assert "UNEXPECTED" not in lines, (case, lines)
            if enabled and not fail:
                assert "account/read" in lines and "model/list" in lines
            windows = []

            @callback_type
            def visit(hwnd, _):
                owner = w.DWORD()
                user.GetWindowThreadProcessId(hwnd, c.byref(owner))
                title = c.create_unicode_buffer(256)
                user.GetWindowTextW(hwnd, title, 256)
                if owner.value == process.pid and title.value == "TurboDbNote":
                    windows.append(hwnd)
                return True

            user.EnumWindows(visit, 0)
            assert windows, "No app window"
            user.PostMessageW(windows[0], 0x0010, 0, 0)
            assert process.wait(timeout=10) == 0
            restored = json.loads((root / "data/last-chat.json").read_text(encoding="utf-8"))
            assert restored["messages"] == history["messages"], "History lost or duplicated"
            assert restored["thread"] == history["thread"]
            assert restored["interrupted"] and restored["elapsed_seconds"] == 12
            print(f"PASS: {case}, history retained, no inference request")
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=5)

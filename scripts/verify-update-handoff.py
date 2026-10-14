"""Verify Windows update handoff with a harmless mock installer, never a real install.

Uses a fixed GUI verification workspace and an owned temporary update-cache directory.
The helper mode starts before GPUI/diagnostics and needs no application configuration.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import queue
import shutil
import subprocess
import sys
import tempfile
import threading
import time


PARENT = r'''
import json, os, queue, subprocess, sys, threading
request = json.loads(sys.stdin.readline())
request["parent"] = os.getpid()
helper = subprocess.Popen([sys.argv[1], "--install-verified-update"],
    stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
    creationflags=subprocess.CREATE_NO_WINDOW)
try:
    helper.stdin.write(json.dumps(request).encode())
    helper.stdin.close()
    received = queue.Queue()
    threading.Thread(target=lambda: received.put(helper.stdout.read(1)), daemon=True).start()
    try:
        ready = received.get(timeout=20)
    except queue.Empty:
        helper.terminate()
        helper.wait(timeout=10)
        ready = b""
    print(json.dumps({"ready": ready == b"\x01"}), flush=True)
    action = sys.stdin.readline().strip()
    if action == "cancel" or ready != b"\x01":
        if helper.poll() is None:
            helper.terminate()
            helper.wait(timeout=10)
finally:
    if helper.poll() is not None:
        helper.stdout.close()
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--executable", type=Path, default=Path("target/debug/turbodbnote.exe"))
    args = parser.parse_args()
    if os.name != "nt":
        raise SystemExit("This verification requires Windows")
    executable = args.executable.resolve(strict=True)
    workspace = Path(__file__).resolve().parents[1] / "target/gui-verification/updates"
    workspace.mkdir(parents=True, exist_ok=True)
    source = workspace / "installer-fixture.rs"
    source.write_text('fn main() { let path = std::env::current_exe().unwrap(); '
                      'std::fs::write(path.with_file_name("installed.marker"), '
                      'b"simulated installer started").unwrap(); }\n', encoding="utf-8")
    fixture = workspace / "installer-fixture.exe"
    subprocess.run(["rustc", "--edition=2024", str(source), "-o", str(fixture)], check=True)
    cache = Path(os.environ["LOCALAPPDATA"]) / "TurboDbNote/updates"
    cache.mkdir(parents=True, exist_ok=True)
    for action in ("exit", "cancel"):
        with tempfile.TemporaryDirectory(prefix="handoff-verification-", dir=cache) as directory:
            directory = Path(directory).resolve()
            directory.relative_to(cache.resolve())  # Validate ownership before context cleanup.
            package = directory / "TurboDBNote-v0.0.0-test-windows-x64-setup.exe"
            shutil.copy2(fixture, package)
            request = {"path": str(package), "sha256": hashlib.sha256(package.read_bytes()).hexdigest()}
            parent = subprocess.Popen([sys.executable, "-c", PARENT, str(executable)],
                                      stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                      stderr=subprocess.DEVNULL, text=True,
                                      creationflags=subprocess.CREATE_NO_WINDOW)
            try:
                parent.stdin.write(json.dumps(request) + "\n")
                parent.stdin.flush()
                received = queue.Queue()
                threading.Thread(target=lambda: received.put(parent.stdout.readline()), daemon=True).start()
                ready = json.loads(received.get(timeout=30))
                assert ready["ready"], "Helper did not verify and lock the fixture"
                assert not package.with_name("installed.marker").exists(), "Installer started before parent exit"
                try:
                    with package.open("wb"):
                        raise AssertionError("Verified package was not protected from modification")
                except PermissionError:
                    pass
                parent.stdin.write(action + "\n")
                parent.stdin.flush()
                parent.wait(timeout=10)
                assert parent.returncode == 0, "Mock parent failed"
                marker = package.with_name("installed.marker")
                deadline = time.monotonic() + (10 if action == "exit" else 1)
                while time.monotonic() < deadline and not marker.exists():
                    time.sleep(.05)
                assert marker.exists() == (action == "exit"), "Incorrect installer launch/cancellation behavior"
                print(f"PASS: mock update handoff ({action}); parent wait and package lock verified")
            finally:
                if parent.poll() is None:
                    parent.stdin.write("cancel\n")
                    parent.stdin.flush()
                    try:
                        parent.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        parent.terminate()
                        parent.wait(timeout=10)
                parent.stdin.close()
                parent.stdout.close()


if __name__ == "__main__":
    main()

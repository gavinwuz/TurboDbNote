"""Start the packaged Linux window with isolated data and diagnostics disabled."""
import argparse
import json
import pathlib
import shutil
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser()
parser.add_argument("package", type=pathlib.Path)
package = parser.parse_args().package.resolve()
with tempfile.TemporaryDirectory(prefix="turbodbnote-gui-") as directory:
    sandbox = pathlib.Path(directory)
    shutil.copy2(package / "turbodbnote", sandbox / "turbodbnote")
    (sandbox / "portable.flag").write_text("Isolated GUI verification")
    (sandbox / "crashsight.local.json").write_text(json.dumps({"enabled": False}))
    (sandbox / "data").mkdir()
    (sandbox / "data/settings.json").write_text(json.dumps({"ai": {"codex": {"auto_connect": False}, "claude": {"auto_connect": False}}}))
    with (sandbox / "stderr.log").open("w") as log:
        app = subprocess.Popen([str(sandbox / "turbodbnote")], cwd=sandbox, stdout=subprocess.DEVNULL, stderr=log)
        try:
            found = False
            for _ in range(100):
                if app.poll() is not None:
                    raise RuntimeError("Packaged application exited before opening a window")
                tree = subprocess.check_output(["xwininfo", "-root", "-tree"], text=True)
                if '"TurboDBNote"' in tree:
                    found = True
                    break
                time.sleep(.2)
            if not found:
                raise RuntimeError("No TurboDBNote X11 window was created")
            time.sleep(2)
            if app.poll() is not None:
                raise RuntimeError("Packaged window failed after initialization")
            print("Packaged Linux GUI opened successfully with isolated data (X11/Xvfb)")
        finally:
            if app.poll() is None:
                app.terminate()
                try:
                    app.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    app.kill()
                    app.wait(timeout=5)

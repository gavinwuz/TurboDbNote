#!/usr/bin/env python3
"""Build the Linux archive from the locked workspace; never include user configuration."""
import hashlib
import json
import pathlib
import re
import shutil
import subprocess
import sys
import tarfile
import tomllib

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
from release import notes, check


def run(*args):
    if args[0] == "cargo":
        args = (sys.executable, str(ROOT / "scripts/run-cargo.py"), *args[1:])
    subprocess.run(args, cwd=ROOT, check=True)


def main():
    if sys.platform != "linux":
        raise SystemExit("Linux packaging requires Linux")
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    check(version)
    run("cargo", "build", "--locked", "--release", "-j", "1", "-p", "turbodbnote", "-p", "turbodbnote-driver-host", "--bins")
    print("::notice title=Linux packaging::Native Release binaries built successfully", flush=True)
    name = f"TurboDBNote-v{version}-linux-x64"
    output = ROOT / "dist"
    stage = output / name
    archive = output / f"{name}.tar.gz"
    if stage.exists() or archive.exists():
        raise SystemExit("Refusing to overwrite an existing Linux package")
    stage.mkdir(parents=True)
    (stage / "drivers/native").mkdir(parents=True)
    shutil.copy2(ROOT / "target/release/turbodbnote", stage / "turbodbnote")
    shutil.copy2(ROOT / "target/release/turbodbnote-driver-host", stage / "drivers/native/turbodbnote-driver-host")
    (stage / "turbodbnote").chmod(0o755)
    (stage / "drivers/native/turbodbnote-driver-host").chmod(0o755)
    for binary in [stage / "turbodbnote", stage / "drivers/native/turbodbnote-driver-host"]:
        dependencies = subprocess.check_output(["ldd", str(binary)], text=True)
        if "not found" in dependencies:
            raise SystemExit("A packaged binary has unresolved runtime dependencies")
        (stage / (binary.name + "-dependencies.txt")).write_text(dependencies)
    print("::notice title=Linux packaging::Runtime dependency checks passed", flush=True)
    for filename in ["LICENSE", "README.md", "README.zh-CN.md"]:
        shutil.copy2(ROOT / filename, stage / filename)
    (stage / "RELEASE-NOTES.md").write_text(notes(version, "en"), encoding="utf-8")
    (stage / "RELEASE-NOTES.zh-CN.md").write_text(notes(version, "zh-CN"), encoding="utf-8")
    (stage / "README.txt").write_text("Run ./turbodbnote after extraction. Linux x64, Ubuntu 22.04/glibc 2.35 baseline.\nRequires a working X11/Wayland session, Vulkan graphics, libxkbcommon, fontconfig and unixODBC.\nPasswords are session-only on Linux. Windows tray and bundled legacy MySQL DLL are Windows-only.\nApplication data uses the normal per-user config directory. AI requires a separately installed CLI.\nSee RELEASE-NOTES.md and the included dependency inventories.\n", encoding="utf-8")
    (stage / "BUILD-INFO.json").write_text(json.dumps(dict(version=version, commit=subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(), target="x86_64-unknown-linux-gnu"), indent=2))
    run("git", "archive", "--format=tar.gz", "--output=" + str(stage / "sources.tar.gz"), "HEAD", "Cargo.toml", "Cargo.lock", "crates", "scripts", "LICENSE", "README.md", "README.zh-CN.md", "CHANGELOG.md", "CHANGELOG.zh-CN.md")
    print("::notice title=Linux packaging::Source archive created", flush=True)
    with tarfile.open(archive, "w:gz") as tar:
        tar.add(stage, arcname=stage.name)
    with archive.open("rb") as source:
        digest = hashlib.file_digest(source, "sha256").hexdigest()
    (output / "SHA256SUMS-linux.txt").write_text(f"{digest}  {archive.name}\n", encoding="ascii")
    print(archive.name)


if __name__ == "__main__":
    try:
        main()
    except (Exception, SystemExit) as error:
        message = str(error)
        if re.search(r"(?i)(password|secret|token|api[_-]?key|authorization)\s*[:=]", message):
            message = "Sensitive diagnostic values omitted"
        message = (type(error).__name__ + ": " + message)[:600]
        message = message.replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")
        print("::error title=Linux packaging::" + message, flush=True)
        raise

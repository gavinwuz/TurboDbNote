#!/usr/bin/env python3
"""Prepare a clean default-branch release and trigger the dual-platform pipeline."""
import argparse
import datetime
import json
import pathlib
import re
import subprocess
import sys
import tomllib

ROOT = pathlib.Path(__file__).resolve().parents[1]
OWN_PACKAGES = {"turbodbn-core", "turbodbn-services", "turbodbn-diagnostics", "turbodbnote", "turbodbnote-driver-host"}


def git(*args):
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()


def replace_version(text, old, version):
    return re.sub(r'(\[workspace\.package\]\s*version\s*=\s*")' + re.escape(old) + r'(")', lambda m: m[1] + version + m[2], text, count=1)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("version", help="New x.y.z version; prepare entries in both Unreleased sections first")
    args = parser.parse_args()
    version = args.version
    if not re.fullmatch(r"\d+\.\d+\.\d+", version):
        raise SystemExit("Version must be x.y.z")
    if git("status", "--porcelain"):
        raise SystemExit("Commit work first; release preparation requires a clean worktree")
    refs = git("ls-remote", "--symref", "origin", "HEAD")
    default = next(line.split()[1].removeprefix("refs/heads/") for line in refs.splitlines() if line.startswith("ref:"))
    if git("branch", "--show-current") != default:
        raise SystemExit(f"Switch to the default branch ({default}) before preparing a release")
    remote_head = next(line.split()[0] for line in refs.splitlines() if line.endswith("\tHEAD") and not line.startswith("ref:"))
    if git("rev-parse", "HEAD") != remote_head:
        raise SystemExit("Default branch must match origin; push or reconcile existing commits first")
    old = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    if tuple(map(int, version.split('.'))) <= tuple(map(int, old.split('.'))):
        raise SystemExit("Version must increase; published versions are immutable")
    if git("ls-remote", "--tags", "origin", f"refs/tags/v{version}"):
        raise SystemExit("Version tag already exists")
    date = datetime.datetime.now(datetime.timezone(datetime.timedelta(hours=8))).date().isoformat()
    edits = {}
    for filename in ["CHANGELOG.md", "CHANGELOG.zh-CN.md"]:
        p = ROOT / filename
        text = p.read_text(encoding="utf-8")
        match = re.search(r"^## \[Unreleased\]\s*\n(.*?)(?=^## |\Z)", text, re.MULTILINE | re.DOTALL)
        if not match or not match[1].strip():
            raise SystemExit(f"Add release notes to Unreleased in {filename}")
        edits[p] = text[:match.start()] + f"## [Unreleased]\n\n## [{version}] - {date}\n\n{match[1].strip()}\n\n" + text[match.end():]
    manifest = ROOT / "Cargo.toml"
    edits[manifest] = replace_version(manifest.read_text(), old, version)
    lock = ROOT / "Cargo.lock"
    blocks = lock.read_text().split("[[package]]")
    for i, block in enumerate(blocks):
        name = re.search(r'^name = "([^"]+)"', block, re.MULTILINE)
        if name and name[1] in OWN_PACKAGES:
            blocks[i] = re.sub(r'^version = "' + re.escape(old) + '"', f'version = "{version}"', block, count=1, flags=re.MULTILINE)
    edits[lock] = "[[package]]".join(blocks)
    resource = ROOT / "crates/desktop/assets/windows.rc"
    rc = resource.read_text()
    for key in ["FILEVERSION", "PRODUCTVERSION"]:
        rc = re.sub(rf'{key} \d+,\d+,\d+,\d+', key + ' ' + version.replace('.', ',') + ',0', rc)
    rc = rc.replace(f'"{old}\\0"', f'"{version}\\0"')
    edits[resource] = rc
    metadata = ROOT / "crates/desktop/assets/release.json"
    edits[metadata] = json.dumps(dict(version=version, channel="stable", released_on=date), indent=2) + "\n"
    for filename in ["README.md", "README.zh-CN.md"]:
        p = ROOT / filename
        edits[p] = p.read_text(encoding="utf-8").replace(f'**v{old}**', f'**v{version}**', 1).replace(f'**Version {old} ·', f'**Version {version} ·', 1).replace(f'**版本 {old} ·', f'**版本 {version} ·', 1)
    for path, text in edits.items():
        path.write_text(text, encoding="utf-8")
    subprocess.run([sys.executable, "scripts/release.py", "--version", version], cwd=ROOT, check=True)
    subprocess.run(["git", "add", *[str(p.relative_to(ROOT)) for p in edits]], cwd=ROOT, check=True)
    subprocess.run(["git", "commit", "-m", f"chore: prepare v{version} release [release]"], cwd=ROOT, check=True)
    subprocess.run(["git", "push", "origin", f"HEAD:refs/heads/{default}"], cwd=ROOT, check=True)
    print("Dual-platform validation and publication triggered. No version tag is created until both packages pass.")
    print("https://github.com/gavinwuz/TurboDbNote/actions/workflows/release.yml")


if __name__ == "__main__":
    main()

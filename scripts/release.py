#!/usr/bin/env python3
"""Validate versions and generate release notes from the two root changelogs."""
import argparse
import datetime
import json
import pathlib
import re
import tomllib

ROOT = pathlib.Path(__file__).resolve().parents[1]


def notes(version, language):
    path = ROOT / ("CHANGELOG.md" if language == "en" else "CHANGELOG.zh-CN.md")
    text = path.read_text(encoding="utf-8")
    pattern = rf"^## \[{re.escape(version)}\][^\n]*\n(.*?)(?=^## |\Z)"
    matches = re.findall(pattern, text, re.MULTILINE | re.DOTALL)
    if len(matches) != 1 or not matches[0].strip():
        raise ValueError("Expected one nonempty changelog section for this version")
    return f"# TurboDBNote v{version}\n\n{matches[0].strip()}\n"


def check(version):
    if not re.fullmatch(r"\d+\.\d+\.\d+", version):
        raise ValueError("Version must be x.y.z")
    actual = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    release = json.loads((ROOT / "crates/desktop/assets/release.json").read_text())
    if actual != version or release["version"] != version or release["channel"] != "stable":
        raise ValueError("Cargo and stable release metadata must match the requested version")
    datetime.date.fromisoformat(release["released_on"])
    own = {"turbodbn-core", "turbodbn-services", "turbodbn-diagnostics", "turbodbnote", "turbodbnote-driver-host"}
    locked = {p["name"]: p["version"] for p in tomllib.loads((ROOT / "Cargo.lock").read_text())["package"] if p["name"] in own}
    if set(locked) != own or any(v != version for v in locked.values()):
        raise ValueError("Workspace package versions in Cargo.lock must match")
    resource = (ROOT / "crates/desktop/assets/windows.rc").read_text()
    numeric = version.replace(".", ",") + ",0"
    if any(key + " " + numeric not in resource for key in ["FILEVERSION", "PRODUCTVERSION"]):
        raise ValueError("Windows numeric version resources must match")
    if any('VALUE "' + key + '", "' + version + "\\0\"" not in resource for key in ["FileVersion", "ProductVersion"]):
        raise ValueError("Windows string version resources must match")

    for lang in ["en", "zh-CN"]:
        notes(version, lang)
    print(f"Validated v{version}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--version", required=True)
    parser.add_argument("--notes", type=pathlib.Path)
    parser.add_argument("--publication", action="store_true")
    args = parser.parse_args()
    check(args.version)
    if args.publication:
        date = json.loads((ROOT / "crates/desktop/assets/release.json").read_text())["released_on"]
        # Use the release team's calendar date, not the compiler timestamp.
        today = datetime.datetime.now(datetime.timezone(datetime.timedelta(hours=8))).date().isoformat()
        if date != today:
            raise SystemExit("Release date differs from publication day; prepare a new candidate")
    if args.notes:
        args.notes.write_text(notes(args.version, "en") + "\n---\n\n" + notes(args.version, "zh-CN"), encoding="utf-8")

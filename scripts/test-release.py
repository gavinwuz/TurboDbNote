"""Regression checks for version/notes validation without touching real release data."""
import json
import hashlib
import shutil
import subprocess
import pathlib
import tempfile
import unittest
from unittest.mock import patch
import release
import importlib.util
spec = importlib.util.spec_from_file_location("prepare_release", pathlib.Path(__file__).with_name("prepare-release.py"))
prepare_release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prepare_release)
replace_version = prepare_release.replace_version


cargo_spec = importlib.util.spec_from_file_location("run_cargo", pathlib.Path(__file__).with_name("run-cargo.py"))
run_cargo = importlib.util.module_from_spec(cargo_spec)
cargo_spec.loader.exec_module(run_cargo)


class CargoDiagnosticTests(unittest.TestCase):
    def test_colored_failures_remain_visible_without_sensitive_values(self):
        self.assertEqual(run_cargo.failure("\x1b[31merror[E0425]: unknown item\x1b[0m"), "error[E0425]: unknown item")
        self.assertEqual(run_cargo.failure("test example::case ... FAILED"), "test example::case ... FAILED")
        self.assertIsNone(run_cargo.failure('warning: unreachable expression'))
        self.assertIsNone(run_cargo.failure('  --> upstream/gpui/src/elements/surface.rs:35:21'))
        self.assertEqual(run_cargo.failure('error: could not compile application'), 'error: could not compile application')
        self.assertIsNone(run_cargo.failure('left: confidential fixture value'))
        self.assertIsNone(run_cargo.failure('authorization=confidential fixture value'))


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = pathlib.Path(self.directory.name)
        self.patch = patch.object(release, "ROOT", self.root)
        self.patch.start()
        self.addCleanup(self.patch.stop)
        (self.root / "crates/desktop/assets").mkdir(parents=True)
        (self.root / "Cargo.toml").write_text('[workspace.package]\nversion = "0.1.0"\n')
        names = ["turbodbn-core", "turbodbn-services", "turbodbn-diagnostics", "turbodbnote", "turbodbnote-driver-host"]
        (self.root / "Cargo.lock").write_text('\n'.join(f'[[package]]\nname = "{n}"\nversion = "0.1.0"\n' for n in names))
        (self.root / "crates/desktop/assets/release.json").write_text(json.dumps(dict(version="0.1.0", channel="stable", released_on="2026-10-12")))
        (self.root / "crates/desktop/assets/windows.rc").write_text('FILEVERSION 0,1,0,0\nPRODUCTVERSION 0,1,0,0\nVALUE "FileVersion", "0.1.0\\0"\nVALUE "ProductVersion", "0.1.0\\0"\n')
        for name in ["CHANGELOG.md", "CHANGELOG.zh-CN.md"]:
            (self.root / name).write_text('## [Unreleased]\n\n## [0.1.0] - 2026-10-12\n\n- Feature\n')

    def test_matching_versions_and_bilingual_notes(self):
        release.check("0.1.0")
        self.assertIn('- Feature', release.notes("0.1.0", "zh-CN"))

    def test_duplicate_version_sections_are_rejected(self):
        p = self.root / "CHANGELOG.md"
        p.write_text(p.read_text() + '\n## [0.1.0]\n- Duplicate\n')
        with self.assertRaises(ValueError):
            release.notes("0.1.0", "en")

    def test_partial_lockfile_update_blocks_release(self):
        p = self.root / "Cargo.lock"
        p.write_text(p.read_text().replace('version = "0.1.0"', 'version = "0.1.1"', 1))
        with self.assertRaises(ValueError):
            release.check("0.1.0")

    @unittest.skipUnless(shutil.which("pwsh"), "PowerShell is needed for Windows checksum generation")
    def test_windows_checksum_manifest_is_portable_to_linux(self):
        file = self.root / "sample portable.zip"
        file.write_bytes(b"release checksum fixture")
        wrapper = self.root / "checksums.ps1"
        wrapper.write_text("param([string]$Source, [string]$Directory)\n. (Join-Path $Source 'scripts/release-notes.ps1')\nWrite-ReleaseChecksums -Paths @((Join-Path $Directory 'sample portable.zip')) -OutputFile (Join-Path $Directory 'SHA256SUMS.txt')\n")
        source = pathlib.Path(__file__).resolve().parents[1]
        subprocess.run(["pwsh", "-NoProfile", "-File", str(wrapper), str(source), str(self.root)], check=True, stdout=subprocess.DEVNULL)
        content = (self.root / "SHA256SUMS.txt").read_bytes()
        self.assertNotIn(b"\r", content)
        expected = hashlib.sha256(file.read_bytes()).hexdigest() + "  sample portable.zip\n"
        self.assertEqual(content.decode("ascii"), expected)
        tool = shutil.which("sha256sum")
        fallback = pathlib.Path("C:/Program Files/Git/usr/bin/sha256sum.exe")
        if not tool and fallback.is_file():
            tool = str(fallback)
        if tool:
            subprocess.run([tool, "--check", "SHA256SUMS.txt"], cwd=self.root, check=True, stdout=subprocess.DEVNULL)

    def test_prepare_changes_only_workspace_version(self):
        text = '[workspace.package]\nversion = "0.1.0"\n[dependencies]\nexample = { version = "0.1.0" }\n'
        changed = replace_version(text, "0.1.0", "0.1.1")
        self.assertIn('version = "0.1.1"', changed)
        self.assertIn('example = { version = "0.1.0" }', changed)


if __name__ == "__main__":
    unittest.main()

"""Regression checks for version/notes validation without touching real release data."""
import json
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

    def test_prepare_changes_only_workspace_version(self):
        text = '[workspace.package]\nversion = "0.1.0"\n[dependencies]\nexample = { version = "0.1.0" }\n'
        changed = replace_version(text, "0.1.0", "0.1.1")
        self.assertIn('version = "0.1.1"', changed)
        self.assertIn('example = { version = "0.1.0" }', changed)


if __name__ == "__main__":
    unittest.main()

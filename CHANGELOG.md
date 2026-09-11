# Changelog

English | [简体中文](CHANGELOG.zh-CN.md)

Version history is maintained here. Release notes are generated from the matching version section; candidate versions are not necessarily publicly released.

## [Unreleased]

### Fixed

- Windows packaging uses explicit compiler parameters and an isolated GPUI build adapter, avoiding `Failed to find fxc.exe` without custom environment variables.

### Added

- Inno Setup EXE alongside the portable ZIP, with bilingual installation, per-user shortcuts, uninstall support and a running-application guard. Windows 7 support is out of scope.
- A unified 40px title bar combining branding, layout/theme actions and window controls.

## [0.1.0]

Preview candidate. Public release date has not been set.

### Added

- Minimal icon navigation, resizable four-region Dock layout, auxiliary tabs and focus mode.
- Dark / Light theme switching for the current session.
- In-memory SQL, Markdown and task cells, folding and task status changes.
- Versioned `.note` models, independent query state and database/AI service contracts.
- Data Note branding, Windows application icons and version metadata.
- Windows x64 portable ZIP packaging, build metadata, SHA256 checksums and candidate artifact workflow.
- English and Simplified Chinese README and changelog; generated bilingual release notes.

### Fixed

- SVG previews on white backgrounds; embedded multi-resolution EXE icons and explicit small/large window icons.
- Confirmed that launching from a different directory resolves the observed Task Manager group icon issue; old-path caching may still require restarting the application.

### Known limitations

**Edits are stored only in memory and are lost when the application closes. Do not use this preview to retain production work.**

File saving, real database execution, AI integration, full SQL completion and Markdown preview are not available. Theme selection does not persist across restarts. Automatic updates are not included. Performance targets have not undergone formal benchmark acceptance. Candidate binaries are unsigned; clean Windows VM and third-party license review remain release gates.

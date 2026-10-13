# Changelog

Release notes are generated from the version sections below.

## [Unreleased]

## [0.1.1] - 2026-10-13

- Allow modern MySQL 8.x and 9.x version families, including 8.4.5, without claiming real-server acceptance for every release.

- Publish the Windows installer and a separate source-material ZIP; exclude application and MySQL source archives from the portable ZIP.

## [0.1.0] - 2026-10-12

First public release of TurboDBNote for Windows and Linux x64.

### Added

- Native SQL notebook with independent `.note` files, recovery, projects, recent files and 600ms autosave.
- SQL cells for MySQL/PostgreSQL/SQLite, paged result grids, execution summaries, affected-row counts and guarded write confirmation.
- Markdown editing with icon/tooltip formatting tools and native undo; task checklists with assignees, deadlines, completion and local reminders.
- 16 database driver types covering SQL, analytics, NoSQL and local files; external adapters provide connection, object browsing and bounded read-only previews.
- Database object browsing, table/schema inspectors, filtering, sorting, staged edits, readable INSERT/UPDATE copy and JSON views.
- Database creation, supported-driver transfers/import/export, object selection, best-effort permission checks, character sets and MySQL collations.
- Codex and Claude Code CLI chat, independent settings/history, streaming output, stop controls and explicit SQL authorization.
- English/Simplified Chinese, themes, font previews, persistent settings and bounded structured operation logs.
- Windows tray hosting with remembered close behavior, Open/Exit and pending-save protection; Windows current-user encrypted connection passwords and bundled legacy MySQL components.
- Automated Windows portable ZIP and Linux tar.gz releases, SHA256 checksums and reusable release preparation. Windows installer and matching symbols are retained as workflow artifacts.

### Platform requirements and limits

- Windows 10 1809+ / Windows 11 x64. Windows binaries are unsigned.
- Linux x64 archive targets Ubuntu 22.04 / glibc 2.35 or newer; a working X11/Wayland session, Vulkan, fontconfig, libxkbcommon and unixODBC are required. See the included dependency inventory.
- Linux passwords remain in session memory; Windows DPAPI, tray hosting, integrated SQL Server authentication and bundled legacy MySQL DLL are Windows-only.
- External adapters do not enable arbitrary Note SQL, writes or transfers; Access/DB2/Oracle need the corresponding native provider. Doris is read-only. Redis cluster browsing is limited to the connected node.
- Task reminders apply only while the application is running and the note is open. AI requires a separately installed/authenticated CLI. Automatic updates are not included.
- CI includes isolated protocol and GUI startup checks. These are not full real-server acceptance for every database, or full desktop/GPU/Wayland acceptance for Linux. Native crash-report delivery is not verified.

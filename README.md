# TurboDBNote

[English](README.md) | [简体中文](README.zh-CN.md)

A native SQL Notebook and database workbench built with Rust and GPUI. Write SQL, Markdown and task notes alongside database browsing and AI chat.

**Version 0.1.1 · Windows and Linux x64**

[Download](https://github.com/gavinwuz/TurboDbNote/releases) · [Changelog](CHANGELOG.md) · [Release workflow](docs/releasing.md)

## Features

- **Data notes:** independent `.note` files, projects, recent files, recovery and 600ms autosave. SQL cells inherit or override their connection/database.
- **SQL results:** paged grids, JSON views, affected-row summaries, filtering, sorting, inspectors and readable INSERT/UPDATE copy. Writes require explicit review and confirmation.
- **Markdown and tasks:** icon/tooltip formatting tools, preview and undo; checklists with assignees, deadlines, completion and local reminders.
- **Database workbench:** connection management, object/schema browsing, supported-driver database creation and import/export/transfer, object selection and collation options.
- **AI assistance:** Codex and Claude Code CLI chat with independent sessions/settings, streaming output and stop controls. SQL execution/sharing requires explicit authorization.
- **Desktop:** resizable panels, Dark/Light themes, English/Simplified Chinese, font previews and structured operation logs. Windows includes remembered tray hosting with Open/Exit.

## Supported databases

16 driver types:

| Category | Databases |
| --- | --- |
| SQL / local | MySQL, PostgreSQL, SQLite, MariaDB, TiDB, SQL Server, Oracle, Microsoft Access, IBM DB2 for LUW |
| Analytics / time series | ClickHouse, Apache Doris, TDengine |
| NoSQL / search | Redis, MongoDB, Cassandra, Elasticsearch |

Capabilities vary. Note SQL execution supports MySQL/PostgreSQL/SQLite. External adapters offer connection, object browsing and bounded read-only previews; Doris is read-only. Access/DB2 require the corresponding ODBC provider; Oracle requires OCI. See [driver capabilities and prerequisites](docs/database-drivers.md).

## Install

- **Windows 10 1809+ / Windows 11 x64:** extract the portable ZIP and run `turbodbnote.exe`. Keep the bundled `drivers` and license material with the executable. Future releases provide source materials as a separate download. Windows 7 is unsupported; binaries are unsigned.
- **Linux x64:** extract the tar.gz and run `./turbodbnote`. The baseline is Ubuntu 22.04 / glibc 2.35, with a working X11/Wayland desktop, Vulkan graphics, fontconfig, libxkbcommon and unixODBC. See the included `*-dependencies.txt` files.
- Verify downloaded files using `SHA256SUMS.txt`. AI chat requires a separately installed and authenticated Codex or Claude Code CLI; it is not bundled.

Notes are saved as standalone `.note` files. Settings, navigation and connection metadata use `connections.db`. Windows portable data lives beside the executable under `data`; Linux uses the per-user configuration directory. Windows passwords use current-user DPAPI encryption; Linux passwords stay in session memory and must be re-entered after restart. Do not copy Windows encrypted credentials expecting them to work under a different user or on Linux.

Windows tray hosting, integrated SQL Server authentication and the bundled legacy MySQL DLL are Windows-only. Redis cluster browsing covers the connected node. Reminders work while the application is running and the note is open. Automatic updates are not included.

## Develop

Use Rust **1.92.0**, Cargo and the locked dependency set. GPUI, Component and assets are pinned together. Windows needs Visual Studio C++ Build Tools, Windows SDK, CMake and PowerShell 7; Linux native packages are listed in the release workflow.

```powershell
cargo run --locked -p turbodbnote
```

On Windows, `./scripts/build-preview.ps1` updates the reusable `target/preview` bundle, including driver components. Release packaging uses an isolated GPUI adapter and never patches the global Cargo registry.

```powershell
cargo fmt --all -- --check
cargo check --locked -j 1 --workspace --all-targets
cargo test --locked -j 1 --workspace -- --test-threads=1
cargo clippy --locked -j 1 --workspace --all-targets -- -D warnings
```

The serial test setting avoids Windows credential-test interference. CI checks native builds, isolated protocol contracts and packaged window startup. X11/Xvfb smoke checks do not imply full Linux desktop/GPU/Wayland acceptance; not every database adapter has real-server acceptance. Native crash-report delivery is not verified.

## Documentation

- [Note workbench and recovery](docs/note-workbench.md)
- [Database transfers](docs/database-transfer.md)
- [Settings and common actions](docs/settings-workbench.md)
- [Database driver scope](docs/database-drivers.md)
- [Diagnostics and verification boundaries](docs/crashsight-integration.md)
- [Release preparation and packaging](docs/releasing.md)

For the next release, add bilingual Unreleased notes, commit features on the clean default branch and run `./scripts/release.ps1 -Version 0.1.1` (Linux: `python3 scripts/prepare-release.py 0.1.1`). Actions tests, packages and publishes only after both platforms pass. Version tags are immutable; existing releases are not overwritten.

Licensed under [Apache-2.0](LICENSE). Bundled third-party components retain their respective licenses.

Future Windows releases include an Inno Setup installer alongside the portable ZIP and SHA256 checksums. See [release instructions](docs/releasing.md). Current binaries remain unsigned.

[Privacy Policy](PRIVACY.md)

Modern MySQL version routing accepts 5.6, 5.7, 8.x and 9.x (including 8.4.5). Connection eligibility does not imply real-server acceptance for every release; authentication and TLS checks remain enforced.

# TurboDBNote

[English](README.md) | [简体中文](README.zh-CN.md)

A native SQL Notebook and database workbench built with Rust and GPUI. Write SQL, Markdown and task notes alongside database browsing and AI chat.

**Version 0.1.2 · Windows and Linux x64**

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

Capabilities vary. Note SQL execution supports MySQL/PostgreSQL/SQLite. External adapters offer connection, object browsing and bounded previews; MongoDB additionally supports explicitly enabled, confirmed document writes. Doris is read-only. Access/DB2 require the corresponding ODBC provider; Oracle requires OCI. See [driver capabilities and prerequisites](docs/database-drivers.md).

MySQL database detection reads binary-flagged UTF-8 metadata without treating encoding failures as permission failures. Notes support bounded `SHOW DATABASES`/`SHOW SCHEMAS`, plus literal `LIKE` filters for `SHOW TABLES`/`SHOW STATUS`/`SHOW VARIABLES`; STATUS/VARIABLES also accept SESSION/GLOBAL scope. These commands honor Settings → Database → SQL permissions and do not change server grants.

## Install

- **Windows 10 1809+ / Windows 11 x64:** extract the portable ZIP and run `turbodbnote.exe`. Keep the bundled `drivers` and license material with the executable. Future releases provide source materials as a separate download. Windows 7 is unsupported; binaries are unsigned.
- **Linux x64:** extract the tar.gz and run `./turbodbnote`. The baseline is Ubuntu 22.04 / glibc 2.35, with a working X11/Wayland desktop, Vulkan graphics, fontconfig, libxkbcommon and unixODBC. See the included `*-dependencies.txt` files.
- Verify downloaded files using `SHA256SUMS.txt`. AI chat requires a separately installed and authenticated Codex or Claude Code CLI; it is not bundled.

Notes are saved as standalone `.note` files. Settings, navigation and connection metadata use `connections.db`. Windows portable data lives beside the executable under `data`; Linux uses the per-user configuration directory. Windows passwords use current-user DPAPI encryption; Linux passwords stay in session memory and must be re-entered after restart. Do not copy Windows encrypted credentials expecting them to work under a different user or on Linux.

Windows tray hosting, integrated SQL Server authentication and the bundled legacy MySQL DLL are Windows-only. Redis cluster browsing covers the connected node. Reminders work while the application is running and the note is open. Stable releases are checked in the background at startup, at most once per 24 hours; Help → Check for updates always checks immediately. New versions offer release notes, skip/later actions and an explicit in-app download with progress, cancellation and SHA256 verification. Windows installer builds can save pending Notes/settings and exit before launching the verified installer; resolve connection drafts and staged edits or running database/Note operations first. Portable Windows and Linux packages are downloaded for manual replacement. Check/download events appear in Output. SHA256 checks integrity, not an independent release signature; installation is never automatic.

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

For the next release, add bilingual Unreleased notes, commit features on the clean default branch and run `./scripts/release.ps1 -Version 0.1.1` (Linux: `cargo run --quiet --locked -j 1 -p turbodbnote-tools -- prepare-release 0.1.1`). Actions tests, packages and publishes only after both platforms pass. Version tags are immutable; existing releases are not overwritten.

Licensed under [Apache-2.0](LICENSE). Bundled third-party components retain their respective licenses.

Future Windows releases include an Inno Setup installer alongside the portable ZIP and SHA256 checksums. See [release instructions](docs/releasing.md). Current binaries remain unsigned.

[Privacy Policy](PRIVACY.md)

Modern MySQL version routing accepts 5.6, 5.7, 8.x and 9.x (including 8.4.5). Connection eligibility does not imply real-server acceptance for every release; authentication and TLS checks remain enforced.

SQL import is available in the Note toolbar: one managed script cell with a virtual, paged statement list. UTF-8 files up to 512 MiB / 1,000,000 statements are indexed on disk; supported statements execute sequentially with one explicit frozen-script authorization. Small scripts can optionally become ordinary SQL cells. Copy the sibling `.turbodbnote-scripts` directory when moving a Note manually. See [Note workbench](docs/note-workbench.md) and [native developer tools](docs/native-tooling.md).

Note MySQL script execution supports a single database, bounded UTF-8 session settings, common dump save/restore variables, version comments, table locks and transactions. The frozen script and its risk summary are authorized once before execution; database mismatches require explicit target selection. Script statements are bounded to 1 MiB, while ordinary SQL cells retain 60 KB.

SQL file exports create ordinary tables only if missing (`IF NOT EXISTS`); existing structures are not compared or changed, and data statements retain their original behavior.

Script cells show cumulative execution statistics and the latest 500 statement details, retain the latest genuine query result, and read up to 512 fields for a selected table on demand. Unknown counts are explicit; affected rows are not committed totals. MySQL conditional creation uses server diagnostics to distinguish new and skipped tables.

Driver settings support version-matched downloads, trusted offline ZIP imports, dependency probes and persisted enable switches. Releases provide full/minimal distributions and reusable offline component packages; external drivers use Rust worker processes, with OCI/ODBC prerequisites where applicable.

Connection URLs can be pasted one per line (up to 100 / 64 KiB), previewed and opened as independent editable drafts without connecting. Supported schemes: MySQL/MariaDB, PostgreSQL, Redis/rediss, ClickHouse and single-host MongoDB. Passwords are masked and remain in memory until an explicit save; unsupported parameters prevent that row from being imported. New MongoDB forms use the default database for authentication unless overridden under Advanced; imported URLs and existing records retain their explicit authentication source.

MongoDB collections now support typed top-level field columns and a linked virtual document tree. Objects/arrays use bounded summaries; copying and CSV export retain complete canonical EJSON values. Node menus provide copying, lazy expansion, staged edits and clone drafts. MongoDB connections remain read-only by default; explicitly disable read-only under Advanced, save and reconnect to enable confirmed single-document updates/clones. Updates compare the original document and never automatically retry. Shared data grids use a drag guide and apply column width once on drop; numeric/fit/reset actions are in column menus and widths are stored locally.

Connection URL help provides masked examples and supports MySQL `@tcp(host:port)` DSN-style URLs. Go-specific `interpolateParams`, `parseTime` and `loc` options are explained before importing; such rows are unselected until the user explicitly accepts TurboDBNote behavior. Settings uses normal tab navigation; driver readiness details and missing prerequisites are collapsible, and enablement uses switches.

Generic ODBC now supports existing x64 DSNs or installed driver names with validated non-secret parameters, read-only catalogs/columns and bounded data previews. Windows full/component packages include the official offline Db2 CLI archive and manual registration instructions; no driver is registered automatically. See [database drivers](docs/database-drivers.md).

Imported MySQL scripts can execute `DROP TRIGGER [IF EXISTS]` after enabling the script-only permission and explicitly authorizing the batch. Trigger metadata is rechecked before execution; ordinary SQL cells remain unchanged.

MySQL scripts additionally support transaction isolation and bounded SET user-variable / SELECT INTO forms; locking SELECT uses the script lock permission. These remain unavailable to ordinary SQL cells.

Note connection selectors show environment, driver and address. Each open Note has independent Read only / Read-write / DBA restrictions beneath global SQL settings. Script previews separate leading comments and actual SQL; failures offer explicit, read-only MySQL diagnostics without changing server policy.

Connection timeout and per-operation timeout are independent (existing connection value retained; execution defaults to 300 seconds). Note Read/write permits INSERT/UPDATE; DELETE requires DBA. Elapsed UI values adapt between ms, seconds and minutes; journals retain milliseconds. External components now require protocol 5.

Global default timeouts are available under Settings → Database → Query. New connections inherit them; existing custom values are retained. Note permission messages provide scoped actions to switch mode, open global SQL settings or choose a writable connection, without running SQL automatically.

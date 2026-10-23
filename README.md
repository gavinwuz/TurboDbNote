# TurboDBNote

[English](README.md) | [简体中文](README.zh-CN.md)

Merged-tree filtering searches only connected targets’ cached tables, views and indexes. Pending or failed searches retain the previous tree; uncached scopes have a separate cache-building entry. Disconnected targets are excluded even when their snapshots remain on disk, and typing or local pagination never connects or reads remote metadata.

The single-pane database tree persists target-scoped directory snapshots in connections.db. MySQL database names and lazily opened database object directories have no fixed total-entry cutoff; remote batches and local result pages remain bounded. Object-name filtering uses local SQLite across indexed, connected scopes. Expand a database to build missing caches; refresh the connection’s names or hover a database to refresh its cached objects. Interrupted refreshes retain the previous complete snapshot. Note editors size from visual lines without trailing half-screen padding, and the outer scrollbar navigates the entire file.

A native SQL Notebook and database workbench built with Rust and GPUI. Write SQL, Markdown and task notes alongside database browsing and AI chat.

**Version 0.1.4 · Windows and Linux x64**

[Download](https://github.com/gavinwuz/TurboDbNote/releases) · [Changelog](CHANGELOG.md) · [Release workflow](docs/releasing.md)

## Features

- **Data notes:** independent `.note` files, projects, recent files, recovery and 600ms autosave. SQL cells inherit or override their connection/database.
- **SQL results:** paged grids, JSON views, affected-row summaries, filtering, sorting, inspectors and readable INSERT/UPDATE copy. Writes require explicit review and confirmation.
- **Markdown and tasks:** icon/tooltip formatting tools, preview and undo; checklists with assignees, deadlines, completion and local reminders.
- **Database workbench:** connection management, object/schema browsing, supported-driver database creation and import/export/transfer, object selection and collation options.
- **AI assistance:** Codex and Claude Code CLI chat with independent sessions/settings, streaming output and stop controls. SQL execution/sharing requires explicit authorization.
- **Desktop:** resizable panels, Dark/Light themes, English/Simplified Chinese, font previews and structured operation logs. Windows includes remembered tray hosting with Open/Exit.

The two-pane object browser filters cached names immediately and searches missing catalog pages after a short pause. Only tables/views and expanded categories are searched by default; the search menu enables all object types or slower comment matching. Supported paged browsers use one on-demand metadata session, released after 30 seconds idle, with a 15-second name/column read budget (or a shorter configured timeout) and explicit retry on failure; opt-in comment/global-index searches use the configured execution timeout; in-memory SQLite stays on its original session.

Split editor panes can be restored to one pane without closing documents. Table comparisons keep the current source fixed and let you search the target connection/database/table, with optional linked scrolling and colored differences. Table-query Notes reuse an already open source and fill the available result area; explicit entry refreshes safe SELECTs, while modified writes/locking queries require manual execution.

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

The center tab bar provides a fixed new-Note button. Table definitions use compact field/type/not-null/auto-increment/primary-key/extra/comment columns with a DDL toggle; unavailable metadata remains explicitly unknown.

MySQL legacy-data GBK mode reads raw bytes from misdeclared latin1 columns through the isolated native compatibility helper. It is opt-in, read-only, preserves true binary values and disables scripts, write-SQL generation and transfers. The compatible-client path currently requires explicitly disabled TLS; existing TLS settings are never changed automatically.


The two-pane object browser loads 200 directory entries at a time with a 5,000-object memory cache. Filtering immediately searches cached names/comments, then completes incomplete directories through the current connected database after 350 ms. Columns load when a table is expanded; indexes, foreign keys and triggers load separately. Unsupported/legacy adapters explicitly retain local-only filtering. Export preflight remains independent.

Table context menus offer center/right table designers, data grids, single-SQL Notes, ER views, data/structure comparisons and transfer actions. Designers show seven columns with horizontal scrolling. Column definitions retain raw MySQL defaults/charset clauses; other adapters expose supported ALTER operations without implicit table rebuilds. Save previews editable/copyable SQL, then separately confirms execution against the frozen connection generation and original DDL. Views remain read-only in this editor. Comparisons require already-connected sources and never write: data compares at most 200 rows per side (compatible primary-key alignment or explicit row-order fallback); DDL comparison is bounded to 2,000 lines per side. A sample match does not establish whole-table consistency.

Center tabs can be reordered by dragging and moved into up to four horizontal panes from the context menu. Empty panes accept dropped tabs or new Notes. Pane sizes and placement of reopened Notes persist in the application database; restoring a layout never reconnects or runs SQL. Data/comparison results are not persisted. Initial grid widths use headers and at most 32 sample rows, with bounded widths; manual overrides take precedence.

Table designer/data/comparison tabs restore only target metadata and comparison input drafts. They require an explicit Load tab action after the saved connection is ready; generations and write grants are recomputed from the current connection. No query result, credential or DDL draft is serialized into workspace layout.

SQL reading mode exposes run/stop and execution-plan icons. Clear results stays in the result toolbar and preserves SQL; ER views display column comments.

Table designers offer driver-specific type templates and custom input. Structure comparison opens a field grid with a DDL toggle; copy-only synchronization drafts apply right-side definitions to the current left table. Cross-driver translation is unavailable. Indexes, constraints, table options, column ordering and unsupported alterations require manual completion; SQLite drafts avoid implicit rebuilds. The navigation rail places Databases before Notes.

Type templates are searchable. Unsupported synchronization differences are listed individually in the preview and copied SQL; server-version compatibility must be reviewed manually. SQLite additions requiring a rebuild and generated/identity/reference column transformations are left for manual migration.

Database sidebar layout is configurable in Settings → Database: **Connections + Objects** remains the default, while **Merged tree** supports independently expanded connected sources, databases and paged object categories in one virtual list. Switching layouts preserves sessions, editor drafts and layout scroll state. Browsing never changes the saved default database or connects other servers. The merged browser stores complete directory snapshots on disk and reads local results in pages, without a fixed total directory-entry cutoff. Loaded column details remain bounded to 20,000 entries. Refresh replaces the corresponding complete snapshot; reconnecting releases runtime pages while keeping persistent metadata.

When Codex CLI is unavailable, AI agent settings offer a managed download of the pinned official native CLI, an official installation guide, the existing executable selector, and matching offline archives. Downloads verify the pinned SHA256, platform, version and App Server capability before binding an application-managed executable; existing CLIs and PATH are preserved. Browser and device-code sign-in use a separate controlled App Server process and can be cancelled. Installation, binding, authentication and connection are separate states; none sends a prompt automatically. CLI Windows 10 compatibility and real account sign-in require separate validation. Official installation and authentication references: https://developers.openai.com/codex/cli/ and https://developers.openai.com/codex/auth/ .

Redis and unsupported object categories retain their existing browser in Connections + Objects; the merged tree provides a layout settings link instead of issuing relational queries.

The merged tree uses one clearable search field with a connection/object scope menu. Per-connection database include/exclude rules support `%`/`*`, persist locally and apply before SQLite pagination; they control sidebar and Note database candidate visibility without retargeting saved SQL. More database names appear after the loaded tree, with a visible overflow scrollbar. Notes keep SQL/MD/task creation at the bottom, offer whole-file Top navigation, and keep a bounded default database selector on one horizontally scrollable toolbar.

The merged tree offers three filter scopes and separate Databases / Users branches. Object searches freeze the displayed database names and stay local; hidden or not-yet-paged databases are excluded. Supported adapters provide read-only account names with paging and manual refresh. Connection database rules also filter Note-level and cell-level candidates while preserving saved targets.

Only expanded databases participate in merged-tree object searches; other database names remain collapsed and changing expansion updates the local search scope. MySQL-compatible users have clipped Create user / View permissions menus: password-masked SQL preview and explicit confirmation for creation, plus a direct-grant workbench in a central editor tab. Account management has driver/version limits and does not infer inherited or effective privileges.

Direct MySQL grants use a resizable directory/object/privilege workbench with independent scrolling, staged changes and guarded SQL preview/confirmation. Duplicate openings reuse the tab; closing or refreshing handles unsaved edits, and restored tabs never connect or apply grants automatically. Note SQL Query runs a complete selection or the entire unit through one execution action, retaining independent read-only result tabs for batches and guarded single-table editing for eligible single queries. Conversion/splitting actions have been removed.

SQL editors prioritize character input: highlighting runs after word boundaries or idle time on a coalescing background worker. Completion uses scoped memory/SQLite caches; T offers up to eight cached table names at the caret, and automatic column hints never fetch remote metadata.

SQL caret movement reuses a bounded shaped viewport and paints visible wrapped lines. Alias-dot completion uses cached columns; missing columns expose an explicit load action at the caret, with no automatic remote metadata read. The whole-file Top button shares the bottom SQL/MD/task strip. Batch rowsets retain independent pages and scroll positions; session-dependent results keep only their returned page. Up to 50 rowsets and approximately 16 MiB of returned text are retained (an individual rowset remains subject to adapter response limits).

Both Note add menus offer SQL Query and SQL File. F5 runs selected complete SQL or the entire query unit, directly for read-only batches and with batch authorization otherwise. SQL File imports a managed copy without executing, supports guarded replacement, and collapses source preview by default. Both use one-line result status, named tabs and expandable details. Query batches are limited to 1 MiB; larger files retain the attachment workflow. Cached T suggestions show table comments; Col suggestions show type and comment without inserting this metadata. Automatic assistance can be paused without changing native editing.

Read-only Note SQL scripts execute directly; scripts containing writes, DDL, session changes, locks, transactions or other risky statements retain batch authorization.

SQL editing reuses bounded fragment layouts and shapes visible soft lines plus caret/selection endpoints. Unaffected highlighting stays visible while background analysis catches up; immutable Note text snapshots are materialized by the save worker.

Note supports two views of the same document, a searchable one-line open-note list with project information, dated filenames, and cursor-local column completion with explicit cache loading.

T completion uses context-aware Tab for SELECT or table names, and Enter for names. Explicit metadata loads update the popup in place without executing SQL.

Note cells share connection/database selection across SQL Query and SQL File. Context-aware column hints and DDL follow the current statement; missing metadata can be loaded in place. Markdown offers toolbar clearing, and tasks accept multiline lists with batch undo.

Note function completions offer offline, dialect-aware explanations, parameters and examples. Enter inserts the function name; Tab inserts an editable call template. Column completions share the result-header primary-key and type markers.

Note DDL follows selected relation names or SQL before the current statement; multiple tables offer line-tagged candidates. Unresolved targets provide explicit selection, and failed metadata loads retain the target for retry. Editor SQL is never executed by this action.

Windows installed editions use a separate update helper staged outside the application folder. Download and SHA256 verification show byte-based progress; after saving and confirmation, the helper waits for the process and other instances to exit, then opens the normal installer with its own installation progress. Reopening after success is optional. Portable editions remain manual; older installed editions need one manual upgrade to acquire the helper.

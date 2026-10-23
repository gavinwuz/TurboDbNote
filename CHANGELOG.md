# Changelog

Release notes are generated from the version sections below.

## [Unreleased]

## [0.1.3] - 2026-10-23

- Unify editable SQL and script cells as SQL Query: execute complete selections or whole cells, show multiple named result tabs, and keep execution status on one line with expandable details. Read-only queries run directly; risky batches retain frozen-target authorization.
- Add managed SQL File cells with connection/database controls, guarded file replacement, preview and compact results. Import SQL File uses the same flow.
- Improve native SQL input and caret movement through bounded layout caching, visible-line rendering and background highlighting. Completion uses bounded cached metadata and cannot execute SQL.
- Anchor T and Col suggestions at the caret. Resolve aliases throughout a statement, load missing metadata in place without dismissing suggestions, show table comments and column types/comments, and offer context-aware table-name or SELECT insertion.
- Add offline function descriptions, parameters, examples and editable call templates with existing-argument protection. Reuse column-header key, text, numeric and date markers.
- Fix selection-first DDL targeting for quoted/qualified table names and SQL in multi-statement cells. Offer line-tagged candidates, explicit unresolved selection and retry; discard stale requests.
- Keep Notes in both panes for same-document comparison, with independent scrolling. Add searchable project-aware open-note lists, connection-aware titles and dated filenames; compact toolbars, bottom Top action, Markdown clearing and multiline task creation with undo.
- Improve split/merged database navigation with persistent per-connection database rules, scoped cached filtering, expandable database/user branches, progressive loading and explicit refresh. Object searches stay inside displayed, expanded databases and never implicitly connect servers.
- Add MySQL user creation and direct privilege inspection/editing in central three-pane tabs, with staged drafts, SQL previews and explicit authorization.
- Improve bounded, paged object browsing, lazy columns and MySQL name-first metadata loading. Preserve original grid values, custom widths, result edits and independent sessions.
- Share compact seven-column table designers across center/right panels, with searchable driver-specific types, guarded SQL previews and separate execution confirmation. Add field-grid structure comparison and copy-only synchronization drafts.
- Add center-tab reordering, horizontal panes and restore controls; improve ER comments, read-only data/DDL comparisons and typed WHERE generation.
- Add opt-in read-only MySQL legacy raw-GBK compatibility, preserving binary values and guarded metadata decoding.
- Add hash-pinned managed Codex installation, offline archives and explicit sign-in without automatic inference.
- Include the patched GPUI Component source in Windows source archives. Windows packages remain unsigned; native Windows/isolated SQLite verification does not establish Linux or live external-server acceptance.

## [0.1.2] - 2026-10-16

- Verify and cache official Oracle/Db2 clients before release compilation; retry transient downloads and report actionable component-specific failures.

- Support independent MongoDB authSource and target-scoped connection tests without requiring admin database listing; classify authentication, permissions and connection failures.

- Add Rust driver component download/offline import, dependency probes and persisted per-driver enable switches; generate full/minimal distributions and reusable offline component archives with a pinned Oracle client.

- Replace empty script write-result grids with bounded execution details, cumulative affected-row statistics, preserved query results and on-demand table structure previews.

- Add IF NOT EXISTS to ordinary table DDL in SQL file exports while preserving quoted data, comments and routine bodies.

- Add managed SQL file path copying and persistent execution failure positions with stage/completion details and frozen-source navigation.

- Replace repeated script authorization dialogs with one frozen-script preview, risk summary, bounded source browsing and explicit authorize-and-execute action.

- Explain missing script permissions with command categories and source positions, plus locate/settings actions; remove misleading metadata-query wording.

- Fix modern MySQL database refresh losing the full visible database list when a default database is configured.

- Integrate issue navigation into the script execution toolbar and support guarded MySQL ENABLE/DISABLE KEYS, including executable version comments and automatic index upgrades.

- Add unsupported-statement filtering/navigation, six statements per page, an explicit list scrollbar and a selected-source copy icon.

- Extend MySQL Note scripts with controlled dump headers/footers, explicit database matching, preauthorization, schema fingerprints and session cleanup; explain disabled execution through hover tips.

- Add Note SQL import with managed script attachments, a virtual paged statement list, bounded previews, sequential execution and per-write confirmation.
- Replace six Python build/release/diagnostic scripts with native Rust developer tools; keep active GUI and asset maintenance checks.

- Fix MySQL database detection for binary-flagged UTF-8 metadata; report incomplete lists as warnings and preserve safe server error categories. Allow bounded Note SHOW DATABASES/SCHEMAS and literal LIKE filters for metadata diagnostics, subject to existing command permissions.

- Add startup/manual stable-release checks, persistent upgrade prompts, version skipping and cancellable in-app downloads with SHA256 verification.
- Windows installer builds save pending Notes/settings before exiting and launching a separately verified installer; staged edits, connection drafts and active database/Note operations block installation. Portable Windows/Linux updates remain manual.

- Add local, bounded connection URL preview and editable batch drafts with masked credentials; move MongoDB authentication-source overrides to Advanced and show concrete component compatibility differences.

- Add typed MongoDB field grids, a virtual linked EJSON tree, node copy/fold menus and confirmed optimistic document edits/clones. Preserve read-only defaults and advance native driver protocol to 3.
- Defer shared grid column resizing until drop, add numeric/fit/reset width actions, and persist widths separately from document data.

- Simplify driver settings with enable switches, per-driver prerequisite details and a collapsed unavailable section; remove the chat-specific settings footer. Add connection URL help and MySQL TCP DSN compatibility with explicit client-option notices.

- Add read-only generic ODBC connections, stable driver status columns, consistent Access/Db2 detection, and official offline Windows Db2 CLI materials. Generate network connection names as database@host-suffix; require worker protocol 4.

- Support script-only MySQL DROP TRIGGER with opt-in permission, batch authorization, target/metadata rechecks and index refresh.

- Support script-only isolation levels, bounded user-variable assignment and SELECT INTO (including explicitly authorized FOR UPDATE).

- Preserve sanitized server errors for script session/trigger/variable statements; show the actual DROP TRIGGER command despite leading comments.

- Unify connection picker identities; add Note-local access ceilings, script comment/source previews, frozen-request checks and explicit sanitized MySQL diagnostics.

- Separate connection and execution timeouts with a backward-compatible 300-second operation default; format elapsed UI values adaptively and restrict Note DELETE to DBA. External component protocol 5 propagates operation deadlines.

- Add global timeout defaults with connection inheritance and preserved overrides; add scoped, actionable Note permission guidance without automatic SQL replay.

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

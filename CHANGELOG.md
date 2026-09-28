# Changelog

- Refine Note SQL/MD icons, cell toolbar, database dropdown width, bounded path tooltips, shared project ordering and recent-file home; reduce JSON parsing allocations.

- Move New project to a dedicated folder-plus icon before Open in the database header and remove duplicate add buttons from category rows.

- Standardize the display brand as TurboDBNote and rename database connection groups to projects in navigation and connection forms, preserving stored configuration identities.

- Add export destination choices, copy-to-new validation and source charset, execution transport preflight, bounded persistent operation traces, and explicit connection-draft discard on close/switch.

- Redesign transfers with source/destination columns, timestamped filenames, selectable tables and object categories, grid previews, configurable batches, and guarded append/replace/primary-key upsert for SQLite and modern MySQL.

- Add searchable database selection, default database priority, clearer tree indentation, and unified export/copy/import tasks with preview, progress, cancellation, and same-engine transfers.

- Kept count progress within the single-row grid toolbar, removed the mode eye icon, improved readable copied WHERE predicates, and added bounded cell-preview caching with lazy header tooltips.

- Connection imports can detect databases using temporary credentials and a searchable single selection. Font and size controls now share a row, with all installed fonts searchable and a live preview below.

- Added selective connection imports and DBP support, clickable output title and a dedicated name-generation icon. Settings now include font previews, shortcut reference, database tabs and compact AI configuration lists with dialogs.

- Recent connections show addresses and relative times; environment menu colors match the sidebar. Live connections allow draft editing with explicit disconnection confirmation before saving.

English | [简体中文](CHANGELOG.zh-CN.md)

Version history is maintained here. Release notes are generated from the matching version section; candidate versions are not necessarily publicly released.

## [Unreleased]

- Replace Note metadata forms with a compact document toolbar, inherited default connection/database scope, sequential run-all/stop-all and collapse/expand actions. Move file operations to Note context menus and save status/project/path to tab indicators and tooltips.

- Redesign Note cells with reading/editing modes, contextual icon toolbars and bottom insertion controls. Reuse the table data grid for SQL results; direct single-table projections with complete keys support guarded edits and filters, while joins and derived results remain read-only.

- Add standalone Note drafts, shared project navigation, recent/open recovery, SQL cell execution with 20-row paging, offline Markdown/tasks, and configurable draft storage and autosave in General settings.

### Added

- Add charset-aware database creation, guarded MySQL/SQLite copying, recent-connection and NCX/DBeaver import menus in the database header; remove duplicate table/view counts from the database name.

- Add multiple filter conditions with AND/OR, separate date/time fields with a Now action, and a compact cell toolbar for NULL, restore-original and copy-WHERE operations.

- Move filters into a collapsible top condition bar with symbolic comparisons and applied/draft state. Share date-aware inputs between filters and cell editing, preserve timestamp precision/timezones, group edit actions, and support grid Ctrl+C/F2 shortcuts.

- Expose duplicate-row and page-export buttons; add editable page size (default 200), first/previous/page/next/last navigation and an explicit manual row-count button. Unknown totals never trigger automatic counting.

- Consolidate data-grid actions, paging and row statistics into one bottom toolbar; use warm yellow highlighting in light mode and muted blue-gray in dark mode, while preserving custom colors.

- Select connections without opening their editor; edit from the context menu. Combine the database name with connection status or separate table/view counts.

- Refined data-grid headers with visible sorting and colored type markers, menu-based column copying, persistent overflow scrollbars, configurable cross highlighting, and consistent refresh icons.

- Add connection type views, filtering and unique name generation; bound error notifications with full-text copying. Upgrade data tabs with a virtual grid, primary-key pagination, sorting/filtering, staged transactional edits, row copying, inspector and schema views.
- Follow the selected connection in the object browser, clearing offline targets and ignoring retired metadata responses.
- Display database connection icons, names and muted addresses on a single row, retaining status and disconnect controls.
- Enable password saving for new connections with a memory-only warning when disabled; prevent clipped connection rows from opening context menus over the object tree.
- Encrypt saved connection passwords with Windows user DPAPI inside SQLite; hide storage paths behind an open-folder action; double-click connections to connect and use a shared context menu.
- Allow testing without a connection name or database; load passwords masked with a visibility toggle; retry TLS failures once without encryption and retain the option only after success.
- Add a searchable detected-database picker and move database creation into the Objects header plus menu.

- Migrate connection profiles to a dedicated SQLite store with creation/update/usage timestamps; retain passwords securely while editing, unify icon feedback and right-aligned dialog actions, confirm risky changes, and add IDX/key/import/export icons.

- Require explicit connection names and database targets; show two-line connection rows, status dots and disconnect controls, cap recent entries at five, and scope table menus and tree indentation correctly.

- Add a resizable connection/object sidebar, grouping and recent connections, table data tabs, reviewed SQL changes, and bounded CSV workflows.

### Fixed

- Renamed the database sidebar to Databases and removed close controls from both Projects and Databases; navigation/layout buttons still toggle the sidebar.

- Added two-step connection setup for MySQL/SQLite/PostgreSQL/ClickHouse/Redis; grouped navigation with persistent ordering, environment/status icons and virtual catalogs; live object inspection and capability-specific database creation.
- Added TLS failure classification and recovery dialogs with explicit encrypted/plaintext trial modes, opt-in save after success, legacy shortcuts, and the missing global dialog render layer.
- Added GB2312/GBK and custom charsets, bounded visible-database discovery and selection, expandable connection catalogs, compact icon actions, and collapsed driver-path details.
- Added managed legacy-driver download/import with pinned hashes, cancellation, default release bundling and source/license files; simplified connection tabs, editable environment/group dropdowns and Save & Connect.
- Removed nested ChatView updates from AI Settings clicks that could panic; added last-chat snapshots and deferred Codex thread resume after restart.

- Isolated code-copy pointer events, added a selection-preserving copy context menu, and synchronized AI expansion with Settings navigation and Return to Chat.

- Windows packaging uses explicit compiler parameters and an isolated GPUI build adapter, avoiding `Failed to find fxc.exe` without custom environment variables.

### Added

- MySQL connection management with saved profiles, search/grouping, test/connect/cancel, Windows credential storage, TLS controls and detected server details. Modern adapter targets 5.6/5.7/8.0; legacy 4.0/4.1/5.x connections use an isolated helper and an explicitly selected MySQL C client DLL, with old-auth opt-in and no silent TLS downgrade.
- Stable semantic translation keys, named interpolation, English fallback, legacy language-pack aliases and build-time catalog/reference validation.

- CrashSight Windows FFI, macOS bridge source, local panic replay and bounded allowlisted usage queue; overseas-first configuration with an explicit next-launch domestic fallback marker. SDK delivery and automatic failure detection are not yet verified.

- Compact Output header actions, timestamped monospace logs with error markers, and a database/AI connection status bar.

- Compact AI composer with disabled database-context selectors, icon-only actions, persistent typography scales, and Codex/Claude/OpenClaw settings tabs. Empty chats hide Latest; account readiness moves out of the conversation area while errors and generation progress remain visible.

- Editor menus now include Close all tabs; AI Chat, Inspector and Schema are fixed tools. About uses localized release metadata cards, and built-in dock menus follow the selected language.

- Icon-and-label tabs/buttons, group close menus, categorized settings with 600 ms auto-save, SQLite migration, language/theme packs, provider/model configuration and Help release checks. OpenClaw/extensions can be registered with installation links; their chat transport is not yet integrated.

- Per-engine automatic connection on first panel display and optional last-session restoration. Defaults are enabled; manual disconnect and failures suppress further automatic attempts for the current run, with no automatic inference or message replay.

- Claude Code CLI engine alongside Codex: native discovery, login checks, streaming replies, Stop and session resume; per-engine settings, drafts and history isolation. Claude model aliases do not reuse Codex effort settings.

- Agent-style composer card with a Codex engine menu, combined model/effort picker, draft-preserving prompt shortcuts, collapsible work status and a local workspace footer.

- A reusable Settings tab in the main editor with General/AI categories, direct AI entry navigation and unsaved-change close handling.

- Conversation-first AI layout, shared General/AI settings with persistence, onboarding, expand/restore, elapsed time and a stop icon. Selectable Markdown, code copying and pause/resume following preserve reading space.

- Automatic Codex executable discovery through PATH and npm/Volta layouts, version validation, manual file browsing and multiple-candidate selection without overwriting user input.

- Inno Setup EXE alongside the portable ZIP, with bilingual installation, per-user shortcuts, uninstall support and a running-application guard. Windows 7 support is out of scope.
- A unified 40px title bar combining branding, layout/theme actions and window controls.
- Codex App Server chat: dynamic model/effort selection, existing-login detection, streaming Markdown, cancellation, new conversations, copy and bounded runtime logs. First stage uses read-only analysis; UI history is session-only.

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

- Unify Note file and SQL/MD insertion icons with neutral document frames and colored upper-right type badges.

- Reuse fixed preview and packaging workspaces; add guarded cleanup of old binaries and incremental caches while preserving runtime data.

- Use a three-row checkbox list icon for task-cell insertion, shared by the menu and bottom toolbar.

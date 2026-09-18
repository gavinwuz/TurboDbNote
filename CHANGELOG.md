# Changelog

English | [简体中文](CHANGELOG.zh-CN.md)

Version history is maintained here. Release notes are generated from the matching version section; candidate versions are not necessarily publicly released.

## [Unreleased]

### Fixed

- Removed nested ChatView updates from AI Settings clicks that could panic; added last-chat snapshots and deferred Codex thread resume after restart.

- Isolated code-copy pointer events, added a selection-preserving copy context menu, and synchronized AI expansion with Settings navigation and Return to Chat.

- Windows packaging uses explicit compiler parameters and an isolated GPUI build adapter, avoiding `Failed to find fxc.exe` without custom environment variables.

### Added

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

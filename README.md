# TurboDbNote

English | [简体中文](README.zh-CN.md)

A native SQL Notebook workbench built with Rust and GPUI Component.

Current version: **v0.1.0 (preview)**. See the [changelog](CHANGELOG.md).

The product name is **TurboDbNote**. The desktop package and executable are named `turbodbnote` (`turbodbnote.exe` on Windows). Internal packages remain `turbodbn-core` and `turbodbn-services`.

This initial scaffold provides a four-region Dock layout, auxiliary tabs, independent SQL / Markdown / task editor entities, cell folding and task status changes, plus document and query models that can be tested without a window.

The editor uses a 48px icon-only navigation rail with Data Note branding, projects, connections, settings and help. The top toolbar controls three regions, focus mode and Dark / Light themes. Output is collapsed by default. Theme changes are saved automatically.

The 40px unified title bar combines Data Note branding, the product name, layout/theme actions and window controls in one row. The navigation rail starts below it. This replaces the separate native caption and toolbar; see the [title bar design (Chinese)](docs/plans/2026-09-11-unified-titlebar.md).

> Notebook edits are currently kept only in memory and are lost when the window closes. Database execution, file saving, notebook Markdown preview and result tables are not connected. With saved settings, AI Chat can automatically check the selected CLI when first displayed; model requests start only when you send a message.

## Development environment

- Rust 1.92.0 (the validated toolchain) and Cargo; dependencies are locked in `Cargo.lock`.
- Windows: MSVC toolchain, Visual Studio C++ Build Tools, Windows SDK and CMake. GPUI requires a native graphics environment.
- macOS / Linux: install the [GPUI platform dependencies](https://github.com/zed-industries/zed/tree/main/docs/src/development). Current validation primarily targets Windows.
- Keep `gpui = 0.2.2`, `gpui-component = 0.5.1` and bundled assets compatible. Check upgrades together; do not mix gpui-pre types.

```powershell
cargo run --locked -p turbodbnote
```

The first build downloads and compiles native GPUI dependencies and takes longer than incremental builds. Use `cargo run --locked --release -p turbodbnote` for release performance evaluation.

## Project structure

```text
crates/
  core/src/        # .note schema, cell types, independent query state
  services/src/    # Database cursors, AI sessions and typed service contracts
  desktop/src/    # Startup, Dock workspace and cell editors
docs/plans/       # Chinese development plans, decisions and acceptance targets
```

The domain layer does not depend on GPUI. Services provide database contracts and a concrete Codex App Server adapter. The desktop uses real GPUI Component controls and retains editor entities across renders. SQL highlighting, completion and formatting require dialect registration and services; these capabilities are not claimed as complete.

## Validation

```powershell
cargo fmt --all -- --check
cargo test --locked -p turbodbn-core -p turbodbn-services
cargo check --locked --workspace --all-targets
cargo clippy --locked --workspace --all-targets -- -D warnings
```

Manual checks: switch the three right-side tabs, resize regions, toggle regions from the toolbar, edit and fold/unfold cells without losing text, and cycle task status. Cross-region dragging is disabled to preserve the separation of projects, database objects and results.

Layout checks: switch between projects and connections; click the active destination again to collapse it. Enter and exit focus mode from different panel combinations and verify that visibility and sizes are restored. Switch Dark / Light from both the toolbar and settings; inspect editor, dialog and icon contrast. Help provides interaction instructions.

The Data Note logo combines database arcs, notebook binding and a page corner. See the [color application SVG](crates/desktop/assets/data-note-app.svg) and [theme-tinted navigation SVG](crates/desktop/assets/icons/data-note.svg). Icons are embedded in the navigation rail and Windows EXE (resource ID 1). The multi-resolution ICO is checked in; ordinary builds do not require image conversion tools.

See the [Rust workbench plan (Chinese)](docs/plans/2026-09-11-rust-workbench.md) for requirements, data flow, performance budgets and future database/AI implementation. The [validation record (Chinese)](docs/plans/2026-09-11-validation.md) distinguishes completed checks from pending UI acceptance.

## Windows application icons

`crates/desktop/build.rs` compiles `assets/windows.rc` and embeds `assets/turbodbnote.ico` as EXE resource ID 1, matching GPUI's window icon loader. The ICO includes 32-bit images at 16, 20, 24, 32, 48, 64, 128 and 256px. Navigation SVGs have a teal base stroke so they are visible on white backgrounds; GPUI applies theme colors at runtime.

After changing the color SVG, regenerate the assets (Python dependencies are needed only for asset maintenance):

```powershell
python -m pip install resvg-py==0.5.0 Pillow
python scripts/generate-icons.py
cargo build --locked -p turbodbnote
python scripts/verify-windows-icon.py
```

See the [size and background preview](crates/desktop/assets/icon-preview.png). Exit old processes before checking a new EXE. If a pinned shortcut retains its old icon, unpin and pin it again.

VERSIONINFO provides FileDescription / ProductName (TurboDbNote) and version fields. Update `assets/windows.rc` when changing the release version. Task Manager may cache process-group names and icons separately from window icons; restart the application and Task Manager when checking updates.

For compatibility, 16–128px frames use traditional 32-bit DIB + AND masks; 256px uses PNG. Windows startup also explicitly sets `ICON_SMALL` and `ICON_BIG`. The resource verifier calls native Windows decoding APIs. `python scripts/verify-window-icons.py` launches a separate empty instance, verifies both window icon slots and closes that instance.

## Packaging and releases

Targets Windows 10 1809+ / Windows 11 x64. Windows 7 is not supported. Install Inno Setup 6.7.3 and Windows SDK, then supply compiler paths as arguments. If `-FxcPath` is omitted, SDK discovery uses the registry and standard installation directories. No custom tool-path environment variables or PATH changes are used. A temporary, version-checked GPUI source adapter reads the compiler path from a local file; the registry source and repository lockfile remain unchanged.

```powershell
pwsh -File scripts/package-windows.ps1 -FxcPath 'C:\SDK\x64\fxc.exe' -IsccPath 'C:\Program Files (x86)\Inno Setup 6\ISCC.exe'
```

This produces `*-portable.zip`, `*-setup.exe` and SHA256 checksums from the same Release binary. The installer defaults to the current user's Programs folder, includes English/Chinese, creates a Start Menu shortcut and optionally a desktop shortcut, and provides an uninstaller. Exit the app before installation or upgrade; it will not be closed automatically. Portable ZIPs include portable.flag and store settings in data/settings.db beside the executable; installed builds use the user configuration directory. Root-level bilingual changelogs generate the bundled release notes. Tag workflows upload both candidate artifacts without public release. See the [packaging plan (Chinese)](docs/plans/2026-09-11-release-v0.1.0.md).

Extract downloads into a separate folder and run `turbodbnote.exe`. Keep the license and instructions; do not replace a running EXE. Compare `Get-FileHash <downloaded-file> -Algorithm SHA256` with `SHA256SUMS.txt`. Candidate builds are unsigned. If Windows blocks a binary, verify its source instead of disabling system protection. Issue reports should include Windows version, GPU, reproduction steps and `BUILD-INFO.json`, without passwords, connection strings or sensitive queries.

## AI Chat: Codex / Claude Code CLI

In Settings → AI, **Automatically connect when opening the panel** and **Restore the previous conversation on startup** are enabled by default for each engine. Only saved configuration is eligible. Automatic connection performs readiness checks, never inference or message replay. Each engine gets one automatic attempt per run; after manual disconnect or failure, use Retry. Disabling history restoration does not delete saved history.

The engine menu switches between Codex App Server and Claude Code CLI. Each engine has separate executable/workspace settings, model choices, draft state and last-session history. Switching is disabled while connecting, saving or generating; stop the current turn first. Switching never sends a request automatically.

The latest conversations are saved separately beside settings: `last-chat.json` for Codex and `last-chat-claude.json` for Claude. Startup restores messages, partial output and elapsed time independently of the connection. On the next explicit send, an unchanged executable/workspace can resume that engine’s saved session. A missing server-side thread reports an error; use New conversation to start afresh. This is a single last-session snapshot, not a searchable history list. Chat content is stored locally as plain JSON; a forced shutdown may lose the last save interval.

Settings now opens as a reusable tab beside the Notebook in the main editor. The left navigation restores the last category; AI Chat's setup button and gear open the AI category directly. Changes save automatically after 600 ms; closing flushes pending edits. Invalid or failed saves offer retry, discard or continue editing, while the Notebook and chat stay alive.

AI Chat prioritizes conversation output in a wider panel. On first use, choose **Open settings**: the shared settings tab has General, AI Agents and AI Providers categories. AI settings contain detection, executable path, working directory, account connection and default model choices for the selected engine. Click Connect to start; subsequent panel opens after restart connect automatically by default. Detection never sends a model request. Choose Codex or Claude from the engine menu; Claude native executable discovery checks PATH and the user .local/bin directory.

The chat toolbar offers **+ New conversation**, expand/restore and settings. While generating, the send button becomes a square stop icon and elapsed time is shown. Stopping preserves received text; the backend interrupts the turn and terminates its process tree if cancellation times out. Scrolling up pauses following; **Latest** resumes it. Clicking to select text freezes the displayed snapshot while reception continues. Markdown supports selection, links and code-block copying; exact selection/async-parse edge cases still need UI acceptance.

Settings persist by provider and the last UI conversation is saved locally. Codex may retain its own history. Saving an AI configuration does not enable database execution. Notebook references/insertion, a multi-session history browser and in-app login remain planned. See the [chat layout design (Chinese)](docs/plans/2026-09-14-chat-layout.md).

Validated against Codex CLI 0.153.4 and Claude Code CLI 2.1.139. Claude uses print-mode streaming JSON and --resume, existing CLI login, no tools, and per-turn process termination for Stop. Claude models are CLI aliases (default / sonnet / opus / haiku), not an account-discovered catalog; effort override is not offered for Claude. See the [Claude integration design (Chinese)](docs/plans/2026-09-17-claude-cli.md). See the [integration design (Chinese)](docs/plans/2026-09-11-codex-app-server.md). No custom tool-path environment variables are set; paths are passed to the process API, models and effort through protocol requests.

## Layout and settings

Title-bar actions and the left navigation rail are icon-only with tooltips; content buttons and settings categories use icons plus text. Tabs show an icon, title and close icon. The editor menu and tab context menu include Close all tabs; context menus also offer Close, Close others, Close right and Close left. The right-side AI Chat, Inspector and Schema tabs are fixed, non-closable and retain their order. Closing keeps notebook and chat entities in memory; Help → Restore auxiliary tabs reopens the notebook and auxiliary tabs. Settings temporarily collapses auxiliary regions and restores their visibility on close.

Settings saves valid edits automatically after a 600 ms pause, without a Save button. Closing a tab/window flushes pending edits; validation and disk errors retain the draft. Auto-save does not connect an agent or send inference. General contains Light/Dark and extensible themes, language, font and size. AI Agents preserves Codex/Claude configuration and adds an OpenClaw/extension registry with executable, working directory and explicit installation-guide links. Extension entries are configuration only; built-in chat still uses Codex or Claude.

AI Providers supports separate named configurations for OpenAI, Gemini, DeepSeek, Qwen, Ollama and Custom. Choose Chat Completions, Responses or Anthropic Messages, enter an API base URL (for example `/v1`), masked API key, model ID and optional JSON parameters. Fetch models requests the configured `/models` endpoint on demand; manual IDs remain available. These profiles and advanced parameters are persisted configuration; they are not yet a direct API chat transport. Switching vendors clears the previous credential and model. HTTPS is required except on loopback; redirects are disabled and requests have a five-second timeout.

Preferences now use SQLite `settings.db`, transactionally storing the complete settings document. On first use an adjacent legacy `settings.json` is imported and retained as a backup. Portable builds use `data/settings.db`; installed builds use the user configuration directory. API keys are masked in the UI but stored locally in the database, without encryption. Chat snapshots remain separate JSON files.

Independent language packs are bundled in `crates/desktop/assets/locales/`. Put compatible JSON packs in `locales/` beside `settings.db`; restart to add or override a language, then select it in General. Packs use `schema_version: 2`, `id`, `name` and a flat `strings` map with stable English semantic keys. Missing entries fall back to the same-language built-in pack, then built-in English. Legacy source-text keys remain compatible for this migration. See [translation rules and validation](docs/i18n.md). Theme packages use GPUI Component's ThemeSet JSON format in the adjacent `themes/` directory and appear under Additional themes.

Help contains About (version, release date, channel, platform and license, sourced from `crates/desktop/assets/release.json`), GitHub, and an asynchronous stable-release check. This preview has no published release date; About states that explicitly. Checks use GitHub's latest-release API and semantic version comparison; they never download or install an update. See [design alternatives and validation](docs/layout-settings.md).

## Documentation maintenance

Keep English and Chinese README sections, commands, versions and feature scope synchronized. Use matching `## [Unreleased]` / `## [0.1.0]` headings in `CHANGELOG.md` and `CHANGELOG.zh-CN.md`, with newest versions first and third-level headings inside each version. `docs/plans/` remains in Chinese for now. Do not maintain separate per-version release-note source files.

### Compact AI workspace

The composer shows disabled Connection/Database selectors until database integration is available. Chat action toolbars use icons with tooltips; the model picker retains its current name and truncates long labels. Agent settings expose Codex, Claude and OpenClaw tabs. Switching Codex/Claude changes the active engine and retains separate settings/history; switching is disabled during generation, connection or pending valid saves. OpenClaw remains a configuration registry.

Font sizes now derive from General's base size: body/controls 1×, supporting text 0.875×, headings 1.125× and branding 1.25×. Code uses the same base size. Theme reloads retain user typography. Empty conversations do not show Latest/Resume, and successful account checks stay in the run log rather than a duplicate chat status section. Errors and generation progress remain visible.

Output uses a terminal icon and the title **Output**, with icon-only Follow, Clear and Close actions in its header. Entries include local timestamps; explicit errors have an ERROR marker and error color. The status bar shows database state on the left and live AI state on the right.

## CrashSight diagnostics

Optional Windows CrashSight dynamic loading and a macOS Objective-C bridge are implemented with a separate bounded SQLite usage queue. The selected project is overseas first, with a domestic fallback applied only at the next launch after a confirmed failure signal. **Automatic detection of upload failure is not available in the documented C API and is not wired.** No App Key is embedded. A public Windows SDK was downloaded to the ignored vendor directory and inspected without initialization; macOS still needs its native framework and an on-device build.

Without an SDK the application continues running and records allowlisted lifecycle/action metadata locally. Usage upload requires your own HTTPS receiver; none is configured. Native crash dumps may contain memory/device data. See [setup, limitations, symbols and local verification](docs/crashsight-integration.md).

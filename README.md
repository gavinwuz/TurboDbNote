# TurboDbNote

English | [简体中文](README.zh-CN.md)

A native SQL Notebook workbench built with Rust and GPUI Component.

Current version: **v0.1.0 (preview)**. See the [changelog](CHANGELOG.md).

The product name is **TurboDbNote**. The desktop package and executable are named `turbodbnote` (`turbodbnote.exe` on Windows). Internal packages remain `turbodbn-core` and `turbodbn-services`.

This initial scaffold provides a four-region Dock layout, auxiliary tabs, independent SQL / Markdown / task editor entities, cell folding and task status changes, plus document and query models that can be tested without a window.

The minimal editor uses a 48px icon navigation rail with Data Note branding, projects, connections, settings and help. The top toolbar controls three regions, focus mode and Dark / Light themes. Output is collapsed by default. Theme selection applies only to the current session.

The 40px unified title bar combines Data Note branding, the product name, layout/theme actions and window controls in one row. The navigation rail starts below it. This replaces the separate native caption and toolbar; see the [title bar design (Chinese)](docs/plans/2026-09-11-unified-titlebar.md).

> Edits are currently kept only in memory and are lost when the window closes. Database execution, file saving, AI, Markdown preview and result tables are not connected. The sample does not execute SQL or start an AI process.

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

The domain layer does not depend on GPUI. Services currently define interfaces without simulated implementations. The desktop uses real GPUI Component controls and retains editor entities across renders. SQL highlighting, completion and formatting require dialect registration and services; these capabilities are not claimed as complete.

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

This produces `*-portable.zip`, `*-setup.exe` and SHA256 checksums from the same Release binary. The installer defaults to the current user's Programs folder, includes English/Chinese, creates a Start Menu shortcut and optionally a desktop shortcut, and provides an uninstaller. Exit the app before installation or upgrade; it will not be closed automatically. Portable mode currently means installation-free; configuration persistence is not implemented yet. Root-level bilingual changelogs generate the bundled release notes. Tag workflows upload both candidate artifacts without public release. See the [packaging plan (Chinese)](docs/plans/2026-09-11-release-v0.1.0.md).

Extract downloads into a separate folder and run `turbodbnote.exe`. Keep the license and instructions; do not replace a running EXE. Compare `Get-FileHash <downloaded-file> -Algorithm SHA256` with `SHA256SUMS.txt`. Candidate builds are unsigned. If Windows blocks a binary, verify its source instead of disabling system protection. Issue reports should include Windows version, GPU, reproduction steps and `BUILD-INFO.json`, without passwords, connection strings or sensitive queries.

## Documentation maintenance

Keep English and Chinese README sections, commands, versions and feature scope synchronized. Use matching `## [Unreleased]` / `## [0.1.0]` headings in `CHANGELOG.md` and `CHANGELOG.zh-CN.md`, with newest versions first and third-level headings inside each version. `docs/plans/` remains in Chinese for now. Do not maintain separate per-version release-note source files.

# Releasing TurboDBNote

[English](releasing.md) | [简体中文操作手册](releasing.zh-CN.md)

The public release has two platform packages: a Windows x64 portable ZIP and a Linux x64 tar.gz, plus SHA256SUMS.txt. Windows packaging also creates an installer and matching symbols as Actions artifacts. Do not put PDB files in the public packages.

## Next release

1. Commit features on the repository default branch using English messages. Add concise entries under `## [Unreleased]` in both root changelogs.
2. Start from a clean default branch matching origin. Run:

```powershell
./scripts/release.ps1 -Version 0.1.1
```

On Linux: `python3 scripts/prepare-release.py 0.1.1` (Python 3.11+).

The command updates Cargo/lockfile, Windows VERSIONINFO, release metadata, README versions and changelogs; creates an English release commit; and pushes it. The `[release]` marker triggers `.github/workflows/release.yml`. It does not create an early tag.

Actions verifies versions and translations, tests and lint, builds native Windows/Linux release packages, checks driver contracts and opens the Linux packaged window with isolated data under X11/Xvfb. Both jobs must pass. The publication job verifies checksums, generates bilingual notes from root CHANGELOG files, creates the immutable version tag at that exact source commit and publishes a single GitHub Release. Repeated versions are rejected; no automatic overwriting or moving tags.

To only build a candidate, use **Actions → Release Windows and Linux → Run workflow**, enter the already prepared version and leave **Publish** off. Check **Publish** to publish that same source after both platforms pass. If a build fails, fix the default-branch source and explicitly rerun; no release/tag was created by a failed build. If publication day changed, update the release metadata and changelog date to the intended actual publication date before rebuilding. Release preparation uses the release team's Asia/Shanghai calendar date, never the compiler timestamp.

Git push authorization is required once on the developer machine; the Actions job uses its repository GITHUB_TOKEN with write permission only for publication. Nothing is added to local PATH or root config files. Inspect progress at https://github.com/gavinwuz/TurboDbNote/actions/workflows/release.yml.

## Platforms

- Windows 10 1809+ / Windows 11 x64; unsigned. Use the existing isolated GPUI packaging adapter; retain MySQL native compatibility components and source/license materials.
- Linux x64: Ubuntu 22.04/glibc 2.35 baseline, X11/Wayland desktop and Vulkan. Requires fontconfig, libxkbcommon and unixODBC; additional providers such as OCI/ODBC are installed by the user. The archive includes `ldd` inventories. X11/Xvfb startup is a smoke check, not full GPU/Wayland/desktop acceptance.
- Linux credentials are session-only. DPAPI, tray hosting, Windows integrated SQL Server authentication and the bundled legacy MySQL DLL are Windows-only. Do not claim Windows verification as Linux real-server verification.

## Local packaging

Windows: `./scripts/package-windows.ps1 -IsccPath 'C:\Program Files (x86)\Inno Setup 6\ISCC.exe'` using PowerShell 7 and the standard SDK/toolchain.

Linux: install the packages listed in the release workflow, then `python3 scripts/package-linux.py`. Existing output paths are rejected. Generated source archives include explicitly selected project sources and never root `config/` or user data.

Driver contract verification uses the Rust example directly on both platforms (no Python dependency for this check):

```sh
cargo run --locked -j 1 -p turbodbnote-driver-host --example verify_contracts -- <package-directory>/drivers/native
```

The check starts only local fixtures and owned worker processes; it verifies bounded reads, rejected writes, invalid input handling and clean shutdown. Packaging and window smoke checks currently retain Python 3.11.

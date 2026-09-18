# Official SDK files (not committed)

Download matching binaries and headers from the CrashSight project's integration guide.

- Windows x64: supply the SDK folder to `scripts/package-windows.ps1 -CrashSightSdkDirectory ...`. It must include `CrashSight64.dll` version 2.2.1+ and all companion runtime files required by that distribution. The package copies the folder to `diagnostics/` beside the executable.
- macOS: build `native/crashsight/macos/bridge.m` against the provided `CrashSight.framework` using `scripts/build-crashsight-macos.sh`; include bridge and framework in `Contents/Frameworks`. Verify architecture, signing, runtime dependency paths, entitlements and notarization on macOS.

Runtime loads only these application-relative paths, never an arbitrary DLL from PATH/current directory. No SDK binary or App Key is included in this repository. Do not put symbol-upload tools or private configuration in the runtime SDK folder.

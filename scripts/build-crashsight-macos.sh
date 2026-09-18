#!/bin/bash
set -euo pipefail
# Usage: bash scripts/build-crashsight-macos.sh /path/to/SDK /path/to/output arm64
# SDK directory must contain the official CrashSight.framework, with headers.
sdk_dir="${1:?SDK directory required}"
output_dir="${2:?Output directory required}"
target_arch="${3:-arm64}"
case "$target_arch" in arm64|x86_64) ;; *) echo 'Expected arm64 or x86_64' >&2; exit 1;; esac
[[ "$(uname -s)" == Darwin ]] || { echo 'macOS/Xcode required' >&2; exit 1; }
[[ -d "$sdk_dir/CrashSight.framework" ]] || { echo 'Missing CrashSight.framework' >&2; exit 1; }
mkdir -p "$output_dir"
project_root="$(cd "$(dirname "$0")/.." && pwd)"
xcrun clang -dynamiclib -fobjc-arc -fvisibility=hidden -arch "$target_arch" \
  -F "$sdk_dir" -framework Foundation -framework CrashSight \
  -Wl,-rpath,@loader_path -Wl,-install_name,@rpath/libturbodbnote_crashsight.dylib \
  "$project_root/native/crashsight/macos/bridge.m" \
  -o "$output_dir/libturbodbnote_crashsight.dylib"
ditto "$sdk_dir/CrashSight.framework" "$output_dir/CrashSight.framework"
otool -L "$output_dir/libturbodbnote_crashsight.dylib"
echo 'Embed in Contents/Frameworks. Sign framework, bridge, and app with the same team before notarization.'

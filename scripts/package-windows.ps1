param(
    [string]$OutputDirectory = 'dist',
    [string]$FxcPath,
    [Parameter(Mandatory)][string]$IsccPath
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if (-not $IsWindows) { throw 'Windows PowerShell 7 is required.' }
$root = Split-Path $PSScriptRoot -Parent
. (Join-Path $PSScriptRoot 'release-notes.ps1')
. (Join-Path $PSScriptRoot 'windows-sdk.ps1')
. (Join-Path $PSScriptRoot 'prepare-package-build.ps1')
Push-Location $root
try {
    $compiler = Resolve-GpuiFxc -FxcPath $FxcPath
    if (-not (Test-Path -LiteralPath $IsccPath -PathType Leaf)) { throw "ISCC compiler not found: $IsccPath" }
    $IsccPath = (Resolve-Path -LiteralPath $IsccPath).Path
    Write-Output "GPUI shader compiler: $compiler"
    function Invoke-Checked([string]$Program, [string[]]$Arguments) {
        & $Program @Arguments
        if ($LASTEXITCODE -ne 0) { throw "$Program failed with exit code $LASTEXITCODE" }
    }
    $metadataJson = & cargo metadata --locked --no-deps --format-version 1
    if ($LASTEXITCODE -ne 0) { throw 'Cargo metadata failed' }
    $package = ($metadataJson | ConvertFrom-Json).packages | Where-Object name -eq 'turbodbnote'
    $version = $package.version
    if ($version -notmatch '^\d+\.\d+\.\d+$') { throw 'Packaging requires an x.y.z version' }
    if ($env:GITHUB_REF_TYPE -eq 'tag' -and $env:GITHUB_REF_NAME -ne "v$version") {
        throw 'Git tag and Cargo version do not match'
    }
    $name = "TurboDbNote-v$version-windows-x64"
    $output = [IO.Path]::GetFullPath((Join-Path $root $OutputDirectory))
    $stage = Join-Path $output $name
    $zip = Join-Path $output "$name-portable.zip"
    $setup = Join-Path $output "$name-setup.exe"
    $checksum = Join-Path $output 'SHA256SUMS.txt'
    foreach ($path in @($stage, $zip, $setup, $checksum)) {
        if (Test-Path -LiteralPath $path) { throw "Output already exists: $path" }
    }
    $notes = Get-VersionReleaseNotes -ChangelogPath (Join-Path $root 'CHANGELOG.md') -Version $version
    $notesZh = Get-VersionReleaseNotes -ChangelogPath (Join-Path $root 'CHANGELOG.zh-CN.md') -Version $version
    Invoke-Checked cargo @('fmt', '--all', '--', '--check')
    Invoke-Checked cargo @('test', '--locked', '-p', 'turbodbn-core', '-p', 'turbodbn-services')
    Invoke-Checked cargo @('clippy', '--locked', '--workspace', '--all-targets', '--', '-D', 'warnings')
    $buildWorkspace = New-PackageBuildWorkspace -Root $root -FxcPath $compiler
    $buildTarget = Join-Path $root 'target'
    Push-Location $buildWorkspace
    try {
        Invoke-Checked cargo @('build', '--locked', '--release', '--target', 'x86_64-pc-windows-msvc', '--target-dir', $buildTarget, '-p', 'turbodbnote')
    } finally { Pop-Location }
    $exe = Join-Path $buildTarget 'x86_64-pc-windows-msvc/release/turbodbnote.exe'
    Invoke-Checked python @('scripts/verify-windows-icon.py', $exe)
    $info = (Get-Item -LiteralPath $exe).VersionInfo
    if ($info.FileVersion -ne $version -or $info.ProductVersion -ne $version -or $info.ProductName -ne 'TurboDbNote') {
        throw 'Windows VERSIONINFO and Cargo version do not match'
    }
    $commit = & git rev-parse HEAD
    if ($LASTEXITCODE -ne 0) { throw 'Unable to resolve Git commit' }
    $changes = & git status --porcelain
    if ($LASTEXITCODE -ne 0) { throw 'Unable to read Git status' }
    $rust = & rustc --version
    if ($LASTEXITCODE -ne 0) { throw 'Unable to resolve Rust version' }
    New-Item -ItemType Directory -Path $stage -Force | Out-Null
    Copy-Item -LiteralPath $exe -Destination (Join-Path $stage 'turbodbnote.exe')
    Copy-Item -LiteralPath (Join-Path $root 'LICENSE') -Destination $stage
    $notes | Set-Content -LiteralPath (Join-Path $stage 'RELEASE-NOTES.md') -Encoding utf8
    $notesZh | Set-Content -LiteralPath (Join-Path $stage 'RELEASE-NOTES.zh-CN.md') -Encoding utf8
    @"
TurboDbNote v$version (Preview) / Windows x64
Run turbodbnote.exe from this extracted folder.
Edits are in-memory only and are LOST when the app exits.
File saving and database execution are not available yet.
AI Chat requires a separately installed Codex or Claude Code CLI and its configured login.
This build is unsigned. Read RELEASE-NOTES.md before use.
简体中文：编辑内容仅保留在内存中，退出后丢失。使用前请阅读 RELEASE-NOTES.zh-CN.md。
"@ | Set-Content -LiteralPath (Join-Path $stage 'README.txt') -Encoding utf8
    [ordered]@{ version=$version; commit=$commit; dirty=[bool]$changes; rust=$rust;
        target='x86_64-pc-windows-msvc'; gpui_adapter='0.2.2-fxc-file-v1'; fxc_version=(Get-Item $compiler).VersionInfo.FileVersion;
        built_at=[DateTime]::UtcNow.ToString('o') } |
        ConvertTo-Json | Set-Content -LiteralPath (Join-Path $stage 'BUILD-INFO.json') -Encoding utf8
    'Portable settings are stored in data/settings.json beside the executable.' |
        Set-Content -LiteralPath (Join-Path $stage 'portable.flag') -Encoding ascii
    Compress-Archive -LiteralPath $stage -DestinationPath $zip -CompressionLevel Optimal
    Invoke-Checked $IsccPath @("/DAppVersion=$version", "/DSourceDir=$stage", "/DOutputDir=$output",
        "/DIconFile=$(Join-Path $root 'crates/desktop/assets/turbodbnote.ico')", (Join-Path $root 'installer/turbodbnote.iss'))
    if (-not (Test-Path -LiteralPath $setup)) { throw 'Installer was not generated' }
    @($zip, $setup) | ForEach-Object {
        $hash = (Get-FileHash -LiteralPath $_ -Algorithm SHA256).Hash.ToLowerInvariant()
        "$hash  $([IO.Path]::GetFileName($_))"
    } | Set-Content -LiteralPath $checksum -Encoding ascii
    Write-Output "Candidate package: $zip"
    Write-Output "Candidate installer: $setup"
    Write-Output "Checksums: $checksum"
} finally {
    Pop-Location
}

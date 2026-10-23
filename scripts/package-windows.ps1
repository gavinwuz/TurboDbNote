param(
    [string]$OutputDirectory = 'dist',
    [string]$FxcPath,
    [string]$LegacyDriverArchive,
    [string]$LegacySourceArchive,
    [string]$OracleClientArchive,
    [string]$Db2ClientArchive,
    [string]$CrashSightSdkDirectory,
    [Parameter(Mandatory)][string]$IsccPath
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if (-not $IsWindows) { throw 'Windows PowerShell 7 is required.' }
$root = Split-Path $PSScriptRoot -Parent
. (Join-Path $PSScriptRoot 'release-notes.ps1')
. (Join-Path $PSScriptRoot 'windows-sdk.ps1')
. (Join-Path $PSScriptRoot 'prepare-package-build.ps1')
. (Join-Path $PSScriptRoot 'stage-crashsight.ps1')
. (Join-Path $PSScriptRoot 'stage-mysql-driver.ps1')
. (Join-Path $PSScriptRoot 'stage-database-drivers.ps1')
$targetRoot = Join-Path $root 'target'
New-Item -ItemType Directory -Path $targetRoot -Force | Out-Null
$packageLock = [IO.File]::Open((Join-Path $targetRoot 'package-build.lock'), [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
Push-Location $root
try {
    if ($LegacySourceArchive -and -not $LegacyDriverArchive) { throw 'LegacySourceArchive requires LegacyDriverArchive' }
    if ($LegacyDriverArchive) { $LegacyDriverArchive = (Resolve-Path -LiteralPath $LegacyDriverArchive).Path }
    if ($LegacySourceArchive) { $LegacySourceArchive = (Resolve-Path -LiteralPath $LegacySourceArchive).Path }
    $compiler = Resolve-GpuiFxc -FxcPath $FxcPath
    if (-not (Test-Path -LiteralPath $IsccPath -PathType Leaf)) { throw "ISCC compiler not found: $IsccPath" }
    $IsccPath = (Resolve-Path -LiteralPath $IsccPath).Path
    Write-Output "GPUI shader compiler: $compiler"
    & cargo build --locked -j 1 -p turbodbnote-tools
    if ($LASTEXITCODE -ne 0) { throw 'Native development tools build failed' }
    $devTools = Join-Path $root 'target/debug/turbodbnote-tools.exe'
    function Invoke-Checked([string]$Program, [string[]]$Arguments) {
        if ($Program -eq 'cargo') {
            & $devTools run-cargo @Arguments
        } else {
            & $Program @Arguments
        }
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
    $name = "TurboDBNote-v$version-windows-x64"
    $output = [IO.Path]::GetFullPath((Join-Path $root $OutputDirectory))
    $stage = Join-Path $output $name
    $zip = Join-Path $output "$name-portable.zip"
    $setup = Join-Path $output "$name-setup.exe"
    $sourcesZip = Join-Path $output "$name-sources.zip"
    $checksum = Join-Path $output 'SHA256SUMS.txt'
    foreach ($path in @($stage, $zip, $setup, $sourcesZip, $checksum, (Join-Path $output "$name-drivers.zip"), (Join-Path $output "$name-minimal"), (Join-Path $output "$name-minimal-portable.zip"), (Join-Path $output "$name-minimal-setup.exe"), (Join-Path $output "$name-minimal-sources.zip"))) {
        if (Test-Path -LiteralPath $path) { throw "Output already exists: $path" }
    }
    $notes = Get-VersionReleaseNotes -ChangelogPath (Join-Path $root 'CHANGELOG.md') -Version $version
    $notesZh = Get-VersionReleaseNotes -ChangelogPath (Join-Path $root 'CHANGELOG.zh-CN.md') -Version $version
    $oraclePreflight = if ($OracleClientArchive) { (Resolve-Path -LiteralPath $OracleClientArchive).Path } else { '-' }
    $db2Preflight = if ($Db2ClientArchive) { (Resolve-Path -LiteralPath $Db2ClientArchive).Path } else { '-' }
    Invoke-Checked $devTools @('prefetch-clients', 'windows-x64', $oraclePreflight, $db2Preflight)
    Invoke-Checked cargo @('fmt', '--all', '--', '--check')
    Invoke-Checked cargo @('test', '--locked', '-j', '1', '-p', 'turbodbn-core', '-p', 'turbodbn-services', '-p', 'turbodbnote-driver-host', '--', '--test-threads=1')
    Invoke-Checked cargo @('clippy', '--locked', '-j', '1', '--workspace', '--all-targets', '--', '-D', 'warnings')
    $buildWorkspace = New-PackageBuildWorkspace -Root $root -FxcPath $compiler
    $buildTarget = Join-Path $root 'target'
    Push-Location $buildWorkspace
    try {
        Invoke-Checked cargo @('build', '--locked', '-j', '1', '--release', '--target', 'x86_64-pc-windows-msvc', '--target-dir', $buildTarget, '-p', 'turbodbnote', '-p', 'turbodbnote-driver-host')
    } finally { Pop-Location }
    $exe = Join-Path $buildTarget 'x86_64-pc-windows-msvc/release/turbodbnote.exe'
    Invoke-Checked python @('scripts/verify-windows-icon.py', $exe)
    $info = (Get-Item -LiteralPath $exe).VersionInfo
    if ($info.FileVersion -ne $version -or $info.ProductVersion -ne $version -or $info.ProductName -ne 'TurboDBNote') {
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
    Copy-Item -LiteralPath (Join-Path $buildTarget 'x86_64-pc-windows-msvc/release/turbodbnote-update-helper.exe') -Destination $stage
    $legacyHelper=Join-Path $buildTarget 'x86_64-pc-windows-msvc/release/turbodbnote-mysql-legacy.exe'
    if (-not (Test-Path -LiteralPath $legacyHelper)) {throw 'Legacy helper was not built'}
    Copy-Item -LiteralPath $legacyHelper -Destination $stage
    # Curated x64 client supports legacy 4.x servers; win32 4.x DLLs cannot load in this app.
    Copy-Item -LiteralPath (Join-Path $root 'LICENSE') -Destination (Join-Path $stage 'LICENSE')
    Stage-DatabaseDrivers -Destination (Join-Path $stage 'drivers/native') -HostPath (Join-Path $buildTarget 'x86_64-pc-windows-msvc/release/turbodbnote-driver-host.exe')
    Stage-MySqlDriver -Destination (Join-Path $stage 'drivers') -LegacyDriverArchive $LegacyDriverArchive -LegacySourceArchive $LegacySourceArchive
    Copy-Item -LiteralPath (Join-Path $root 'LICENSE') -Destination (Join-Path $stage 'drivers/native/LICENSE.TurboDBNote')
    $oracleArgs = @('prepare-oracle', (Join-Path $root 'target/tools/oracle-bundle'), (Join-Path $stage 'drivers/native'), 'windows-x64')
    if ($OracleClientArchive) { $oracleArgs += (Resolve-Path -LiteralPath $OracleClientArchive).Path }
    Invoke-Checked $devTools $oracleArgs
    $db2Args = @('prepare-db2', (Join-Path $root 'target/tools/db2-client'), (Join-Path $stage 'drivers/native'))
    if ($Db2ClientArchive) { $db2Args += (Resolve-Path -LiteralPath $Db2ClientArchive).Path }
    Invoke-Checked $devTools $db2Args
    $driverArchive = Join-Path $output "$name-drivers.zip"
    Invoke-Checked $devTools @('driver-components', (Join-Path $stage 'drivers/native'), $driverArchive, $version, 'windows-x64')
    'full' | Set-Content -LiteralPath (Join-Path $stage 'package-flavor.txt') -Encoding ascii
    # Include the exact application source used for this build alongside the third-party source.
    $sourceStage = Join-Path $stage 'sources/TurboDBNote'
    $sourceFiles = & git ls-files --cached --others --exclude-standard
    if ($LASTEXITCODE -ne 0) { throw 'Unable to enumerate release source' }
    foreach ($sourceFile in ($sourceFiles | Sort-Object -Unique)) {
        if ($sourceFile -notmatch '^(crates/|drivers/|scripts/|installer/|vendor/gpui-component/|Cargo\.(toml|lock)$|LICENSE$|README|CHANGELOG)' -or -not (Test-Path -LiteralPath $sourceFile -PathType Leaf)) { continue }
        $destination = Join-Path $sourceStage $sourceFile
        New-Item -ItemType Directory -Path (Split-Path $destination -Parent) -Force | Out-Null
        Copy-Item -LiteralPath $sourceFile -Destination $destination
    }

    if ($CrashSightSdkDirectory) {
        Copy-CrashSightRuntime -SdkDirectory $CrashSightSdkDirectory -Destination (Join-Path $stage 'diagnostics')
    }
    # Preserve matching EXE/PDB privately for symbol upload; do not bundle PDBs.
    $symbols = Join-Path $output "$name-symbols"
    if (Test-Path -LiteralPath $symbols) { throw 'Symbol output already exists' }
    $pdb = [IO.Path]::ChangeExtension($exe, '.pdb')
    if (-not (Test-Path -LiteralPath $pdb -PathType Leaf)) { throw 'Release PDB missing; crash reports would not be symbolicated' }
    New-Item -ItemType Directory -Path $symbols | Out-Null
    Copy-Item -LiteralPath $exe, $pdb -Destination $symbols
    foreach ($nativeSymbolName in @('turbodbnote-update-helper.exe', 'turbodbnote_update_helper.pdb', 'turbodbnote-driver-host.exe', 'turbodbnote_driver_host.pdb', 'turbodbnote-mysql-legacy.exe', 'turbodbnote_mysql_legacy.pdb')) {
        $nativeSymbolFile = Join-Path $buildTarget "x86_64-pc-windows-msvc/release/$nativeSymbolName"
        if (-not (Test-Path -LiteralPath $nativeSymbolFile -PathType Leaf)) { throw 'Native driver symbols missing' }
        Copy-Item -LiteralPath $nativeSymbolFile -Destination $symbols
    }
    Copy-Item -LiteralPath (Join-Path $root 'LICENSE') -Destination $stage
    $notes | Set-Content -LiteralPath (Join-Path $stage 'RELEASE-NOTES.md') -Encoding utf8
    $notesZh | Set-Content -LiteralPath (Join-Path $stage 'RELEASE-NOTES.zh-CN.md') -Encoding utf8
    @"
TurboDBNote v$version / Windows x64
Run turbodbnote.exe from this extracted folder.
Notes persist as individual .note files. Application data is stored in connections.db.
MySQL compatibility components are included in the adjacent drivers directory.
AI Chat requires a separately installed Codex or Claude Code CLI and its configured login.
This build is unsigned. Read RELEASE-NOTES.md before use.
简体中文：笔记独立持久化为 .note 文件，MySQL 兼容组件随包集成。使用前请阅读 RELEASE-NOTES.zh-CN.md。
"@ | Set-Content -LiteralPath (Join-Path $stage 'README.txt') -Encoding utf8
    [ordered]@{ version=$version; commit=$commit; dirty=[bool]$changes; rust=$rust;
        target='x86_64-pc-windows-msvc'; gpui_adapter='0.2.2-fxc-file-v1'; fxc_version=(Get-Item $compiler).VersionInfo.FileVersion;
        built_at=[DateTime]::UtcNow.ToString('o') } |
        ConvertTo-Json | Set-Content -LiteralPath (Join-Path $stage 'BUILD-INFO.json') -Encoding utf8
    'Portable settings are stored in data/connections.db beside the executable.' |
        Set-Content -LiteralPath (Join-Path $stage 'portable.flag') -Encoding ascii
    Write-WindowsReleaseArchives -Stage $stage -PortableZip $zip -SourcesZip $sourcesZip
    Invoke-Checked $IsccPath @("/DAppVersion=$version", "/DSourceDir=$stage", "/DOutputDir=$output",
        "/DIconFile=$(Join-Path $root 'crates/desktop/assets/turbodbnote.ico')", (Join-Path $root 'installer/turbodbnote.iss'))
    if (-not (Test-Path -LiteralPath $setup)) { throw 'Installer was not generated' }
    $minimalStage = Join-Path $output "$name-minimal"
    $minimalZip = Join-Path $output "$name-minimal-portable.zip"
    $minimalSetup = Join-Path $output "$name-minimal-setup.exe"
    foreach ($path in @($minimalStage, $minimalZip, $minimalSetup)) { if (Test-Path -LiteralPath $path) { throw "Output already exists: $path" } }
    New-Item -ItemType Directory -Path $minimalStage | Out-Null
    foreach ($item in Get-ChildItem -LiteralPath $stage) {
        if ($item.Name -eq 'drivers') {
            New-Item -ItemType Directory -Path (Join-Path $minimalStage 'drivers') | Out-Null
            foreach ($driverItem in Get-ChildItem -LiteralPath $item.FullName | Where-Object Name -ne 'native') {
                Copy-Item -LiteralPath $driverItem.FullName -Destination (Join-Path $minimalStage 'drivers') -Recurse
            }
        } else { Copy-Item -LiteralPath $item.FullName -Destination $minimalStage -Recurse }
    }
    'minimal' | Set-Content -LiteralPath (Join-Path $minimalStage 'package-flavor.txt') -Encoding ascii
    $minimalSources = Join-Path $output "$name-minimal-sources.zip"
    Write-WindowsReleaseArchives -Stage $minimalStage -PortableZip $minimalZip -SourcesZip $minimalSources
    Invoke-Checked $IsccPath @("/DAppVersion=$version", '/DAppFlavor=minimal', "/DSourceDir=$minimalStage", "/DOutputDir=$output", "/DIconFile=$(Join-Path $root 'crates/desktop/assets/turbodbnote.ico')", (Join-Path $root 'installer/turbodbnote.iss'))
    if (-not (Test-Path -LiteralPath $minimalSetup)) { throw 'Minimal installer was not generated' }
    Write-ReleaseChecksums -Paths @($zip, $setup, $sourcesZip, $minimalZip, $minimalSetup, $minimalSources, $driverArchive) -OutputFile $checksum
    Write-Output "Package: $zip"
    Write-Output "Installer: $setup"
    Write-Output "Sources: $sourcesZip"
    Write-Output "Checksums: $checksum"
} finally {
    Pop-Location
    $packageLock.Dispose()
}

param([string]$CargoPath = 'cargo', [string]$LegacyDriverArchive, [string]$LegacySourceArchive)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root = Split-Path $PSScriptRoot -Parent
. (Join-Path $PSScriptRoot 'stage-mysql-driver.ps1')
. (Join-Path $PSScriptRoot 'stage-database-drivers.ps1')
$target = Join-Path $root 'target'
$preview = Join-Path $target 'preview'
New-Item -ItemType Directory -Path $preview -Force | Out-Null
$guard = [IO.File]::Open((Join-Path $target 'preview-build.lock'), [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
Push-Location $root
try {
    $running = Get-CimInstance Win32_Process | Where-Object {
        $_.ExecutablePath -and $_.ExecutablePath.StartsWith($preview + '\', [StringComparison]::OrdinalIgnoreCase)
    }
    if ($running) { throw 'Close the preview application before updating it. Its data will be preserved.' }
    & $CargoPath build --locked -j 1 --target-dir $target -p turbodbnote -p turbodbnote-driver-host --bins
    if ($LASTEXITCODE -ne 0) { throw 'Preview build failed' }
    Stage-DatabaseDrivers -Destination (Join-Path $preview 'drivers/native') -HostPath (Join-Path $target 'debug/turbodbnote-driver-host.exe')
    Stage-MySqlDriver -Destination (Join-Path $preview 'drivers') -CargoPath $CargoPath -LegacyDriverArchive $LegacyDriverArchive -LegacySourceArchive $LegacySourceArchive
    foreach ($name in @('turbodbnote-update-helper.exe', 'turbodbnote_update_helper.pdb', 'turbodbnote.exe', 'turbodbnote.pdb', 'turbodbnote-mysql-legacy.exe', 'turbodbnote_mysql_legacy.pdb')) {
        Copy-Item -LiteralPath (Join-Path $target "debug/$name") -Destination (Join-Path $preview $name) -Force
    }
    if (-not (Test-Path -LiteralPath (Join-Path $preview 'portable.flag'))) {
        'Reusable local preview' | Set-Content -LiteralPath (Join-Path $preview 'portable.flag')
    }
    Write-Output "Preview: $(Join-Path $preview 'turbodbnote.exe')"
} finally {
    Pop-Location
    $guard.Dispose()
}

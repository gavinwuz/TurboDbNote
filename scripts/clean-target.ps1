param([switch]$Apply, [switch]$ObsoleteDriverRuntime, [switch]$ObsoleteJdbcRuntime)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root = [IO.Path]::GetFullPath((Join-Path (Split-Path $PSScriptRoot -Parent) 'target'))
if (-not (Test-Path -LiteralPath $root)) { return }
$prefix = $root + [IO.Path]::DirectorySeparatorChar
if ((Get-Item -LiteralPath $root).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Refusing linked target directory' }
if (Get-Process cargo, rustc, link -ErrorAction SilentlyContinue) { throw 'Wait for compilation to finish before cleaning' }
$running = @(Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -and $_.ExecutablePath.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase) })
$protected = @($running | ForEach-Object { Split-Path $_.ExecutablePath -Parent })
if ($ObsoleteJdbcRuntime) {
    $nativeRoot = [IO.Path]::GetFullPath((Join-Path $root 'preview/drivers/native'))
    if (-not (Test-Path -LiteralPath (Join-Path $nativeRoot 'jre'))) { return }
    $previewRoot = Join-Path $root 'preview'
    if ($running | Where-Object { $_.ExecutablePath.StartsWith($previewRoot + '\', [StringComparison]::OrdinalIgnoreCase) }) { throw 'Close the preview before removing retired JDBC resources' }
    foreach ($folder in @($previewRoot, (Join-Path $previewRoot 'drivers'), $nativeRoot)) {
        if ((Get-Item -LiteralPath $folder).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Refusing linked driver directory' }
    }
    if (-not (Test-Path -LiteralPath (Join-Path $nativeRoot '.runtime-sha256'))) { throw 'Missing generated-runtime ownership marker' }
    $manifest = Get-Content -LiteralPath (Join-Path $nativeRoot 'runtime-lock.json') -Raw | ConvertFrom-Json
    if (@($manifest.artifacts | Where-Object { $_.file -eq 'h2.jar' }).Count -ne 1) { throw 'Not the retired JDBC bundle' }
    $removals = @('jre', 'jars', 'H2Host.class', '.runtime-sha256', 'runtime-lock.json') | ForEach-Object { Join-Path $nativeRoot $_ } | Where-Object { Test-Path -LiteralPath $_ }
    $bytes = 0L
    foreach ($candidate in $removals) {
        $resolved = (Resolve-Path -LiteralPath $candidate).Path
        if (-not $resolved.StartsWith($nativeRoot + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Cleanup escaped native resources' }
        $item = Get-Item -LiteralPath $resolved -Force
        $items = @($item)
        if ($item.PSIsContainer) { $items += @(Get-ChildItem -LiteralPath $resolved -Recurse -Force) }
        if ($items | Where-Object { ($_.Attributes -band [IO.FileAttributes]::ReparsePoint) -or $_.Extension -in @('.note', '.db', '.mdb', '.accdb') }) { throw 'Refusing linked files or user data inside JDBC resources' }
        $bytes += ($items | Where-Object { -not $_.PSIsContainer } | Measure-Object Length -Sum).Sum
    }
    Write-Output ('Retired generated JDBC resources: {0:N1} MiB. Rust drivers and application data are preserved.' -f ($bytes / 1MB))
    if ($Apply) { foreach ($candidate in $removals) { Remove-Item -LiteralPath $candidate -Recurse -Force } }
    return
}
if ($ObsoleteDriverRuntime) {
    $previewRoot = Join-Path $root 'preview'
    $obsolete = Join-Path $previewRoot 'drivers/runtime'
    if (-not (Test-Path -LiteralPath $obsolete)) { return }
    if ($running | Where-Object { $_.ExecutablePath.StartsWith($previewRoot + '\', [StringComparison]::OrdinalIgnoreCase) }) { throw 'Close the preview application before removing its obsolete driver runtime' }
    if (-not (Test-Path -LiteralPath (Join-Path $previewRoot 'drivers/native/turbodbnote-driver-host.exe'))) { throw 'Stage and verify the native replacement first' }
    foreach ($folder in @($previewRoot, (Join-Path $previewRoot 'drivers'), $obsolete)) {
        if ((Get-Item -LiteralPath $folder).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Refusing linked driver directory' }
    }
    $resolved = (Resolve-Path -LiteralPath $obsolete).Path
    if ($resolved -ne [IO.Path]::GetFullPath($obsolete) -or -not $resolved.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) { throw 'Unexpected obsolete runtime path' }
    if (-not (Test-Path -LiteralPath (Join-Path $resolved '.runtime-sha256'))) { throw 'Missing generated-runtime ownership marker' }
    $manifest = Get-Content -LiteralPath (Join-Path $resolved 'runtime-lock.json') -Raw | ConvertFrom-Json
    if (-not $manifest.PSObject.Properties['python']) { throw 'Not the obsolete Python driver bundle' }
    $contents = @(Get-ChildItem -LiteralPath $resolved -Recurse -Force)
    if ($contents | Where-Object { $_.Attributes -band [IO.FileAttributes]::ReparsePoint -or $_.Extension -in @('.note', '.db', '.accdb', '.mdb') }) { throw 'Refusing linked files or user data inside driver resources' }
    $bytes = ($contents | Where-Object { -not $_.PSIsContainer } | Measure-Object Length -Sum).Sum
    Write-Output ('Obsolete generated Python driver resources: {0:N1} MiB. Native drivers and application data are preserved.' -f ($bytes / 1MB))
    if ($Apply) { Remove-Item -LiteralPath $resolved -Recurse -Force }
    return
}
$files = [Collections.Generic.List[IO.FileInfo]]::new()
# Keep tool downloads, Cargo dependency artifacts, current preview and all runtime data.
foreach ($folder in @(Get-Item -LiteralPath $root) + @(Get-ChildItem -LiteralPath $root -Directory)) {
    if ($folder.Attributes -band [IO.FileAttributes]::ReparsePoint) { continue }
    if ($folder.FullName -ne $root -and $folder.Name -notmatch 'review|preview|workbench|^toolbar-|^connection-|^filter-|^grid-|^grouping-|^settings-|^project-|^import-font|^multi-database|^large-database|^tls-guidance|^driver-ui|^driver-bundled|^database-') { continue }
    if ($folder.Name -eq 'preview' -or $protected.Contains($folder.FullName)) { continue }
    foreach ($file in Get-ChildItem -LiteralPath $folder.FullName -File) {
        if ($file.Attributes -band [IO.FileAttributes]::ReparsePoint) { continue }
        if ($file.Name -match '^turbodb(note|n).*(\.(exe|pdb|exp|lib|o|d)|\.exe\.sha256)$') { $files.Add($file) }
    }
}
$incremental = Join-Path $root 'debug/incremental'
$cacheBytes = 0L
if (Test-Path -LiteralPath $incremental) {
    $cache = Get-Item -LiteralPath $incremental
    if ($cache.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Refusing linked incremental cache' }
    $contents = @(Get-ChildItem -LiteralPath $incremental -Recurse -Force)
    if ($contents | Where-Object { $_.Attributes -band [IO.FileAttributes]::ReparsePoint }) { throw 'Refusing links inside incremental cache' }
    $cacheBytes = ($contents | Where-Object { -not $_.PSIsContainer } | Measure-Object Length -Sum).Sum
}
$bytes = ($files | Measure-Object Length -Sum).Sum + $cacheBytes
Write-Output ('Reclaimable: {0:N2} GiB; old binary files: {1}; runtime data is preserved.' -f ($bytes / 1GB), $files.Count)
if (-not $Apply) { return }
foreach ($file in $files) {
    $path = [IO.Path]::GetFullPath($file.FullName)
    if (-not $path.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) { throw 'Cleanup path escaped target' }
    Remove-Item -LiteralPath $path -Force
}
if (Test-Path -LiteralPath $incremental) {
    $resolved = (Resolve-Path -LiteralPath $incremental).Path
    if ($resolved -ne (Join-Path $root 'debug/incremental')) { throw 'Unexpected incremental cache target' }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
Write-Output ('Removed {0:N2} GiB.' -f ($bytes / 1GB))

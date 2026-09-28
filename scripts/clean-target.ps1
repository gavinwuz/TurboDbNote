param([switch]$Apply)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root = [IO.Path]::GetFullPath((Join-Path (Split-Path $PSScriptRoot -Parent) 'target'))
if (-not (Test-Path -LiteralPath $root)) { return }
$prefix = $root + [IO.Path]::DirectorySeparatorChar
if ((Get-Item -LiteralPath $root).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Refusing linked target directory' }
if (Get-Process cargo, rustc, link -ErrorAction SilentlyContinue) { throw 'Wait for compilation to finish before cleaning' }
$running = @(Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -and $_.ExecutablePath.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase) })
$protected = @($running | ForEach-Object { Split-Path $_.ExecutablePath -Parent })
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

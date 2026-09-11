function Resolve-GpuiFxc {
    param([string]$FxcPath)
    # GPUI 0.2.2 otherwise falls back to one hard-coded SDK version.
    if ($FxcPath) {
        if (-not (Test-Path -LiteralPath $FxcPath -PathType Leaf)) {
            throw "FxcPath does not point to a file: $FxcPath"
        }
        return (Resolve-Path -LiteralPath $FxcPath).Path
    }
    $roots = @()
    foreach ($key in @('HKLM:\SOFTWARE\Microsoft\Windows Kits\Installed Roots',
                       'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows Kits\Installed Roots')) {
        $kits = Get-ItemProperty -LiteralPath $key -Name KitsRoot10 -ErrorAction SilentlyContinue
        if ($kits) { $roots += $kits.KitsRoot10 }
    }
    $programFiles = [Environment]::GetFolderPath('ProgramFilesX86')
    if ($programFiles) { $roots += Join-Path $programFiles 'Windows Kits/10' }
    $candidates = foreach ($root in ($roots | Where-Object { $_ } | Select-Object -Unique)) {
        $bin = Join-Path $root 'bin'
        foreach ($directory in (Get-ChildItem -LiteralPath $bin -Directory -ErrorAction SilentlyContinue)) {
            $sdkVersion = $null
            if ([version]::TryParse($directory.Name, [ref]$sdkVersion)) {
                $compiler = Join-Path $directory.FullName 'x64/fxc.exe'
                if (Test-Path -LiteralPath $compiler -PathType Leaf) {
                    [pscustomobject]@{ Version = $sdkVersion; Path = $compiler }
                }
            }
        }
    }
    $latest = $candidates | Sort-Object Version -Descending | Select-Object -First 1
    if ($latest) { return $latest.Path }
    throw 'fxc.exe was not found. Install Windows SDK or pass -FxcPath with the full path to fxc.exe.'
}

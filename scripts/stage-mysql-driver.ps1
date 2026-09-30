# Shared by preview and release packaging. No server binaries are redistributed.
function Stage-MySqlDriver {
    param(
        [Parameter(Mandatory)][string]$Destination,
        [string]$CargoPath = 'cargo',
        [string]$LegacyDriverArchive,
        [string]$LegacySourceArchive
    )
    if ($LegacySourceArchive -and -not $LegacyDriverArchive) { throw 'LegacySourceArchive requires LegacyDriverArchive' }
    $repo = Split-Path $PSScriptRoot -Parent
    $cache = Join-Path $repo 'target/tools/mysql-bundle'
    New-Item -ItemType Directory -Path $cache -Force | Out-Null
    $lock = $null
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    while (-not $lock) {
        try { $lock = [IO.File]::Open((Join-Path $cache 'stage.lock'), [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None) }
        catch [IO.IOException] {
            if ([DateTime]::UtcNow -ge $deadline) { throw 'Another build is preparing the MySQL bundle; retry when it finishes.' }
            Start-Sleep -Milliseconds 200
        }
    }
    Push-Location $repo
    try {
        $arguments = @('run', '--locked', '-j', '1', '-p', 'turbodbn-services', '--example', 'prepare_mysql_driver', '--', $cache)
        if ($LegacyDriverArchive) { $arguments += (Resolve-Path -LiteralPath $LegacyDriverArchive).Path }
        if ($LegacySourceArchive) { $arguments += (Resolve-Path -LiteralPath $LegacySourceArchive).Path }
        & $CargoPath @arguments
        if ($LASTEXITCODE -ne 0) { throw 'Unable to prepare verified MySQL compatibility bundle' }
        $id = 'mysql-5.1.73-winx64'
        $source = Join-Path $cache $id
        $destinationRoot = [IO.Path]::GetFullPath($Destination)
        $target = Join-Path $destinationRoot $id
        New-Item -ItemType Directory -Path $target -Force | Out-Null
        foreach ($name in @('libmysql.dll', 'COPYING', 'README.mysql', 'provenance.json', 'mysql-5.1.73-source.tar.gz')) {
            $file = Join-Path $source $name
            if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { throw "Compatibility bundle missing: $name" }
            Copy-Item -LiteralPath $file -Destination (Join-Path $target $name) -Force
            if ((Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash -ne (Get-FileHash -LiteralPath (Join-Path $target $name) -Algorithm SHA256).Hash) {
                throw "Compatibility bundle copy verification failed: $name"
            }
        }
        Write-Output "MySQL compatibility bundle: $target"
    } finally {
        Pop-Location
        $lock.Dispose()
    }
}

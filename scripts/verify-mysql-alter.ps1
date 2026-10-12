param([Parameter(Mandatory)][string]$MySqlBin, [string]$CargoPath = 'cargo')
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$fixture = Join-Path $root 'target/ui-verification/mysql-alter'
$data = Join-Path $fixture 'data'
$marker = Join-Path $fixture '.owned-fixture'
$server = Join-Path $MySqlBin 'mysqld.exe'
if (-not (Test-Path -LiteralPath $server)) { throw 'mysqld.exe not found' }
New-Item -ItemType Directory -Path $fixture -Force | Out-Null
$lock = [IO.File]::Open((Join-Path $fixture 'verify.lock'), [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
$owned = $null
try {
    $existingPattern = '--datadir="?' + [regex]::Escape($data) + '"?(?:\s|$)'
    if (Get-CimInstance Win32_Process -Filter "Name='mysqld.exe'" | Where-Object { $_.CommandLine -match $existingPattern }) { throw 'The fixture data directory is already in use; existing servers are not stopped' }
    if (Test-Path -LiteralPath $data) {
        if (-not (Test-Path -LiteralPath $marker)) { throw 'Existing data has no owned-fixture marker' }
    } else {
        $init = Start-Process -FilePath $server -ArgumentList @('--no-defaults', '--initialize-insecure', "--basedir=`"$(Split-Path $MySqlBin -Parent)`"", "--datadir=`"$data`"") -WindowStyle Hidden -PassThru -Wait -RedirectStandardError (Join-Path $fixture 'init.log')
        if ($init.ExitCode -ne 0) { throw 'Isolated MySQL initialization failed' }
        'TurboDBNote owned ALTER fixture' | Set-Content -LiteralPath $marker
    }
    $listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback,0)
    $listener.Start(); $port = $listener.LocalEndpoint.Port; $listener.Stop()
    $owned = Start-Process -FilePath $server -ArgumentList @('--no-defaults', "--basedir=`"$(Split-Path $MySqlBin -Parent)`"", "--datadir=`"$data`"", '--bind-address=127.0.0.1', "--port=$port", '--mysqlx=0', '--skip-log-bin', '--console') -WindowStyle Hidden -PassThru -RedirectStandardError (Join-Path $fixture 'server.log')
    $deadline = [DateTime]::UtcNow.AddSeconds(45)
    $ready = $false
    while ([DateTime]::UtcNow -lt $deadline -and -not $owned.HasExited) {
        $socket = [Net.Sockets.TcpClient]::new()
        try { $socket.Connect('127.0.0.1',$port); $ready = $true; break } catch { Start-Sleep -Milliseconds 200 } finally { $socket.Dispose() }
    }
    if (-not $ready) { throw 'Isolated MySQL did not start' }
    Push-Location $root
    try {
        & $CargoPath run --locked -j 1 -p turbodbn-services --example mysql_alter_probe -- --isolated-fixture $port $data
        if ($LASTEXITCODE -ne 0) { throw 'ALTER regression probe failed' }
    } finally { Pop-Location }
} finally {
    if ($owned -and -not $owned.HasExited) { Stop-Process -Id $owned.Id }
    # Windows can spawn a monitor/server pair; match only this owned fixture.
    if ($owned) {
        $pattern = '--datadir="?' + [regex]::Escape($data) + '"?(?:\s|$)'
        Get-CimInstance Win32_Process -Filter "Name='mysqld.exe'" |
            Where-Object { $_.CommandLine -match $pattern } |
            ForEach-Object { Stop-Process -Id $_.ProcessId -ErrorAction SilentlyContinue }
    }
    $lock.Dispose()
}

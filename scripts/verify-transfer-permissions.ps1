param([Parameter(Mandatory)][string]$MySqlBin, [string]$UiScript)
$ErrorActionPreference='Stop'
$repo=Split-Path $PSScriptRoot -Parent
$verifyRoot=Join-Path $repo 'target/mysql-permission-smoke'
New-Item -ItemType Directory -Path $verifyRoot -Force | Out-Null
$guard=[IO.File]::Open((Join-Path $verifyRoot 'verify.lock'),[IO.FileMode]::OpenOrCreate,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None)
$serverProcess=$null
try {
    $listener=[Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback,0)
    $listener.Start();$port=$listener.LocalEndpoint.Port;$listener.Stop()
    $server=Join-Path $MySqlBin 'mysqld.exe'
    $client=Join-Path $MySqlBin 'mysql.exe'
    if (-not (Test-Path -LiteralPath $server)) {throw 'mysqld.exe missing'}
    $data=Join-Path $verifyRoot 'data'
    if (-not (Test-Path -LiteralPath (Join-Path $data 'mysql'))) {
        $init=Start-Process -FilePath $server -ArgumentList @('--no-defaults','--initialize-insecure',"--basedir=`"$(Split-Path $MySqlBin -Parent)`"","--datadir=`"$data`"") -WindowStyle Hidden -PassThru -Wait -RedirectStandardError (Join-Path $verifyRoot 'init.log')
        if ($init.ExitCode -ne 0) {throw 'Isolated initialization failed'}
    }
    $serverProcess=Start-Process -FilePath $server -ArgumentList @('--no-defaults',"--basedir=`"$(Split-Path $MySqlBin -Parent)`"","--datadir=`"$data`"",'--bind-address=127.0.0.1',"--port=$port",'--mysqlx=0','--skip-log-bin','--event-scheduler=OFF','--console') -WindowStyle Hidden -PassThru -RedirectStandardError (Join-Path $verifyRoot 'server.log')
    $deadline=[DateTime]::UtcNow.AddSeconds(40)
    $ready=$false
    while ([DateTime]::UtcNow -lt $deadline -and -not $serverProcess.HasExited) {
        $socket=[Net.Sockets.TcpClient]::new()
        try {$socket.Connect('127.0.0.1',$port);$ready=$true;break} catch {Start-Sleep -Milliseconds 250} finally {$socket.Dispose()}
    }
    if (-not $ready) {throw 'Isolated server failed to start'}
    @'
CREATE DATABASE IF NOT EXISTS permission_fixture;
DROP DATABASE IF EXISTS collation_fixture;
DROP DATABASE IF EXISTS collation_mismatch;
CREATE TABLE IF NOT EXISTS permission_fixture.items(id INT PRIMARY KEY,name VARCHAR(30));
INSERT IGNORE INTO permission_fixture.items VALUES(1,'fixture');
CREATE OR REPLACE VIEW permission_fixture.hidden_view AS SELECT * FROM permission_fixture.items;
CREATE USER IF NOT EXISTS 'fixture_reader'@'127.0.0.1';
REVOKE ALL PRIVILEGES, GRANT OPTION FROM 'fixture_reader'@'127.0.0.1';
GRANT SELECT ON permission_fixture.* TO 'fixture_reader'@'127.0.0.1';
'@ | & $client --no-defaults --protocol=TCP --host=127.0.0.1 --port=$port --user=root --default-character-set=utf8mb4
    if ($LASTEXITCODE -ne 0) {throw 'Fixture creation failed'}
    Push-Location $repo
    try {
        & cargo run --locked -j 1 -p turbodbn-services --example transfer_permission_probe -- $port
        if ($LASTEXITCODE -ne 0) {throw 'Permission probe failed'}
        if ($UiScript) {
            @'
CREATE PROCEDURE IF NOT EXISTS permission_fixture.fixture_procedure() SELECT 1;
CREATE FUNCTION IF NOT EXISTS permission_fixture.fixture_function() RETURNS INT DETERMINISTIC RETURN 1;
CREATE EVENT IF NOT EXISTS permission_fixture.fixture_event ON SCHEDULE EVERY 1 DAY DO SELECT 1;
GRANT SHOW VIEW, TRIGGER, EVENT ON permission_fixture.* TO 'fixture_reader'@'127.0.0.1';
GRANT SHOW_ROUTINE ON *.* TO 'fixture_reader'@'127.0.0.1';
'@ | & $client --no-defaults --protocol=TCP --host=127.0.0.1 --port=$port --user=root
            if ($LASTEXITCODE -ne 0) {throw 'UI fixture creation failed'}
            & python $UiScript $port
            if ($LASTEXITCODE -ne 0) {throw 'Isolated UI probe failed'}
        }
    } finally {Pop-Location}
} finally {
    if ($serverProcess -and -not $serverProcess.HasExited) {Stop-Process -Id $serverProcess.Id}
    if ($serverProcess) {
        $marker='--datadir="?' + [regex]::Escape($data) + '"?(?:\s|$)'
        Get-CimInstance Win32_Process -Filter "Name='mysqld.exe'" |
            Where-Object { $_.CommandLine -match $marker } |
            ForEach-Object { Stop-Process -Id $_.ProcessId -ErrorAction SilentlyContinue }
    }
    $guard.Dispose()
}

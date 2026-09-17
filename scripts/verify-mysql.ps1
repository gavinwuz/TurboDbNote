param([Parameter(Mandatory)][string]$MySqlBin, [switch]$DisableTls, [switch]$ObjectsOnly)
$ErrorActionPreference='Stop'
$root=Split-Path $PSScriptRoot -Parent
$testDirectory=Join-Path $root ('target/mysql-smoke-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $testDirectory | Out-Null
$server=Join-Path $MySqlBin 'mysqld.exe'
if (-not (Test-Path -LiteralPath $server)) { throw 'mysqld.exe not found' }
$listener=[Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback,0)
$listener.Start();$port=$listener.LocalEndpoint.Port;$listener.Stop()
$data=Join-Path $testDirectory 'data'
$argsInit=@('--no-defaults','--initialize-insecure',"--basedir=`"$(Split-Path $MySqlBin -Parent)`"", "--datadir=`"$data`"")
$init=Start-Process -FilePath $server -ArgumentList $argsInit -WindowStyle Hidden -PassThru -Wait -RedirectStandardError (Join-Path $testDirectory 'init.log')
if ($init.ExitCode -ne 0) { throw "MySQL initialization failed; see $testDirectory" }
$serverArguments=@('--no-defaults',"--basedir=`"$(Split-Path $MySqlBin -Parent)`"", "--datadir=`"$data`"",'--bind-address=127.0.0.1',"--port=$port",'--mysqlx=0','--skip-log-bin','--console')
if ($DisableTls) { $serverArguments += "--ssl=0" }
$serverProcess=Start-Process -FilePath $server -ArgumentList $serverArguments -WindowStyle Hidden -PassThru -RedirectStandardError (Join-Path $testDirectory 'server.log')
try {
    $deadline=[DateTime]::UtcNow.AddSeconds(45)
    $ready=$false
    while ([DateTime]::UtcNow -lt $deadline -and -not $serverProcess.HasExited) {
        $socket=[Net.Sockets.TcpClient]::new()
        try { $socket.Connect('127.0.0.1',$port);$ready=$true;break } catch { Start-Sleep -Milliseconds 250 } finally { $socket.Dispose() }
    }
    if (-not $ready) {throw "MySQL did not start; see $testDirectory"}
    @'
CREATE DATABASE `测试库`;
CREATE TABLE `测试库`.fixture_items(id INT PRIMARY KEY, name VARCHAR(80));
CREATE INDEX fixture_name ON `测试库`.fixture_items(name);
CREATE USER 'probe_native'@'127.0.0.1' IDENTIFIED WITH mysql_native_password BY 'fixture-only';
CREATE USER 'probe_sha2'@'127.0.0.1' IDENTIFIED WITH caching_sha2_password BY 'fixture-only';
'@ | & (Join-Path $MySqlBin 'mysql.exe') --no-defaults --protocol=TCP --host=127.0.0.1 --port=$port --user=root --password= --default-character-set=utf8
    if ($LASTEXITCODE -ne 0) {throw 'Unable to create isolated authentication fixtures'}
    Push-Location $root
    try {
        $modes = if ($ObjectsOnly) { @() } elseif ($DisableTls) { @('tls-unsupported') } else { @('test','connect','bad-password','bad-db','tls-required','tls-verify','native-auth','sha2-auth','gbk','gb2312','custom-charset','single-db') }
        foreach($mode in $modes){
            & cargo run --locked -j 1 -p turbodbn-services --example mysql_probe -- $port $mode
            if ($LASTEXITCODE -ne 0) {throw "MySQL probe failed: $mode"}
        }
        if (-not $DisableTls) {
            & cargo run --locked -j 1 -p turbodbn-services --example database_probe -- mysql $port '测试库'
            if ($LASTEXITCODE -ne 0) {throw 'Modern object/create probe failed'}
        }
    } finally {Pop-Location}
} finally {
    # Only the temporary server created above is stopped; existing servers are untouched.
    if (-not $serverProcess.HasExited) { Stop-Process -Id $serverProcess.Id }
    # MySQL on Windows may spawn a monitor/server pair. Match our exact unique
    # datadir so the child is reaped too, without touching an existing instance.
    $marker='--datadir="?' + [regex]::Escape($data) + '"?(?:\s|$)'
    Get-CimInstance Win32_Process -Filter "Name='mysqld.exe'" |
        Where-Object { $_.CommandLine -match $marker } |
        ForEach-Object { Stop-Process -Id $_.ProcessId -ErrorAction SilentlyContinue }
}
"PASS: isolated MySQL connection checks; logs: $testDirectory"

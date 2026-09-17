param([Parameter(Mandatory)][string]$ServerDirectory,[Parameter(Mandatory)][string]$ClientDirectory)
$ErrorActionPreference='Stop'
$root=Split-Path $PSScriptRoot -Parent
$serverRoot=(Resolve-Path -LiteralPath $ServerDirectory).Path
$clientRoot=(Resolve-Path -LiteralPath $ClientDirectory).Path
$testRoot=Join-Path $root ('target/mysql-legacy-smoke-'+[guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $testRoot | Out-Null
Copy-Item -LiteralPath (Join-Path $serverRoot 'data') -Destination $testRoot -Recurse
$data=Join-Path $testRoot 'data'
$socket=[Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback,0);$socket.Start();$port=$socket.LocalEndpoint.Port;$socket.Stop()
$server=Join-Path $serverRoot 'bin/mysqld.exe'
if (-not (Test-Path -LiteralPath $server)) {$server=Join-Path $serverRoot 'bin/mysqld-nt.exe'}
$process=Start-Process -FilePath $server -ArgumentList @('--no-defaults',"--basedir=`"$serverRoot`"","--datadir=`"$data`"",'--bind-address=127.0.0.1',"--port=$port",'--skip-innodb','--console') -WindowStyle Hidden -PassThru -RedirectStandardError (Join-Path $testRoot 'server.log')
try{
    $deadline=[DateTime]::UtcNow.AddSeconds(25);$ready=$false
    while([DateTime]::UtcNow -lt $deadline -and -not $process.HasExited){
        $client=[Net.Sockets.TcpClient]::new();try{$client.Connect('127.0.0.1',$port);$ready=$true;break}catch{Start-Sleep -Milliseconds 200}finally{$client.Dispose()}
    }
    if(-not $ready){throw "Legacy server failed to start; see $testRoot"}
    $mysql=Join-Path $clientRoot 'bin/mysql.exe'
    $version = & $mysql --no-defaults --protocol=TCP --host=127.0.0.1 --port=$port --user=root --password= --batch --skip-column-names --execute='SELECT VERSION()'
    if($LASTEXITCODE -ne 0){throw 'Unable to detect fixture version'}
    $passwordFunction = if ($version -match '^4\.0\.') {'PASSWORD'} else {'OLD_PASSWORD'}
    @"
GRANT USAGE ON *.* TO 'legacy_probe'@'localhost' IDENTIFIED BY 'fixture-only';
SET PASSWORD FOR 'legacy_probe'@'localhost' = $passwordFunction('fixture-only');
"@ | & $mysql --no-defaults --protocol=TCP --host=127.0.0.1 --port=$port --user=root --password=
    if($LASTEXITCODE -ne 0){throw 'Unable to create legacy auth fixture'}
    $modes=@('test','connect','wrong-password','old-auth','old-auth-denied','tls-refuse')
    if($version -notmatch '^4\.0\.') {
        'CREATE DATABASE `测试`;' | & $mysql --no-defaults --protocol=TCP --host=127.0.0.1 --port=$port --user=root --password= --default-character-set=utf8
        if($LASTEXITCODE -ne 0){throw 'Unable to create Unicode database fixture'}
        $modes+=@('gbk','gb2312','single-db')
    }
    'CREATE TABLE test.fixture_items(id INT PRIMARY KEY, name VARCHAR(80)); CREATE INDEX fixture_name ON test.fixture_items(name);' | & $mysql --no-defaults --protocol=TCP --host=127.0.0.1 --port=$port --user=root --password=
    if($LASTEXITCODE -ne 0){throw 'Unable to create object fixture'}
    Push-Location $root
    try{foreach($mode in $modes){
        & cargo run --locked -j 1 -p turbodbn-services --example mysql_legacy_probe -- $port (Join-Path $clientRoot 'bin/libmysql.dll') $mode
        if($LASTEXITCODE -ne 0){throw "Legacy probe failed: $mode"}
    }
        & cargo run --locked -j 1 -p turbodbn-services --example database_probe -- mysql $port test (Join-Path $clientRoot 'bin/libmysql.dll')
        if($LASTEXITCODE -ne 0){throw 'Legacy object/create probe failed'}
    }finally{Pop-Location}
}finally{
    if(-not $process.HasExited){Stop-Process -Id $process.Id}
    $marker='--datadir="?'+[regex]::Escape($data)+'"?(?:\s|$)'
    Get-CimInstance Win32_Process | Where-Object {$_.Name -like 'mysqld*.exe' -and $_.CommandLine -match $marker} | ForEach-Object{Stop-Process -Id $_.ProcessId -ErrorAction SilentlyContinue}
}
"PASS: legacy probes; logs $testRoot"

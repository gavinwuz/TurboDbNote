function Stage-DatabaseDrivers {
    param([Parameter(Mandatory)][string]$Destination, [Parameter(Mandatory)][string]$HostPath)
    $nativeDirectory = [IO.Path]::GetFullPath($Destination)
    if ((Split-Path $nativeDirectory -Leaf) -ne 'native' -or (Split-Path (Split-Path $nativeDirectory -Parent) -Leaf) -ne 'drivers') { throw 'Expected an explicit drivers/native destination' }
    if (-not (Test-Path -LiteralPath $HostPath -PathType Leaf)) { throw 'Build the Rust driver host first' }
    New-Item -ItemType Directory -Path $nativeDirectory -Force | Out-Null
    Copy-Item -LiteralPath $HostPath -Destination (Join-Path $nativeDirectory 'turbodbnote-driver-host.exe') -Force
    Write-Output "Rust driver host staged: $nativeDirectory"
}

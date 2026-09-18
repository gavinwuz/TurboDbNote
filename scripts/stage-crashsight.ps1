function Copy-CrashSightRuntime {
    param([Parameter(Mandatory)][string]$SdkDirectory, [Parameter(Mandatory)][string]$Destination)
    $sdkRoot = (Resolve-Path -LiteralPath $SdkDirectory).Path
    $dll = Join-Path $sdkRoot 'CrashSight64.dll'
    if (-not (Test-Path -LiteralPath $dll -PathType Leaf)) { throw 'SDK directory must contain CrashSight64.dll' }
    $version = (Get-Item -LiteralPath $dll).VersionInfo.FileVersion
    if (-not $version -or $version -notmatch '^(\d+)\.(\d+)\.(\d+)') { throw 'Unable to verify CrashSight SDK version' }
    if ([version]$Matches[0] -lt [version]'2.2.1') { throw 'CrashSight 2.2.1+ required for the seven-argument exception ABI' }
    if (Test-Path -LiteralPath $Destination) { throw 'CrashSight staging destination already exists' }
    # Copy the complete, explicitly selected redistributable folder. Private
    # symbol-upload tools and keys must not be placed in this runtime-only folder.
    Copy-Item -LiteralPath $sdkRoot -Destination $Destination -Recurse
    Write-Output "CrashSight runtime staged (SDK $version)"
}

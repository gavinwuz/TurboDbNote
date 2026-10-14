param([Parameter(Mandatory)][string]$Version)
$ErrorActionPreference = 'Stop'
Push-Location (Split-Path $PSScriptRoot -Parent)
try {
    & cargo run --quiet --locked -j 1 -p turbodbnote-tools -- prepare-release $Version
    if ($LASTEXITCODE -ne 0) { throw 'Release preparation failed; inspect the preserved worktree or CI result before retrying.' }
} finally { Pop-Location }

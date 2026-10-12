param([Parameter(Mandatory)][string]$Version)
$ErrorActionPreference = 'Stop'
Push-Location (Split-Path $PSScriptRoot -Parent)
try {
    & python scripts/prepare-release.py $Version
    if ($LASTEXITCODE -ne 0) { throw 'Release preparation failed; inspect the preserved worktree or CI result before retrying.' }
} finally { Pop-Location }

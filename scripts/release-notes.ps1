function Get-VersionReleaseNotes {
    param(
        [Parameter(Mandatory)][string]$ChangelogPath,
        [Parameter(Mandatory)][string]$Version
    )
    $text = [IO.File]::ReadAllText($ChangelogPath)
    $headings = [regex]::Matches($text, '(?m)^## \[([^\]\r\n]+)\][^\r\n]*\r?$')
    $matches = @($headings | Where-Object { $_.Groups[1].Value -ceq $Version })
    if ($matches.Count -ne 1) { throw "Expected exactly one [$Version] section in $ChangelogPath" }
    $heading = $matches[0]
    $start = $heading.Index + $heading.Length
    $next = [regex]::Match($text.Substring($start), '(?m)^## ')
    $length = if ($next.Success) { $next.Index } else { $text.Length - $start }
    $body = $text.Substring($start, $length).Trim()
    if (-not $body) { throw "Empty [$Version] section in $ChangelogPath" }
    return "# TurboDBNote v$Version`n`n$body`n"
}

function Write-ReleaseChecksums {
    param([Parameter(Mandatory)][string[]]$Paths, [Parameter(Mandatory)][string]$OutputFile)
    $lines = @($Paths | ForEach-Object {
        $hash = (Get-FileHash -LiteralPath $_ -Algorithm SHA256).Hash.ToLowerInvariant()
        "$hash  $([IO.Path]::GetFileName($_))"
    })
    [IO.File]::WriteAllText($OutputFile, ($lines -join "`n") + "`n", [Text.Encoding]::ASCII)
}

function Write-WindowsReleaseArchives {
    param([Parameter(Mandatory)][string]$Stage,
        [Parameter(Mandatory)][string]$PortableZip,
        [Parameter(Mandatory)][string]$SourcesZip)
    $stagePath = (Resolve-Path -LiteralPath $Stage).Path
    $rootName = Split-Path $stagePath -Leaf
    $portable = [IO.Compression.ZipFile]::Open($PortableZip, [IO.Compression.ZipArchiveMode]::Create)
    try {
        $sources = [IO.Compression.ZipFile]::Open($SourcesZip, [IO.Compression.ZipArchiveMode]::Create)
        try {
            foreach ($file in Get-ChildItem -LiteralPath $stagePath -Recurse -File) {
                $relative = [IO.Path]::GetRelativePath($stagePath, $file.FullName).Replace('\', '/')
                $sourceOnly = $relative.StartsWith('sources/', [StringComparison]::OrdinalIgnoreCase) -or
                    $relative -ieq 'drivers/mysql-5.1.73-winx64/mysql-5.1.73-source.tar.gz'
                $archive = if ($sourceOnly) { $sources } else { $portable }
                [IO.Compression.ZipFileExtensions]::CreateEntryFromFile($archive, $file.FullName,
                    "$rootName/$relative", [IO.Compression.CompressionLevel]::Optimal) | Out-Null
            }
        } finally { $sources.Dispose() }
    } finally { $portable.Dispose() }
}

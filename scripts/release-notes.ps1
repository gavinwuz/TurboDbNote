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
    return "# TurboDbNote v$Version`n`n$body`n"
}

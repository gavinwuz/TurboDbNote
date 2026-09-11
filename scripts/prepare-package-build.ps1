function New-PackageBuildWorkspace {
    param([string]$Root, [string]$FxcPath)
    # Isolate the only upstream change; never edit the registry or the repository lockfile.
    $json = & cargo metadata --locked --format-version 1 --filter-platform x86_64-pc-windows-msvc
    if ($LASTEXITCODE -ne 0) { throw 'Unable to inspect locked GPUI source' }
    $metadata = $json | ConvertFrom-Json
    $gpui = @($metadata.packages | Where-Object { $_.name -eq 'gpui' -and $_.version -eq '0.2.2' })
    if ($gpui.Count -ne 1) { throw 'Build adapter requires exactly gpui 0.2.2; review it before upgrading' }
    $workspace = Join-Path $Root ('target/package-builds/' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $workspace -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $Root 'Cargo.toml'), (Join-Path $Root 'Cargo.lock') -Destination $workspace
    Copy-Item -LiteralPath (Join-Path $Root 'crates') -Destination $workspace -Recurse
    $source = Join-Path $workspace 'upstream/gpui'
    New-Item -ItemType Directory -Path (Split-Path $source) -Force | Out-Null
    Copy-Item -LiteralPath (Split-Path $gpui[0].manifest_path) -Destination $source -Recurse
    $buildFile = Join-Path $source 'build.rs'
    $build = [IO.File]::ReadAllText($buildFile)
    $start = $build.IndexOf('    fn find_fxc_compiler() -> String {')
    $end = $build.IndexOf('    fn compile_shader_for_module(', $start)
    if ($start -lt 0 -or $end -le $start) { throw 'GPUI build adapter no longer matches upstream' }
    $replacement = @'
    fn find_fxc_compiler() -> String {
        println!("cargo:rerun-if-changed=package-fxc-path.txt");
        let path = include_str!("package-fxc-path.txt").trim();
        assert!(Path::new(path).is_file(), "Configured fxc.exe does not exist");
        path.to_owned()
    }

'@
    [IO.File]::WriteAllText($buildFile, $build.Substring(0, $start) + $replacement + $build.Substring($end))
    [IO.File]::WriteAllText((Join-Path $source 'package-fxc-path.txt'), $FxcPath)
    Add-Content -LiteralPath (Join-Path $workspace 'Cargo.toml') -Value "`n[patch.crates-io]`ngpui = { path = 'upstream/gpui' }"
    Push-Location $workspace
    try {
        # Re-resolve only the source override using the copied lock; all versions must remain pinned.
        $patchedJson = & cargo metadata --offline --format-version 1 --filter-platform x86_64-pc-windows-msvc
        if ($LASTEXITCODE -ne 0) { throw 'Unable to resolve isolated build lockfile' }
        $patched = $patchedJson | ConvertFrom-Json
        $before = @($metadata.packages | ForEach-Object { "$($_.name)@$($_.version)" } | Sort-Object)
        $after = @($patched.packages | ForEach-Object { "$($_.name)@$($_.version)" } | Sort-Object)
        if (Compare-Object $before $after) { throw 'Build adapter unexpectedly changed dependency versions' }
    } finally { Pop-Location }
    return $workspace
}

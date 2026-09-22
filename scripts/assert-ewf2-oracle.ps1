# Shared by the VHDX harness and ordinary-file acceptance runs; no device access.
function Assert-Ewf2Oracle {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string[]]$Segments,
        [Parameter(Mandatory)][string]$StagingDirectory,
        [Parameter(Mandatory)][string]$EwfExport,
        [Parameter(Mandatory)][string]$EwfVerify,
        [string]$PhysicalSha256,
        [System.Collections.IDictionary]$LogicalHashes
    )
    $ErrorActionPreference = 'Stop'
    if ([bool]$PhysicalSha256 -eq [bool]$LogicalHashes) { throw 'Supply exactly one expected physical digest or logical manifest' }
    if (Test-Path -LiteralPath $StagingDirectory) { throw 'Oracle staging directory must be new' }
    foreach ($tool in @($EwfExport, $EwfVerify)) {
        $version = & $tool -V 2>&1 | Out-String
        if ($LASTEXITCODE -ne 0 -or $version -notmatch '20260924') { throw 'Requires pinned libewf 20260924' }
    }
    $null = New-Item -ItemType Directory -Path $StagingDirectory
    $staging = (Resolve-Path -LiteralPath $StagingDirectory).ProviderPath
    $names = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    $first = $null
    foreach ($segment in $Segments) {
        $item = Get-Item -LiteralPath $segment
        if ($item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'Expected ordinary segment files' }
        if (-not $names.Add($item.Name)) { throw 'Duplicate oracle segment name' }
        $copy = Join-Path $staging $item.Name
        Copy-Item -LiteralPath $item.FullName -Destination $copy
        if ((Get-FileHash -LiteralPath $item.FullName).Hash -ne (Get-FileHash -LiteralPath $copy).Hash) { throw 'Oracle staging changed segment bytes' }
        if ($item.Extension -cin @('.Ex01', '.Lx01')) {
            if ($first) { throw 'Multiple first segments' }
            $first = $copy
        }
    }
    if (-not $first) { throw 'Missing first EWF2 segment' }
    $logical = [bool]$LogicalHashes
    if (($logical -and [IO.Path]::GetExtension($first) -cne '.Lx01') -or
        (-not $logical -and [IO.Path]::GetExtension($first) -cne '.Ex01')) { throw 'Oracle profile does not match expected content' }
    $target = Join-Path $staging 'exported'
    $format = if ($logical) { 'files' } else { 'raw' }
    $log = & $EwfExport -q -u -f $format -t $target $first 2>&1
    if ($LASTEXITCODE -ne 0) { throw "ewfexport failed: $($log -join ' ')" }
    if ($logical) {
        $files = @(Get-ChildItem -LiteralPath $target -Recurse -File)
        $actual = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
        foreach ($file in $files) {
            if ($file.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Unexpected exported reparse point' }
            $relative = [IO.Path]::GetRelativePath($target, $file.FullName).Replace('\', '/')
            if (-not $actual.Add($relative)) { throw 'Duplicate exported path' }
        }
        if (-not $actual.SetEquals([string[]]@($LogicalHashes.Keys))) { throw 'Exported logical paths differ from source manifest' }
        foreach ($file in $files) {
            $relative = [IO.Path]::GetRelativePath($target, $file.FullName).Replace('\', '/')
            if ((Get-FileHash -LiteralPath $file.FullName).Hash.ToLowerInvariant() -cne $LogicalHashes[$relative]) { throw "Exported logical SHA256 mismatch: $relative" }
        }
    } elseif ((Get-FileHash -LiteralPath ($target + '.raw')).Hash.ToLowerInvariant() -cne $PhysicalSha256) {
        throw 'Exported physical SHA256 differs from source'
    }
    $log = & $EwfVerify -q -f $format $first 2>&1
    if ($LASTEXITCODE -ne 0) { throw "ewfverify failed: $($log -join ' ')" }
    return @{ status = 'passed'; libewf_version = '20260924'; profile = $format; segments = $Segments.Count }
}

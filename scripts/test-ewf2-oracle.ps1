# Ordinary-file acceptance for the shared Windows oracle helper; no elevation.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Binary,
    [Parameter(Mandatory)][string]$EwfExport,
    [Parameter(Mandatory)][string]$EwfVerify,
    [string]$Directory = './target'
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'assert-ewf2-oracle.ps1')
$Binary = (Resolve-Path -LiteralPath $Binary).Path
$EwfExport = (Resolve-Path -LiteralPath $EwfExport).Path
$EwfVerify = (Resolve-Path -LiteralPath $EwfVerify).Path
$work = Join-Path (Resolve-Path -LiteralPath $Directory).Path ('ewf2-oracle-' + [Guid]::NewGuid().ToString('N'))
$source = Join-Path $work 'source'
$null = New-Item -ItemType Directory -Path (Join-Path $source 'folder')
$bytes = [byte[]]::new(262144)
for ($index = 0; $index -lt $bytes.Length; $index++) { $bytes[$index] = ($index * 71 + [Math]::Floor($index / 257)) % 251 }
$raw = Join-Path $source 'folder/data.bin'
[IO.File]::WriteAllBytes($raw, $bytes)
[IO.File]::WriteAllBytes((Join-Path $source 'empty.bin'), [byte[]]::new(0))
$expected = (Get-FileHash -LiteralPath $raw).Hash.ToLowerInvariant()
$hashes = @{ 'folder/data.bin' = $expected; 'empty.bin' = 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855' }
$results = @()
foreach ($mode in @('Ex01', 'Lx01')) {
    $command = if ($mode -eq 'Ex01') { 'acquire-sequential' } else { 'collect' }
    $inputPath = if ($mode -eq 'Ex01') { $raw } else { $source }
    $report = & $Binary --json --quiet ewf $command $inputPath (Join-Path $work "case.$mode") --chunks-per-segment 2 | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw "CLI failed: $($report | ConvertTo-Json -Compress -Depth 8)" }
    $parameters = @{ Segments = $report.segments; StagingDirectory = (Join-Path $work "oracle-$mode"); EwfExport = $EwfExport; EwfVerify = $EwfVerify }
    if ($mode -eq 'Ex01') { $parameters.PhysicalSha256 = $expected }
    else { $parameters.LogicalHashes = $hashes }
    $results += Assert-Ewf2Oracle @parameters
}
foreach ($failure in @('hash', 'path')) {
    $bad = $hashes.Clone()
    $message = if ($failure -eq 'hash') {
        $bad['folder/data.bin'] = '0' * 64
        'Exported logical SHA256 mismatch'
    } else {
        $bad.Remove('empty.bin')
        $bad['unexpected.bin'] = $hashes['empty.bin']
        'Exported logical paths differ'
    }
    $parameters.LogicalHashes = $bad
    $parameters.StagingDirectory = Join-Path $work "wrong-$failure"
    try {
        $null = Assert-Ewf2Oracle @parameters
        throw "Accepted incorrect source $failure"
    } catch { if ($_.Exception.Message -notmatch $message) { throw } }
}
@{ status = 'passed'; results = $results; negative_hash_and_path = 'passed'; retained_artifacts = $work } | ConvertTo-Json -Depth 5

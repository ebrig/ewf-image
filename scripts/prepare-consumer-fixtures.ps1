param(
    [Parameter(Mandatory)][string]$SourceDirectory,
    [Parameter(Mandatory)][string]$OutputDirectory,
    [string]$TargetDirectory
)
$ErrorActionPreference = 'Stop'
$repository = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
if (-not $TargetDirectory) { $TargetDirectory = Join-Path $repository 'target' }
$TargetDirectory = [IO.Path]::GetFullPath($TargetDirectory, $PWD.ProviderPath)
$sourceRoot = (Resolve-Path -LiteralPath $SourceDirectory).Path
$outputRoot = [IO.Path]::GetFullPath($OutputDirectory)
if (Test-Path -LiteralPath $outputRoot) { throw 'Output directory must be new' }
$raw = Join-Path $sourceRoot 'synthetic-disk.raw'
$logical = Join-Path $sourceRoot 'logical-source'
$expected = '6bbac1977bf3cdaff4c1e36ddea49e4c67a084a5c05f380812bb1ca949f84e14'
if ((Get-Item -LiteralPath $raw).Length -ne 4194304 -or (Get-FileHash -LiteralPath $raw -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expected) {
    throw 'Expected the recorded 4 MiB synthetic source; source bytes changed'
}
$sources = @(
    @{ path = 'note.txt'; bytes = 51; sha256 = '602665c99c4bfd08209364ce85b6f9e74d6e7495663c2e40b4ac0be12996865a' },
    @{ path = 'folder/repeat.bin'; bytes = 110000; sha256 = '236ac23c57821dd959102618a3dd5d3d6025406df132639bc6af3b0176893bb0' }
)
foreach ($item in $sources) {
    $path = Join-Path $logical $item.path
    if ((Get-Item -LiteralPath $path).Length -ne $item.bytes -or (Get-FileHash -LiteralPath $path).Hash.ToLowerInvariant() -ne $item.sha256) { throw "Changed logical source: $($item.path)" }
}
if (@(Get-ChildItem -LiteralPath $logical -Recurse -File).Count -ne 2) { throw 'Unexpected logical source files' }
if ($outputRoot.StartsWith($logical + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'Output must be outside logical input' }
Push-Location $repository
try {
    & git diff --quiet HEAD -- src crates Cargo.toml Cargo.lock
    if ($LASTEXITCODE -ne 0) { throw 'Commit runtime source changes before preparing attributed fixtures' }
    $revision = (& git rev-parse HEAD).Trim()
    & cargo build --release --locked --features cli --bin ewf-image --target-dir $TargetDirectory
    if ($LASTEXITCODE -ne 0) { throw 'EWF build failed' }
    & cargo build --release --locked -p aff4-image --bin aff4-image --example acquire --target-dir $TargetDirectory
    if ($LASTEXITCODE -ne 0) { throw 'AFF4 build failed' }
    $ewf = Join-Path $TargetDirectory 'release/ewf-image.exe'
    $aff4 = Join-Path $TargetDirectory 'release/aff4-image.exe'
    $acquire = Join-Path $TargetDirectory 'release/examples/acquire.exe'
    New-Item -ItemType Directory -Path $outputRoot | Out-Null
    function Invoke-Recorded([string]$program, [string]$name, [string[]]$arguments) {
        $text = (& $program @arguments 2> (Join-Path $outputRoot "$name.stderr.txt")) | Out-String
        $code = $LASTEXITCODE
        [IO.File]::WriteAllText((Join-Path $outputRoot "$name.stdout.txt"), $text)
        if ($code -ne 0) { throw "$name failed with exit $code; retained output/logs at $outputRoot" }
        return $text
    }
    foreach ($mode in @(@{command='acquire'; extension='E01'}, @{command='acquire-sequential'; extension='Ex01'})) {
        $output = Join-Path $outputRoot "physical.$($mode.extension)"
        $report = (Invoke-Recorded $ewf $mode.extension @('--quiet', $mode.command, $raw, $output, '--chunks-per-segment', '16', '--compression', 'zlib')) | ConvertFrom-Json
        if ($report.verification.sha256 -ne $expected) { throw 'Physical SHA256 mismatch' }
    }
    $null = Invoke-Recorded $ewf 'Lx01' @('--quiet','collect',$logical,(Join-Path $outputRoot 'logical.Lx01'),'--chunks-per-segment','1')
    $null = Invoke-Recorded $acquire 'aff4-acquire' @($raw,(Join-Path $outputRoot 'physical.aff4'))
    $verified = (Invoke-Recorded $aff4 'aff4-verify' @('verify',(Join-Path $outputRoot 'physical.aff4'))) | ConvertFrom-Json
    if ($expected -notin @($verified.resources | ForEach-Object { $_.verification.sha256 })) { throw 'AFF4 decoded SHA256 mismatch' }
    $null = Invoke-Recorded $aff4 'aff4-collect' @('collect',$logical,(Join-Path $outputRoot 'logical.aff4'))
    $images = @(Get-ChildItem -LiteralPath $outputRoot -File | Where-Object { $_.Extension -match '^\.(E[0-9]+|Ex[0-9]+|Lx[0-9]+|aff4)$' } | ForEach-Object {
        @{ path = $_.Name; bytes = $_.Length; sha256 = (Get-FileHash -LiteralPath $_.FullName).Hash.ToLowerInvariant() }
    })
    $manifest = @{
        source_revision = $revision
        preparation_script_sha256 = (Get-FileHash -LiteralPath $PSCommandPath).Hash.ToLowerInvariant()
        binary_sha256 = @{ ewf = (Get-FileHash $ewf).Hash.ToLowerInvariant(); aff4 = (Get-FileHash $aff4).Hash.ToLowerInvariant(); aff4_acquire = (Get-FileHash $acquire).Hash.ToLowerInvariant() }
        physical_source = @{ bytes = 4194304; sha256 = $expected }
        logical_sources = $sources
        images = $images
        consumer_status = 'not tested'
        notes = 'Fresh synthetic outputs; physical source has no filesystem. Import every set with all segments present, export media/files, and compare to source hashes. Local verification is not EnCase consumer validation.'
    }
    $manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $outputRoot 'consumer-manifest.json') -Encoding utf8
    Write-Output "Prepared $outputRoot from $revision; EnCase consumer checks remain pending."
} finally { Pop-Location }

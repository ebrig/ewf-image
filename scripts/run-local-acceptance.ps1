#requires -Version 7.0
[CmdletBinding()]
param(
    [string]$OutputDirectory,
    [string]$FixtureSourceDirectory,
    [string]$EwfExport,
    [string]$EwfVerify,
    [switch]$RunWindowsDevices,
    [string]$WslDistribution,
    [string]$WslLibewfPrefix,
    [string]$WslAff4Oracle,
    [string]$WslTargetDirectory = '/tmp/ewf-local-acceptance-target',
    [switch]$AllowDirty,
    [switch]$PlanOnly
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'acceptance-common.ps1')
$repository = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$pwsh = (Get-Process -Id $PID).Path
if ([bool]$EwfExport -ne [bool]$EwfVerify) { throw 'Supply both EWF oracle tools or neither' }
if (($WslLibewfPrefix -or $WslAff4Oracle) -and -not $WslDistribution) { throw 'WSL oracle paths require WslDistribution' }
if ($WslDistribution -and (-not $IsWindows -or -not $WslTargetDirectory.StartsWith('/'))) { throw 'WSL requires Windows and an absolute Linux target directory' }
foreach ($variable in @('FixtureSourceDirectory', 'EwfExport', 'EwfVerify')) {
    $value = Get-Variable -Name $variable -ValueOnly
    if ($value) { Set-Variable -Name $variable -Value (Resolve-Path -LiteralPath $value).Path }
}
function Get-SourceState {
    $head = (& git -C $repository rev-parse HEAD | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) { throw 'Cannot identify repository revision' }
    $tree = (& git -C $repository rev-parse 'HEAD^{tree}' | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) { throw 'Cannot identify repository tree' }
    $status = (& git -C $repository status --porcelain=v1 --untracked-files=normal | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) { throw 'Cannot read repository state' }
    return @{ head = $head; tree = $tree; status = $status; clean = -not [bool]$status }
}
$sourceStart = Get-SourceState
if (-not $sourceStart.clean -and -not $AllowDirty) { throw 'Commit or account for local changes first; AllowDirty is for runner development only' }
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $repository ('target/local-acceptance-' + [Guid]::NewGuid().ToString('N')) }
$output = [IO.Path]::GetFullPath($OutputDirectory, $PWD.ProviderPath)
if (Test-Path -LiteralPath $output) { throw 'OutputDirectory must be new; previous acceptance evidence is never overwritten' }
$null = New-Item -ItemType Directory -Path $output
$build = Join-Path $output 'build'
$buildEnvironment = @{ CARGO_TARGET_DIR = $build }
$manifest = [ordered]@{
    schema_version = 1; status = 'running'; release_ready = $false
    started_utc = [DateTime]::UtcNow.ToString('o'); finished_utc = $null
    repository = $repository; source_start = $sourceStart; source_end = $null
    allow_dirty = [bool]$AllowDirty; plan_only = [bool]$PlanOnly
    host = @{ os = [Runtime.InteropServices.RuntimeInformation]::OSDescription; architecture = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString(); powershell = $PSVersionTable.PSVersion.ToString() }
    tools = [ordered]@{}; checks = [Collections.Generic.List[object]]::new(); artifacts = @()
    excluded_artifact_directories = @('build')
    scope = 'Local checks only; no tag, push, publication, or release approval. Skipped and blocked gates remain unvalidated.'
}
Save-AcceptanceManifest $manifest $output
function Check([string]$Name, [string]$Program, [string[]]$Arguments, [hashtable]$Environment = @{}, [int[]]$Blocked = @()) {
    if ($PlanOnly -and -not $Name.StartsWith('version-')) {
        Add-AcceptanceSkip $manifest $output $Name 'PlanOnly: command not executed'
        return @{ status = 'skipped' }
    }
    return Invoke-AcceptanceCheck -Manifest $manifest -Directory $output -Name $Name -Program $Program -Arguments $Arguments -Environment $Environment -WorkingDirectory $repository -BlockedExitCodes $Blocked
}
function Version([string]$Name, [string]$Program, [string[]]$Arguments) {
    $result = Check "version-$Name" $Program $Arguments $buildEnvironment
    Set-AcceptanceToolVersion $manifest $output $Name $Program $result
    return $result
}
function Wsl-Check([string]$Name, [string[]]$Arguments) {
    # Fixed forwarding shell: caller-controlled paths stay individual argv items.
    return Check $Name 'wsl.exe' (@('-d', $WslDistribution, '--cd', $script:wslRepository, '--exec', 'bash', '-lc', 'exec "$@"', 'local-acceptance', 'env', "CARGO_TARGET_DIR=$WslTargetDirectory") + $Arguments)
}
try {
    $null = Version 'git' 'git' @('--version')
    $null = Version 'cargo' 'cargo' @('--version')
    $null = Version 'rustc' 'rustc' @('-Vv')
    $metadataCheck = Version 'cargo-metadata' 'cargo' @('metadata', '--no-deps', '--format-version', '1', '--locked')
    if ($metadataCheck.status -ne 'passed') { throw 'Cargo metadata is required to identify package artifacts' }
    $metadata = Get-Content (Join-Path $output $metadataCheck.stdout.path) -Raw | ConvertFrom-Json
    $package = $metadata.packages | Where-Object name -EQ 'ewf-image'
    $null = Check 'runner-selftest' $pwsh @('-NoProfile', '-File', (Join-Path $PSScriptRoot 'test-acceptance-runner.ps1'))
    $null = Check 'format' 'cargo' @('fmt', '--all', '--check') $buildEnvironment
    $null = Check 'workspace-tests' 'cargo' @('test', '--workspace', '--all-features', '--locked') $buildEnvironment
    $null = Check 'no-default-tests' 'cargo' @('test', '--no-default-features', '--locked') $buildEnvironment
    $null = Check 'clippy' 'cargo' @('clippy', '--workspace', '--all-targets', '--all-features', '--locked', '--', '-D', 'warnings') $buildEnvironment
    $docEnvironment = $buildEnvironment.Clone(); $docEnvironment.RUSTDOCFLAGS = '-D warnings'
    $null = Check 'rustdoc' 'cargo' @('doc', '--workspace', '--all-features', '--no-deps', '--locked') $docEnvironment
    $packageArgs = @('package', '--all-features', '--locked')
    if ($AllowDirty) { $packageArgs += '--allow-dirty' }
    $packageCheck = Check 'package' 'cargo' $packageArgs $buildEnvironment
    $ewfBuild = Check 'ewf-release' 'cargo' @('build', '--release', '--locked', '--features', 'cli', '--bin', 'ewf-image') $buildEnvironment
    $aff4Build = Check 'aff4-release' 'cargo' @('build', '--release', '--locked', '-p', 'aff4-image', '--bin', 'aff4-image', '--example', 'acquire') $buildEnvironment
    $suffix = if ($IsWindows) { '.exe' } else { '' }
    $binary = Join-Path $build "release/ewf-image$suffix"
    if ($EwfExport -and $ewfBuild.status -eq 'passed') {
        foreach ($tool in @(@('ewfexport', $EwfExport), @('ewfverify', $EwfVerify))) {
            if ([IO.Path]::GetExtension($tool[1]) -eq '.ps1') { $null = Version $tool[0] $pwsh @('-NoProfile', '-File', $tool[1], '-V') }
            else { $null = Version $tool[0] $tool[1] @('-V') }
        }
        $null = Check 'ewf2-oracle' $pwsh @('-NoProfile', '-File', (Join-Path $PSScriptRoot 'test-ewf2-oracle.ps1'), '-Binary', $binary, '-EwfExport', $EwfExport, '-EwfVerify', $EwfVerify, '-Directory', $output)
    } else { Add-AcceptanceSkip $manifest $output 'ewf2-oracle' 'Oracle paths not supplied or release build unavailable' }
    if ($RunWindowsDevices -and $IsWindows -and $ewfBuild.status -eq 'passed') {
        $deviceArgs = @('-NoProfile', '-File', (Join-Path $PSScriptRoot 'test-device-acquisition.ps1'), '-Binary', $binary, '-SequentialOnly', '-Directory', $output)
        if ($EwfExport) { $deviceArgs += @('-EwfExport', $EwfExport, '-EwfVerify', $EwfVerify) }
        $preflight = Check 'windows-device-prerequisites' $pwsh ($deviceArgs + '-CheckPrerequisites') @{} @(77)
        if ($preflight.status -eq 'passed') { $null = Check 'windows-devices' $pwsh $deviceArgs }
        else { Add-AcceptanceSkip $manifest $output 'windows-devices' 'Prerequisites did not pass; see preflight status and logs' }
    } else { Add-AcceptanceSkip $manifest $output 'windows-devices' 'Requires explicit RunWindowsDevices, Windows, and a successful build' }
    if ($WslDistribution) {
        $mapped = Check 'version-wsl-path' 'wsl.exe' @('-d', $WslDistribution, '--exec', 'wslpath', '-a', '-u', $repository)
        if ($mapped.status -ne 'passed') { throw 'WSL repository mapping failed' }
        $script:wslRepository = (Get-Content (Join-Path $output $mapped.stdout.path) -Raw).Trim()
        $null = Wsl-Check 'version-wsl-rustc' @('rustc', '-Vv')
        $null = Wsl-Check 'version-wsl-cargo' @('cargo', '--version')
        $null = Wsl-Check 'linux-aff4-tests' @('cargo', 'test', '-p', 'aff4-image', '--locked')
        if ($WslLibewfPrefix) {
            foreach ($tool in @('ewfexport', 'ewfverify', 'ewfinfo', 'ewfacquirestream')) {
                $null = Wsl-Check "version-linux-$tool" @("$WslLibewfPrefix/bin/$tool", '-V')
            }
            $null = Wsl-Check 'linux-ewf-oracles' @('bash', 'scripts/test-interoperability.sh', $WslLibewfPrefix)
        } else { Add-AcceptanceSkip $manifest $output 'linux-ewf-oracles' 'WslLibewfPrefix not supplied' }
        if ($WslAff4Oracle) {
            $null = Wsl-Check 'version-aff4-oracle' @($WslAff4Oracle, '--version')
            $null = Wsl-Check 'aff4-oracle-binary-hash' @('sha256sum', $WslAff4Oracle)
            $null = Wsl-Check 'linux-aff4-oracles' @('env', "AFF4_ORACLE=$WslAff4Oracle", 'cargo', 'test', '-p', 'aff4-image', '--test', 'writer', 'independent_', '--locked', '--', '--ignored')
        } else { Add-AcceptanceSkip $manifest $output 'linux-aff4-oracles' 'WslAff4Oracle not supplied' }
    } else {
        foreach ($name in @('linux-aff4-tests', 'linux-ewf-oracles', 'linux-aff4-oracles')) { Add-AcceptanceSkip $manifest $output $name 'WSL not configured' }
    }
    if ($FixtureSourceDirectory -and $IsWindows -and $ewfBuild.status -eq 'passed' -and $aff4Build.status -eq 'passed') {
        $null = Check 'consumer-fixtures' $pwsh @('-NoProfile', '-File', (Join-Path $PSScriptRoot 'prepare-consumer-fixtures.ps1'), '-SourceDirectory', $FixtureSourceDirectory, '-OutputDirectory', (Join-Path $output 'consumer-fixtures'), '-TargetDirectory', $build) $buildEnvironment
    } else { Add-AcceptanceSkip $manifest $output 'consumer-fixtures' 'Requires Windows, a source corpus, and successful release builds' }
    foreach ($name in @('encase-manual', 'macos', 'msrv', 'fuzz-campaign', 'large-scale-benchmarks', 'linux-privileged-storage')) {
        Add-AcceptanceSkip $manifest $output $name 'Outside this runner execution; prior runs do not certify this revision'
    }
    $artifactDirectory = Join-Path $output 'artifacts'
    $null = New-Item -ItemType Directory -Path $artifactDirectory
    if ($packageCheck.status -eq 'passed') { Copy-Item -LiteralPath (Join-Path $build "package/ewf-image-$($package.version).crate") -Destination $artifactDirectory }
    if ($ewfBuild.status -eq 'passed') { Copy-Item -LiteralPath $binary -Destination $artifactDirectory }
    if ($aff4Build.status -eq 'passed') {
        Copy-Item -LiteralPath (Join-Path $build "release/aff4-image$suffix") -Destination $artifactDirectory
        Copy-Item -LiteralPath (Join-Path $build "release/examples/acquire$suffix") -Destination (Join-Path $artifactDirectory "aff4-acquire$suffix")
    }
} catch {
    $manifest.runner_error = $_.Exception.Message
    $manifest.runner_error_location = $_.InvocationInfo.PositionMessage
}
finally {
    try {
        $manifest.source_end = Get-SourceState
        if ($manifest.source_start.head -ne $manifest.source_end.head -or $manifest.source_start.status -cne $manifest.source_end.status) { $manifest.runner_error = 'Repository state changed during acceptance; results do not certify one revision' }
        # Deliberately omit compiler intermediates. Hash every retained log,
        # fixture, package and executable; the manifest gets a separate checksum.
        $files = @(Get-ChildItem -LiteralPath $output -Force | Where-Object { $_.Name -notin @('build', 'acceptance.json', 'acceptance.sha256', 'acceptance.json.pending') } | ForEach-Object {
            if ($_.PSIsContainer) { Get-ChildItem -LiteralPath $_.FullName -Recurse -File -Force } else { $_ }
        })
        $manifest.artifacts = @($files | Sort-Object FullName | ForEach-Object { @{ path = [IO.Path]::GetRelativePath($output, $_.FullName).Replace('\', '/'); bytes = $_.Length; sha256 = (Get-FileHash -LiteralPath $_.FullName).Hash.ToLowerInvariant() } })
        $manifest.runner_sha256 = (Get-FileHash -LiteralPath $PSCommandPath).Hash.ToLowerInvariant()
        $manifest.helper_sha256 = (Get-FileHash -LiteralPath (Join-Path $PSScriptRoot 'acceptance-common.ps1')).Hash.ToLowerInvariant()
    } catch { $manifest.runner_error = $_.Exception.Message }
    Complete-AcceptanceManifest $manifest $output
}
@{ status = $manifest.status; release_ready = $false; manifest = (Join-Path $output 'acceptance.json'); revision = $manifest.source_start.head } | ConvertTo-Json
if ($manifest.status -eq 'failed') { exit 1 }

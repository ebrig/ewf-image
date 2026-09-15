<#
.SYNOPSIS
Windows acceptance tests confined to newly created disposable VHDX images.
.DESCRIPTION
Requires elevation and the Hyper-V PowerShell module. Existing disks are never
selected for formatting. Optional pinned libewf tools independently verify/export
output; their absence is reported explicitly. Physical hot-unplug is not tested.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Binary,
    [string]$Directory = [IO.Path]::GetTempPath(),
    [string]$EwfExport,
    [string]$EwfVerify,
    [switch]$CheckPrerequisites
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
$Binary = (Resolve-Path -LiteralPath $Binary).ProviderPath
$principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
$missing = @('New-VHD', 'Mount-VHD', 'Dismount-VHD', 'Get-VHD', 'Get-Disk', 'Get-Volume',
    'Get-Partition', 'Initialize-Disk', 'New-Partition', 'Format-Volume',
    'Add-PartitionAccessPath', 'Remove-PartitionAccessPath') | Where-Object { -not (Get-Command $_ -ErrorAction SilentlyContinue) }
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator) -or $missing.Count -gt 0) {
    @{ schema_version = 1; status = 'blocked'; reason = 'Requires elevation and Hyper-V/Storage cmdlets'; missing_commands = @($missing) } | ConvertTo-Json
    exit 77
}
if ([bool]$EwfExport -ne [bool]$EwfVerify) { throw 'Supply both libewf tools or neither' }
if ($EwfExport) {
    $EwfExport = (Resolve-Path -LiteralPath $EwfExport).ProviderPath
    $EwfVerify = (Resolve-Path -LiteralPath $EwfVerify).ProviderPath
    foreach ($tool in @($EwfExport, $EwfVerify)) {
        $version = & $tool -V 2>&1 | Out-String
        if ($LASTEXITCODE -ne 0 -or $version -notmatch '20260924') { throw 'Requires libewf 20260924' }
    }
}
if ($CheckPrerequisites) {
    @{ schema_version = 1; status = 'ready'; oracle_available = [bool]$EwfExport } | ConvertTo-Json
    exit 0
}

$parent = (Resolve-Path -LiteralPath $Directory).ProviderPath
$workRoot = [IO.Path]::GetFullPath((Join-Path $parent ('ewf-device-accept-' + [Guid]::NewGuid().ToString('N'))))
$rootBoundary = $workRoot.TrimEnd('\') + '\'
$null = New-Item -ItemType Directory -Path $workRoot
$images = [Collections.Generic.List[string]]::new()
$accessPaths = [Collections.Generic.List[object]]::new()
$checks = [Collections.Generic.List[string]]::new()
$size = 64MB

function Assert-OwnedPath([string]$Path) {
    $absolute = [IO.Path]::GetFullPath($Path)
    if (-not $absolute.StartsWith($rootBoundary, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Path is outside the owned workspace: $absolute"
    }
}

function New-OwnedImage([string]$Name, [int]$SectorSize) {
    $image = Join-Path $workRoot ($Name + '.vhdx')
    Assert-OwnedPath $image
    $images.Add($image)
    $null = New-VHD -Path $image -Dynamic -SizeBytes $size -LogicalSectorSizeBytes $SectorSize -PhysicalSectorSizeBytes 4096
    return $image
}

function Get-OwnedDisk([string]$Image) {
    Assert-OwnedPath $Image
    if (-not $images.Contains($Image)) { throw 'Image was not created by this run' }
    $vhd = Get-VHD -Path $Image
    if (-not $vhd.Attached -or $vhd.DiskNumber -eq $null) { throw 'Owned image is not attached' }
    $disk = Get-Disk -Number $vhd.DiskNumber
    # PowerShell 7's Windows compatibility session can deserialize BusType as
    # its display name; native Storage CIM objects may expose the numeric value.
    if ($disk.Size -ne $size -or [string]$disk.BusType -notin @('File Backed Virtual', '15') -or $disk.IsBoot -or $disk.IsSystem) {
        throw 'Refusing operation on a disk that is not the expected owned VHD'
    }
    return $disk
}

function Attach-OwnedImage([string]$Image, [bool]$ReadOnly = $true) {
    Assert-OwnedPath $Image
    $null = Mount-VHD -Path $Image -NoDriveLetter -ReadOnly:$ReadOnly
    return Get-OwnedDisk $Image
}

function Invoke-Cli([string[]]$CommandArguments, [int]$ExpectedExit = 0) {
    $lines = & $Binary --quiet @CommandArguments 2> (Join-Path $workRoot 'cli-stderr.txt')
    $code = $LASTEXITCODE
    $report = ($lines -join "`n") | ConvertFrom-Json
    if ($code -ne $ExpectedExit -or $report.exit_code -ne $ExpectedExit) {
        throw "CLI exit $code (expected $ExpectedExit): $($lines -join ' ')"
    }
    return $report
}

function Get-ZeroDigest {
    $sha = [Security.Cryptography.SHA256]::Create()
    try {
        $block = [byte[]]::new(65536)
        for ($offset = 0; $offset -lt $size; $offset += $block.Length) {
            $null = $sha.TransformBlock($block, 0, $block.Length, $block, 0)
        }
        $null = $sha.TransformFinalBlock([byte[]]::new(0), 0, 0)
        return ([BitConverter]::ToString($sha.Hash)).Replace('-', '').ToLowerInvariant()
    } finally { $sha.Dispose() }
}

function Assert-Oracle([string]$Output, [string]$Expected) {
    if (-not $EwfExport) { return }
    $target = Join-Path $workRoot ([Guid]::NewGuid().ToString('N'))
    $null = & $EwfExport -q -u -f raw -t $target $Output 2>&1
    if ($LASTEXITCODE -ne 0) { throw 'ewfexport failed' }
    if ((Get-FileHash -LiteralPath ($target + '.raw') -Algorithm SHA256).Hash.ToLowerInvariant() -ne $Expected) {
        throw 'libewf exported bytes differ from the independent synthetic digest'
    }
    $null = & $EwfVerify -q $Output 2>&1
    if ($LASTEXITCODE -ne 0) { throw 'ewfverify failed' }
}

try {
    $expected = Get-ZeroDigest
    foreach ($sector in @(512, 4096)) {
        # Fresh uninitialized VHDX media is zero-filled. Attach read-only so
        # Windows cannot initialize signatures or filesystems on this source.
        $image = New-OwnedImage "source-$sector" $sector
        $disk = Attach-OwnedImage $image
        $number = $disk.Number
        $device = "\\.\PhysicalDrive$number"
        $output = Join-Path $workRoot "sector-$sector.E01"
        $wrongSector = if ($sector -eq 512) { 4096 } else { 512 }
        $wrong = Invoke-Cli @('acquire', $device, (Join-Path $workRoot "wrong-$sector.E01"), '--sector-size', "$wrongSector") 1
        if ($wrong.error -notmatch 'geometry') { throw 'Expected device geometry mismatch' }
        $paused = Invoke-Cli @('acquire', $device, $output, '--stop-after', '4194304', '--chunks-per-segment', '32') 130
        if ($paused.checkpoint_bytes -ne 4194304) { throw 'Incorrect checkpoint offset' }
        Dismount-VHD -Path $image
        $null = Invoke-Cli @('resume', $output) 1
        $null = Invoke-Cli @('checkpoint', 'validate', $output)
        $replacement = New-OwnedImage "replacement-$sector" $sector
        $other = Attach-OwnedImage $replacement
        if ($other.Number -ne $number) { throw 'Replacement did not reuse the test disk slot' }
        $changed = Invoke-Cli @('resume', $output) 1
        if ($changed.error -notmatch 'identity') { throw 'Expected replacement identity rejection' }
        Dismount-VHD -Path $replacement
        $disk = Attach-OwnedImage $image
        if ($disk.Number -ne $number) { throw 'Source disk number changed during test' }
        $done = Invoke-Cli @('resume', $output)
        if ($done.status -ne 'complete' -or $done.verification.sha256 -ne $expected -or $done.source.sector_size -ne $sector) {
            throw 'Resumed media does not match independent zero-source digest and geometry'
        }
        Assert-Oracle $output $expected
        # A second fresh acquisition checks that source logical bytes remain zero.
        $again = Invoke-Cli @('acquire', $device, (Join-Path $workRoot "unchanged-$sector.E01"))
        if ($again.verification.sha256 -ne $expected) { throw 'Source media changed' }
        Dismount-VHD -Path $image
        $checks.Add("$sector-byte VHDX: geometry, pause, detach, replacement rejection, resume, unchanged media")
    }

    # Format only a new, ownership-checked test VHD to exercise destination overlap.
    $image = New-OwnedImage 'overlap' 512
    $disk = Attach-OwnedImage $image $false
    $disk = Get-OwnedDisk $image
    $null = Initialize-Disk -Number $disk.Number -PartitionStyle GPT
    $disk = Get-OwnedDisk $image
    $partition = New-Partition -DiskNumber $disk.Number -UseMaximumSize
    $null = $partition | Format-Volume -FileSystem NTFS -Confirm:$false
    $mountpoint = Join-Path $workRoot 'mounted'
    $null = New-Item -ItemType Directory -Path $mountpoint
    $access = $mountpoint + '\'
    Assert-OwnedPath $access
    Add-PartitionAccessPath -DiskNumber $disk.Number -PartitionNumber $partition.PartitionNumber -AccessPath $access
    $accessPaths.Add(@{ Image = $image; Partition = $partition.PartitionNumber; Path = $access })
    $collision = Invoke-Cli @('acquire', "\\.\PhysicalDrive$($disk.Number)", (Join-Path $mountpoint 'case.E01')) 1
    if ($collision.error -notmatch 'destination') { throw 'Expected destination overlap rejection' }
    if (Get-ChildItem -LiteralPath $mountpoint -Force | Where-Object Name -Like '.case*') { throw 'Overlap created output state' }
    $checks.Add('mounted-folder destination overlap rejected before state creation')
} finally {
    # Cleanup uses only recorded images, and validates all paths before deletion.
    # Any cleanup failure retains the workspace instead of deleting attached files.
    try {
        foreach ($entry in $accessPaths) {
            Assert-OwnedPath $entry.Path
            $disk = Get-OwnedDisk $entry.Image
            Remove-PartitionAccessPath -DiskNumber $disk.Number -PartitionNumber $entry.Partition -AccessPath $entry.Path
        }
        foreach ($image in $images) {
            Assert-OwnedPath $image
            if ((Get-VHD -Path $image).Attached) { Dismount-VHD -Path $image }
        }
        $resolved = (Resolve-Path -LiteralPath $workRoot).ProviderPath
        if (-not [string]::Equals([IO.Path]::GetFullPath($resolved), $workRoot, [StringComparison]::OrdinalIgnoreCase)) {
            throw 'Workspace path changed; refusing recursive cleanup'
        }
        Remove-Item -LiteralPath $resolved -Recurse -Force
    } catch {
        Write-Warning "Cleanup incomplete; retained owned workspace: $workRoot"
        throw
    }
}
@{ schema_version = 1; status = 'passed'; checks = $checks.ToArray(); oracle_validation = $(if ($EwfExport) { 'passed' } else { 'not_run' }) } | ConvertTo-Json -Depth 5

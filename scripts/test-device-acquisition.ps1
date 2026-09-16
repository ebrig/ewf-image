#requires -Version 7.0
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
    'Add-PartitionAccessPath', 'Remove-PartitionAccessPath', 'Set-Disk') | Where-Object { -not (Get-Command $_ -ErrorAction SilentlyContinue) }
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
$imageSizes = @{}
$accessPaths = [Collections.Generic.List[object]]::new()
$checks = [Collections.Generic.List[string]]::new()
$activeProcesses = [Collections.Generic.List[Diagnostics.Process]]::new()
$size = 64MB

function Assert-OwnedPath([string]$Path) {
    $absolute = [IO.Path]::GetFullPath($Path)
    if ($absolute.StartsWith('\\?\')) { $absolute = $absolute.Substring(4) }
    if (-not $absolute.StartsWith($rootBoundary, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Path is outside the owned workspace: $absolute"
    }
}

function New-OwnedImage([string]$Name, [int]$SectorSize, [long]$Capacity = $size) {
    $image = Join-Path $workRoot ($Name + '.vhdx')
    Assert-OwnedPath $image
    $images.Add($image)
    $imageSizes[$image] = $Capacity
    $null = New-VHD -Path $image -Dynamic -SizeBytes $Capacity -LogicalSectorSizeBytes $SectorSize -PhysicalSectorSizeBytes 4096
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
    if ($disk.Size -ne $imageSizes[$Image] -or [string]$disk.BusType -notin @('File Backed Virtual', '15') -or $disk.IsBoot -or $disk.IsSystem) {
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
        throw "CLI exit $code (expected $ExpectedExit), phase=$($report.phase), status=$($report.status): $($report.error)"
    }
    return $report
}

function New-SyntheticSource {
    $path = Join-Path $workRoot 'source.raw'
    Assert-OwnedPath $path
    $stream = [IO.File]::Open($path, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
    try {
        $block = [byte[]]::new(65536)
        $random = [Random]::new(20260925)
        for ($offset = 0; $offset -lt $size; $offset += $block.Length) {
            $random.NextBytes($block)
            # Distinct blocks catch displaced/repeated reads. Avoid an accidental
            # MBR signature so Windows does not interpret random partition entries.
            [BitConverter]::GetBytes([long]$offset).CopyTo($block, 0)
            if ($offset -eq 0) { $block[510] = 0; $block[511] = 0 }
            $stream.Write($block, 0, $block.Length)
        }
        $stream.Flush($true)
    } finally { $stream.Dispose() }
    return $path
}

function Initialize-OwnedSource([string]$Image, [string]$Raw) {
    Assert-OwnedPath $Raw
    $disk = Attach-OwnedImage $Image $false
    $disk = Get-OwnedDisk $Image
    Set-Disk -Number $disk.Number -IsReadOnly $false
    $disk = Get-OwnedDisk $Image
    Set-Disk -Number $disk.Number -IsOffline $true
    $disk = Get-OwnedDisk $Image
    if (-not $disk.IsOffline -or $disk.IsReadOnly -or (Get-Item -LiteralPath $Raw).Length -ne $disk.Size) {
        throw 'Owned source must be offline and match the synthetic data size'
    }
    # This is the only raw-device write: an ownership-checked, newly created,
    # offline VHDX. Acquisition reattaches it read-only after seeding.
    $destination = [IO.File]::Open("\\.\PhysicalDrive$($disk.Number)", [IO.FileMode]::Open, [IO.FileAccess]::Write, [IO.FileShare]::ReadWrite)
    try {
        $source = [IO.File]::OpenRead($Raw)
        try { $source.CopyTo($destination, 65536); $destination.Flush($true) }
        finally { $source.Dispose() }
    } finally { $destination.Dispose() }
    Dismount-VHD -Path $Image
}

function Assert-Oracle([string]$Output, [string]$Expected) {
    if (-not $EwfExport) { return }
    $target = Join-Path $workRoot ([Guid]::NewGuid().ToString('N'))
    $oracleLog = & $EwfExport -q -u -f raw -t $target $Output 2>&1
    if ($LASTEXITCODE -ne 0) { throw "ewfexport failed: $($oracleLog -join ' ')" }
    if ((Get-FileHash -LiteralPath ($target + '.raw') -Algorithm SHA256).Hash.ToLowerInvariant() -ne $Expected) {
        throw 'libewf exported bytes differ from the independent synthetic digest'
    }
    $oracleLog = & $EwfVerify -q $Output 2>&1
    if ($LASTEXITCODE -ne 0) { throw "ewfverify failed: $($oracleLog -join ' ')" }
}

function Test-ActiveRemoval([string]$Image, [string]$Expected) {
    $disk = Attach-OwnedImage $Image
    $number = $disk.Number
    $output = Join-Path $workRoot 'active-removal.E01'
    $stdout = Join-Path $workRoot 'active-stdout.json'
    $stderr = Join-Path $workRoot 'active-stderr.txt'
    # Frequent small seals leave enough time to detach after a real checkpoint.
    # No delay/fault-injection switches are added to the production executable.
    $arguments = @('--quiet', 'acquire', "\\.\PhysicalDrive$number", $output,
        '--compression', 'raw', '--sectors-per-chunk', '1', '--chunks-per-segment', '128', '--zero-fill', '--retries', '0')
    $quoted = $arguments | ForEach-Object { '"' + $_ + '"' }
    $process = Start-Process -FilePath $Binary -ArgumentList $quoted -WindowStyle Hidden -PassThru -RedirectStandardOutput $stdout -RedirectStandardError $stderr
    $activeProcesses.Add($process)
    $checkpoint = Join-Path $workRoot '.active-removal.E01.ewf-acquisition/checkpoint-00001'
    $deadline = [DateTime]::UtcNow.AddSeconds(60)
    while (-not [IO.File]::Exists($checkpoint)) {
        if ($process.HasExited -or [DateTime]::UtcNow -gt $deadline) {
            throw "Acquisition did not reach a live checkpoint: $(Get-Content -LiteralPath $stdout -Raw)"
        }
        Start-Sleep -Milliseconds 10
    }
    if ($process.HasExited) { throw 'Acquisition completed before the removal test' }
    $null = Get-OwnedDisk $Image
    Dismount-VHD -Path $Image
    if (-not $process.WaitForExit(30000)) { throw 'Acquisition did not exit after VHDX removal' }
    $process.Refresh()
    $report = Get-Content -LiteralPath $stdout -Raw | ConvertFrom-Json
    if ($process.ExitCode -ne 1 -or $report.exit_code -ne 1 -or $report.published -or
        $report.substituted_sectors -ne 0 -or $report.accepted_bytes -ge $size -or $report.checkpoint_bytes -le 0) {
        throw "Removal must stop without substitution/publication and preserve a checkpoint: $($report | ConvertTo-Json -Compress -Depth 8)"
    }
    $null = Invoke-Cli @('checkpoint', 'validate', $output)
    $disk = Attach-OwnedImage $Image
    if ($disk.Number -ne $number) { throw 'Source disk slot changed during active-removal test' }
    $done = Invoke-Cli @('resume', $output)
    if ($done.verification.sha256 -ne $Expected) { throw 'Active-removal resume changed media' }
    Assert-Oracle $output $Expected
    Dismount-VHD -Path $Image
    $checks.Add('active VHDX removal: no zero substitution, retained checkpoint, verified resume')
}

try {
    $raw = New-SyntheticSource
    $expected = (Get-FileHash -LiteralPath $raw -Algorithm SHA256).Hash.ToLowerInvariant()
    foreach ($sector in @(512, 4096)) {
        $image = New-OwnedImage "source-$sector" $sector
        Initialize-OwnedSource $image $raw
        $disk = Attach-OwnedImage $image
        $number = $disk.Number
        $device = "\\.\PhysicalDrive$number"
        $output = Join-Path $workRoot "sector-$sector.E01"
        $wrongSector = if ($sector -eq 512) { 4096 } else { 512 }
        $wrong = Invoke-Cli @('acquire', $device, (Join-Path $workRoot "wrong-$sector.E01"), '--sector-size', "$wrongSector") 1
        if ($wrong.error -notmatch 'geometry') { throw "Expected device geometry mismatch: $($wrong.error)" }
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
            throw 'Resumed media does not match independent synthetic-source digest and geometry'
        }
        Assert-Oracle $output $expected
        # A second fresh acquisition checks that all source logical bytes are unchanged.
        $again = Invoke-Cli @('acquire', $device, (Join-Path $workRoot "unchanged-$sector.E01"))
        if ($again.verification.sha256 -ne $expected) { throw 'Source media changed' }
        Dismount-VHD -Path $image
        $checks.Add("$sector-byte VHDX: geometry, pause, detach, replacement rejection, resume, unchanged media")
    }

    Test-ActiveRemoval (Join-Path $workRoot 'source-512.vhdx') $expected

    # Format only a new, ownership-checked test VHD to exercise destination overlap.
    $image = New-OwnedImage 'overlap' 512 128MB
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

    # Leave only 8 MiB free on this private NTFS volume. Allocation is performed
    # by real writes; SetLength alone is not evidence that clusters were consumed.
    $filler = Join-Path $mountpoint 'filler.bin'
    Assert-OwnedPath $filler
    $volume = Get-Volume -FilePath $access
    $fillBytes = [long]$volume.SizeRemaining - 8MB
    if ($fillBytes -lt 16MB) { throw 'Insufficient private volume capacity for the full-disk test' }
    $stream = [IO.File]::Open($filler, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
    try {
        $block = [byte[]]::new(65536)
        while ($fillBytes -gt 0) {
            $count = [int][Math]::Min($fillBytes, $block.Length)
            $stream.Write($block, 0, $count)
            $fillBytes -= $count
        }
        $stream.Flush($true)
    } finally { $stream.Dispose() }
    $output = Join-Path $mountpoint 'full.E01'
    $failed = Invoke-Cli @('acquire', $raw, $output, '--compression', 'raw', '--chunks-per-segment', '16') 1
    if ($failed.checkpoint_bytes -le 0 -or $failed.checkpoint_bytes -ge $size -or $failed.error -notmatch '112|disk.*full|space') {
        throw "Expected actual disk-full failure with retained checkpoint: $($failed | ConvertTo-Json -Compress)"
    }
    $null = Invoke-Cli @('checkpoint', 'validate', $output)
    Assert-OwnedPath $filler
    Remove-Item -LiteralPath $filler -Force
    $done = Invoke-Cli @('resume', $output)
    if ($done.verification.sha256 -ne $expected) { throw 'Disk-full resume changed media' }
    # Stage verified byte-identical copies on the host filesystem. A WSL oracle
    # cannot traverse a freshly mounted Windows NTFS volume through DrvFS.
    if ($EwfExport) {
        $oracleDirectory = Join-Path $workRoot 'full-oracle'
        $null = New-Item -ItemType Directory -Path $oracleDirectory
        foreach ($segment in $done.segments) {
            Assert-OwnedPath $segment
            $copy = Join-Path $oracleDirectory ([IO.Path]::GetFileName($segment))
            Assert-OwnedPath $copy
            Copy-Item -LiteralPath $segment -Destination $copy
            if ((Get-FileHash -LiteralPath $segment).Hash -ne (Get-FileHash -LiteralPath $copy).Hash) {
                throw 'Oracle staging changed segment bytes'
            }
        }
        Assert-Oracle (Join-Path $oracleDirectory 'full.E01') $expected
    }
    $checks.Add('actual NTFS disk full, retained checkpoint, capacity restoration, verified resume')
} finally {
    # Cleanup uses only recorded images, and validates all paths before deletion.
    # Any cleanup failure retains the workspace instead of deleting attached files.
    try {
        foreach ($process in $activeProcesses) {
            if (-not $process.HasExited) {
                $process.Kill()
                if (-not $process.WaitForExit(10000)) { throw 'Owned test process did not terminate' }
            }
            $process.Dispose()
        }
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

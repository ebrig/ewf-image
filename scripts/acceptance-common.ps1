# Process execution and evidence recording shared with the runner's self-test.
function Save-AcceptanceManifest($Manifest, [string]$Directory) {
    $path = Join-Path $Directory 'acceptance.json'
    [IO.File]::WriteAllText(($path + '.pending'), ($Manifest | ConvertTo-Json -Depth 16))
    [IO.File]::Move(($path + '.pending'), $path, $true)
}

function Add-AcceptanceSkip($Manifest, [string]$Directory, [string]$Name, [string]$Reason) {
    $Manifest.checks.Add(@{ name = $Name; status = 'skipped'; reason = $Reason })
    Save-AcceptanceManifest $Manifest $Directory
}

function Set-AcceptanceToolVersion($Manifest, [string]$Directory, [string]$Name, [string]$Program, $Record) {
    $tool = @{ status = $Record.status; program = $Program; stdout = $Record.stdout; stderr = $Record.stderr }
    if ($Record.status -eq 'passed') {
        $parts = foreach ($stream in @('stdout', 'stderr')) {
            if ($Record[$stream]) {
                $reader = [IO.File]::OpenText((Join-Path $Directory $Record[$stream].path))
                try {
                    $buffer = [char[]]::new(4096)
                    $count = $reader.ReadBlock($buffer, 0, $buffer.Length)
                    if ($count) { [string]::new($buffer, 0, $count) }
                } finally { $reader.Dispose() }
            }
        }
        $tool.version = ($parts -join "`n").Trim()
        $tool.version_available = [bool]$tool.version
    }
    $Manifest.tools[$Name] = $tool
    Save-AcceptanceManifest $Manifest $Directory
}

function Invoke-AcceptanceCheck {
    param(
        $Manifest, [string]$Directory, [string]$Name, [string]$Program,
        [string[]]$Arguments, [string]$WorkingDirectory,
        [hashtable]$Environment = @{}, [int[]]$BlockedExitCodes = @()
    )
    if ($Name -notmatch '^[a-z0-9-]+$' -or @($Manifest.checks | Where-Object name -EQ $Name).Count) { throw 'Check names must be unique safe filenames' }
    $record = [ordered]@{ name = $Name; status = 'running'; program = $Program; arguments = @($Arguments); environment = $Environment; started_utc = [DateTime]::UtcNow.ToString('o') }
    $Manifest.checks.Add($record)
    Save-AcceptanceManifest $Manifest $Directory
    Write-Host "Acceptance: $Name"
    $timer = [Diagnostics.Stopwatch]::StartNew()
    $stdoutPath = Join-Path $Directory "$Name.stdout.log"
    $stderrPath = Join-Path $Directory "$Name.stderr.log"
    $stdout = $null; $stderr = $null; $process = $null
    try {
        $start = [Diagnostics.ProcessStartInfo]::new($Program)
        $start.UseShellExecute = $false
        $start.CreateNoWindow = $true
        $start.RedirectStandardOutput = $true
        $start.RedirectStandardError = $true
        $start.WorkingDirectory = $WorkingDirectory
        foreach ($argument in $Arguments) { $start.ArgumentList.Add($argument) }
        foreach ($key in $Environment.Keys) { $start.Environment[$key] = $Environment[$key] }
        $stdout = [IO.File]::Open($stdoutPath, [IO.FileMode]::CreateNew)
        $stderr = [IO.File]::Open($stderrPath, [IO.FileMode]::CreateNew)
        $process = [Diagnostics.Process]::new()
        $process.StartInfo = $start
        $null = $process.Start()
        $record.pid = $process.Id
        Save-AcceptanceManifest $Manifest $Directory
        # Drain both streams concurrently, directly to disk; do not retain large
        # test reports in RAM or deadlock on a full redirected pipe.
        $outCopy = $process.StandardOutput.BaseStream.CopyToAsync($stdout)
        $errCopy = $process.StandardError.BaseStream.CopyToAsync($stderr)
        $process.WaitForExit()
        [Threading.Tasks.Task]::WhenAll([Threading.Tasks.Task[]]@($outCopy, $errCopy)).GetAwaiter().GetResult()
        $record.exit_code = $process.ExitCode
        $record.status = if ($process.ExitCode -eq 0) { 'passed' } elseif ($process.ExitCode -in $BlockedExitCodes) { 'blocked' } else { 'failed' }
    } catch {
        $record.status = 'failed'
        $record.error = $_.Exception.Message
    } finally {
        if ($stdout) { $stdout.Dispose() }
        if ($stderr) { $stderr.Dispose() }
        if ($process) { $process.Dispose() }
        $timer.Stop()
        $record.elapsed_seconds = $timer.Elapsed.TotalSeconds
        $record.finished_utc = [DateTime]::UtcNow.ToString('o')
        foreach ($stream in @('stdout', 'stderr')) {
            $path = Join-Path $Directory "$Name.$stream.log"
            if (Test-Path -LiteralPath $path) {
                $record[$stream] = @{ path = [IO.Path]::GetFileName($path); bytes = (Get-Item -LiteralPath $path).Length; sha256 = (Get-FileHash -LiteralPath $path).Hash.ToLowerInvariant() }
            }
        }
        Save-AcceptanceManifest $Manifest $Directory
    }
    return $record
}

function Complete-AcceptanceManifest($Manifest, [string]$Directory) {
    $bad = @($Manifest.checks | Where-Object { $_.status -in @('failed', 'blocked', 'running') })
    $skips = @($Manifest.checks | Where-Object status -EQ 'skipped')
    $Manifest.status = if ($bad.Count -or $Manifest.Contains('runner_error')) { 'failed' } elseif ($skips.Count) { 'passed_with_skips' } else { 'passed' }
    $Manifest.release_ready = $false
    $Manifest.finished_utc = [DateTime]::UtcNow.ToString('o')
    Save-AcceptanceManifest $Manifest $Directory
    $hash = (Get-FileHash -LiteralPath (Join-Path $Directory 'acceptance.json')).Hash.ToLowerInvariant()
    [IO.File]::WriteAllText((Join-Path $Directory 'acceptance.sha256'), "$hash  acceptance.json`n")
}

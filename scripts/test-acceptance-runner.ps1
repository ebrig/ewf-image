# Self-test uses child processes and synthetic logs only; no repository mutation.
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'acceptance-common.ps1')
$directory = Join-Path ([IO.Path]::GetTempPath()) ('acceptance-selftest-' + [Guid]::NewGuid().ToString('N'))
$null = New-Item -ItemType Directory -Path $directory
$manifest = [ordered]@{ schema_version = 1; status = 'running'; checks = [Collections.Generic.List[object]]::new(); tools = [ordered]@{}; release_ready = $false; finished_utc = $null }
$pwsh = (Get-Process -Id $PID).Path
$common = @{ Manifest = $manifest; Directory = $directory; Program = $pwsh; WorkingDirectory = $directory }
$ok = Invoke-AcceptanceCheck @common -Name success -Arguments @('-NoProfile', '-Command', '[Console]::Write(("o" * 131072)); [Console]::Error.Write(("e" * 131072)); exit 0')
$fail = Invoke-AcceptanceCheck @common -Name failure -Arguments @('-NoProfile', '-Command', 'exit 7')
$blocked = Invoke-AcceptanceCheck @common -Name unavailable -BlockedExitCodes @(77) -Arguments @('-NoProfile', '-Command', 'exit 77')
$missing = Invoke-AcceptanceCheck -Manifest $manifest -Directory $directory -Name missing -Program (Join-Path $directory 'missing-executable') -WorkingDirectory $directory
$stderrVersion = Invoke-AcceptanceCheck @common -Name stderr-version -Arguments @('-NoProfile', '-Command', '[Console]::Error.WriteLine("tool 1.2"); exit 0')
$emptyVersion = Invoke-AcceptanceCheck @common -Name empty-version -Arguments @('-NoProfile', '-Command', 'exit 0')
Set-AcceptanceToolVersion $manifest $directory 'stderr-only' $pwsh $stderrVersion
Set-AcceptanceToolVersion $manifest $directory 'empty' $pwsh $emptyVersion
if ($manifest.tools['stderr-only'].version -cne 'tool 1.2' -or -not $manifest.tools['stderr-only'].version_available -or $manifest.tools['empty'].version_available) { throw 'Incorrect version stream handling' }
Add-AcceptanceSkip $manifest $directory 'manual' 'requires a human consumer check'
Complete-AcceptanceManifest $manifest $directory
if ($ok.status -ne 'passed' -or $fail.status -ne 'failed' -or $fail.exit_code -ne 7 -or $blocked.status -ne 'blocked' -or $missing.status -ne 'failed' -or -not $missing.error -or $manifest.status -ne 'failed') { throw 'Incorrect process status classification' }
if ($ok.stdout.bytes -ne 131072 -or $ok.stderr.bytes -ne 131072) { throw 'Redirected output was truncated' }
foreach ($record in @($ok, $fail, $blocked)) {
    foreach ($stream in @('stdout', 'stderr')) {
        if ((Get-FileHash -LiteralPath (Join-Path $directory $record[$stream].path)).Hash.ToLowerInvariant() -ne $record[$stream].sha256) { throw 'Log hash mismatch' }
    }
}
$manifest.checks.Remove($fail) | Out-Null
$manifest.checks.Remove($blocked) | Out-Null
$manifest.checks.Remove($missing) | Out-Null
Complete-AcceptanceManifest $manifest $directory
if ($manifest.status -ne 'passed_with_skips' -or $manifest.release_ready) { throw 'Skipped acceptance incorrectly claimed complete' }
$saved = Get-Content (Join-Path $directory 'acceptance.json') -Raw | ConvertFrom-Json
$expected = (Get-FileHash -LiteralPath (Join-Path $directory 'acceptance.json')).Hash.ToLowerInvariant()
if ((Get-Content (Join-Path $directory 'acceptance.sha256') -Raw) -cne "$expected  acceptance.json`n" -or $saved.checks.Count -ne 4) { throw 'Manifest integrity mismatch' }
# Delete only this exact GUID-owned temporary directory after validating its path.
$resolved = (Resolve-Path -LiteralPath $directory).Path
if (-not [string]::Equals($resolved, [IO.Path]::GetFullPath($directory), [StringComparison]::OrdinalIgnoreCase)) { throw 'Self-test directory changed' }
Remove-Item -LiteralPath $resolved -Recurse -Force
'Acceptance runner self-test passed'

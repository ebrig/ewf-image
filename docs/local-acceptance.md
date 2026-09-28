# Local acceptance

The local acceptance runner produces reproducible validation evidence for a
committed checkout. Run it from the repository root with PowerShell 7. The runner
does not tag, push, publish, install toolchains, or automate GUI applications.

The runner requires a **new** output directory and creates an isolated native
Cargo target inside it. The directory keeps the logs, the `ewf-cli` release
executable, the verified crate package, and optional consumer fixtures. Allow for the disk space
and build time of a full Cargo build.

```powershell
./scripts/run-local-acceptance.ps1 -OutputDirectory C:\acceptance\ewf-candidate
```

The default checks cover runner self-tests, formatting, workspace tests with all
features and with no default features, Clippy, rustdoc, package verification, and
release builds. The `build` directory contains compiler intermediates and is
excluded from the artifact index. The copied executable and the crate package
are indexed separately. Use an output directory outside the repository or a
location ignored by Git, such as `target`.

## Optional checks

Supplying local inputs enables additional checks:

```powershell
./scripts/run-local-acceptance.ps1 `
  -OutputDirectory C:\acceptance\ewf-candidate-full `
  -FixtureSourceDirectory C:\fixtures\synthetic `
  -EwfExport C:\libewf\ewfexport.exe -EwfVerify C:\libewf\ewfverify.exe `
  -WslDistribution kali-linux `
  -WslLibewfPrefix /opt/libewf-20260924 `
  -WslAff4Oracle /opt/aff4tools/target/release/aff4tools
```

The oracle options also accept PowerShell `.ps1` wrapper scripts. The EWF helper
requires libewf 20260924. The WSL suite records the installed versions and the
SHA256 of the AFF4 oracle binary. Install the intended independent oracle before
the run. WSL builds use a separate Cargo target, which defaults to
`/tmp/ewf-local-acceptance-target` and can be changed with `-WslTargetDirectory`.
Native and Linux builds cannot share a Cargo target.

The runner only reads the fixture source directory. Newly prepared fixtures
record the committed runtime of the run and the executable and segment hashes.
Their EnCase status is set to `not tested`.

Add `-RunWindowsDevices` only on a disposable Windows lab host. The read-only
prerequisite probe runs first. If the probe exits with code 77, the check is
recorded as **blocked** and the VHDX operations are skipped. When the
prerequisites are satisfied, the ownership-checked NTFS and exFAT Ex01/Lx01
harness runs. Supplying both oracle tools also enables independent export checks
in that harness. The runner does not bypass elevation.

## Bundle contents

`acceptance.json` records the starting and ending Git revision, tree, and working
status, as well as host and tool details. `acceptance.json` also records each
command with its build-environment overrides, timestamps, exit codes, the SHA256
of each check's stdout and stderr, and the sizes and hashes of retained
artifacts. The manifest is saved before and after the checks, so an interrupted
run can remain in the `running` state and is not complete. `acceptance.sha256`
covers the final manifest.

Hashes establish identity, not authenticity. Keep the bundle together with its
logs. Manifests contain local paths and fixture metadata.

## Interpret results

Each check has one of these statuses: `passed`, `failed`, `blocked`, `running`,
or `skipped`. A failed, blocked, or unfinished check, or a runner error, makes the
overall status `failed` with exit code 1. Otherwise, any skipped checks produce
`passed_with_skips` with exit code 0. A zero exit code does **not** establish
release readiness, and `release_ready` is always false.

The runner always skips manual EnCase checks, macOS and MSRV checks, sustained
fuzzing, large-scale benchmarks, and privileged Linux storage tests. Run and
record those checks separately. Earlier validation does not count as a pass for
the current run. Repository changes during a run invalidate the revision
recorded for that run.

## Planning and runner development

`-PlanOnly` records the checks that would be skipped and probes tool versions
without building anything or opening devices. `-AllowDirty` is intended for
developing the runner itself. `-AllowDirty` is recorded prominently, and a run
that uses it does not identify an exact committed source snapshot. Normal
candidate runs must start from a clean tree. Neither option turns skipped checks
into passes.

The runner self-test can also be run on its own:

```powershell
./scripts/test-acceptance-runner.ps1
```

The self-test uses synthetic child processes to check capture of large
simultaneous stdout and stderr output, classification of exit, blocked, and
spawn-failure results, manifest and log hashes, and incomplete acceptance status.

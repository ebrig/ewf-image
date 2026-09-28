# Virtual-device acceptance

These repository scripts test operating-system device behavior using newly
created synthetic images. The scripts never select existing physical disks for
formatting. Run the privileged tests on a disposable development or CI host, and
do not manipulate the test devices while the tests run. Every mount and device
is tracked, and cleanup checks ownership before removal. If cleanup fails, the
script keeps the workspace and prints its location.

## Select a harness

| Environment | Harness | Purpose |
| --- | --- | --- |
| Linux, root | `test-device-acquisition.py` | E01 devices, errors, cancellation, overlap, and recovery |
| Windows, elevated PowerShell 7 | `test-device-acquisition.ps1` | Owned VHDX device/storage scenarios |
| Windows, ordinary files | `test-ewf2-oracle.ps1` | Independent Ex01/Lx01 export checks without device attachment |

Ordinary-file checks do not validate filesystem-full or virtual-device behavior.

## Linux

The Linux harness requires root, Python 3.9 or later, loop and device-mapper
kernel support, `losetup`, `dmsetup`, `mount`, `umount`, `mkfs.ext4`, `setpriv`,
`findmnt`, and the pinned libewf 20260924 tools. The workspace parent directory
must be on a resolvable block filesystem. The default, `/var/tmp`, avoids systems
where `/tmp` is a tmpfs.

```sh
cargo build --release -p ewf-cli --locked
sudo python3 scripts/test-device-acquisition.py \
  --binary "$PWD/target/release/ewf-cli" \
  --ewfexport /absolute/libewf/bin/ewfexport \
  --ewfverify /absolute/libewf/bin/ewfverify
```

The suite exercises:

- 512-byte and 4096-byte loop-device geometry and mismatched overrides.
- Checkpointed cancellation, source detachment, offline checkpoint validation,
  rejection of a replacement device in the same slot, reattachment, and verified resume.
- Permission failure without substitution or accepted data.
- Kernel read errors from a private device-mapper `error` target: the default
  stop, explicit zero substitution, and a native error range that matches the
  affected sectors exactly.
- Suspended device-mapper reads after a checkpoint. The read deadline and SIGINT
  each stop acquisition without substitution and report before the device is
  released. The retained checkpoint is validated and resumed to independently
  verified media. The harness confirms that the reader thread is blocked in the
  kernel before testing the stop.
- Destination overlap using a filesystem mounted from a new test image.
- Filesystem ENOSPC on a private tmpfs, followed by capacity expansion and
  recovery from the retained checkpoint.
- Independent source hashing, byte-correct libewf export, and `ewfverify`.

The Linux interoperability CI job includes this harness. Aligned uncached device
I/O isolates sector errors that buffered block reads can spread to neighboring
sectors.

The source image contains nonzero deterministic data. The source is opened
read-only through loop devices and checked afterward for unchanged bytes.
Filesystem formatting is confined to a separate new image. Cleanup unmounts owned
mounts, validates DM UUIDs before removal, and validates loop backing paths
before detachment. Do not disable Python assertions. The script refuses to run
under optimized Python.

## Windows

The Windows harness requires elevated PowerShell 7, the Hyper-V and Storage
cmdlets, and VHDX support. The following preflight check attaches no devices:

```powershell
./scripts/test-device-acquisition.ps1 -Binary ./target/release/ewf-cli.exe -CheckPrerequisites
```

Without elevation or the required cmdlets, the preflight reports `blocked` and
exits with code 77. On a suitable disposable lab host, run:

```powershell
./scripts/test-device-acquisition.ps1 -Binary ./target/release/ewf-cli.exe `
  -EwfExport C:\libewf\ewfexport.exe -EwfVerify C:\libewf\ewfverify.exe
```

The suite creates new VHDX sources with 512-byte and 4096-byte sectors and fills
each with 64 MiB of deterministic nonzero data. The suite writes only to
ownership-checked, offline test disks. The suite then attaches the sources
read-only and compares the acquired media with an independently calculated
source SHA256. The suite also tests pause, detachment, replacement, reattachment,
and resume. A second acquisition checks that the source is preserved. Another
case removes a VHDX during active acquisition. That case requires a fatal
disconnect with no zero substitution, a retained checkpoint, and a verified
resume across 1024 segments.

A separate new VHDX is ownership-checked before formatting. The suite uses it to
test a destination conflict with a mounted folder and actual NTFS exhaustion.
After an owned filler file is removed to restore capacity, acquisition must
resume from the retained checkpoint and produce the expected bytes. Cleanup stops
owned test processes and removes only verified literal workspace paths and
recorded VHDX images. If clean detachment cannot be confirmed, the files are kept.

The oracle tools are optional for local development. When they are absent, the
report states `oracle_validation: not_run`. The opt-in `windows-devices` CI job
requires both pinned tools and an elevated, disposable, self-hosted runner
labelled `ewf-device-lab`. The job runs only through manual workflow dispatch.
Normal pull request jobs never use that runner.

The Rust aligned-read regression uses a regular Windows file and does not replace
device acceptance. The VHDX ownership check handles both the numeric and the
display-name bus types returned by the Windows PowerShell compatibility session.

The harness also creates private NTFS and exFAT destinations to test Ex01 and
Lx01 capacity failure, process termination during staging, publication rollback,
and a fresh retry verified against the source SHA256. Use `-SequentialOnly` to
run only these cases. The harness removes each finished volume's access path and
detaches its VHDX before attaching the next filesystem.

When both pinned oracle tools are supplied, the harness copies the output
byte-for-byte onto the host filesystem and then checks the Ex01 media and the
exact Lx01 file paths and per-file SHA256 values. `ewfverify` uses `-f raw` for
Ex01 and `-f files` for Lx01. `sequential_oracle_validation` reports `passed`
only after all four filesystem and profile cases complete their oracle checks.
When the tools are absent, it reports `not_run`.

The shared oracle helper has its own acceptance test, which does not require
elevation:

```powershell
./scripts/test-ewf2-oracle.ps1 -Binary ./target/release/ewf-cli.exe `
  -EwfExport C:\libewf\ewfexport.exe -EwfVerify C:\libewf\ewfverify.exe
```

The helper checks split Ex01 and Lx01 output, nested and empty files, and the
rejection of incorrect expected paths and hashes. By default, it keeps the
synthetic output under `target`. When WSL cannot access a temporary Windows
mount, the WSL oracle wrappers can copy the segments byte-for-byte onto the host
filesystem. Hashes are checked before the oracle runs.

## Remaining boundaries

On Linux, virtual devices are removed between acquisition processes. On Windows,
removal during active acquisition is also tested. Physical hot-unplug,
uncooperative storage drivers, actual bad media, controller caches, hardware
write blockers, and sudden power loss require separate hardware tests.

Windows identity discovery queries the opened Rust handle through a small native
boundary used only by the CLI. Controller, SAN, and virtual-storage relationships
that Windows volume disk extents do not report are outside the destination-overlap
checks. None of these scripts certify power-loss durability or the consistency of
a live volume.

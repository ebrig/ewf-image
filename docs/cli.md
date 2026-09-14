# Acquisition command line

Build with `cargo build --release --features cli`, or install with
`cargo install --path . --features cli --locked`. The optional CLI dependencies
are excluded from ordinary library builds.

```text
ewf-image acquire disk.raw case.E01 --case-number CASE-123 --examiner "A. Examiner"
ewf-image resume case.E01
ewf-image checkpoint inspect case.E01
ewf-image checkpoint validate case.E01
ewf-image verify case.E01
```

Regular-file sources must be nonempty and sector aligned. The default
sector size is 512; `--sector-size` accepts 512, 1024, 2048, or 4096. Output is
physical E01 with raw (`--compression raw`) or default zlib compression.
`--sectors-per-chunk` and `--chunks-per-segment` set acquisition geometry.
Existing images and CLI session manifests are never overwritten.

Windows physical disks (`\\.\PhysicalDriveN`) and Linux block devices (`/dev/sdX`,
partitions, or device-mapper paths) can also be opened read-only. Geometry is
discovered; an explicit sector size must agree with it. The source must expose a
stable hardware identifier. Device IDs and geometry are checked before resume,
after opening, after a failed read, and after acquisition. The CLI does not
elevate privileges, lock/dismount volumes, or freeze a live filesystem. Use a
stable, appropriately write-protected source.

Windows uses the installed PowerShell Storage cmdlets (`Get-Disk`, `Get-Volume`,
and `Get-Partition`) with a static script and separately passed path values.
Linux uses sysfs geometry and WWID/serial/DM UUID metadata. Missing identifiers
or unresolved destination storage cause preflight failure. Source paths/device
numbers must remain stable across resume.

Device acquisition rejects destinations on the source disk. Linux also checks
partition parents and stacked-device slaves for shared backing storage. Windows
checks the disk IDs exposed by Storage cmdlets; hidden controller, SAN, and
virtual-storage relationships are outside that check. Network destinations and
unresolved storage layouts are unsupported for device acquisition. File sources
can use any destination supported by the writer.

Platform discovery and conflict logic have fixture tests; physical-device,
hot-unplug, and hardware write-blocker behavior still need hardware acceptance
testing. The CLI does not make a power-loss durability claim.

`acquire` automatically reopens the published image, decodes all media, and
compares its hashes with embedded references and the acquisition SHA256.
`verify` performs a fresh read using the same library, not an independent
implementation; libewf interoperability is tested separately. Successful hashes
cover zero substitutions too and do not establish recovery of unreadable data.

## Cancellation and resume

Ctrl+C requests cooperative cancellation. On Unix, SIGTERM and SIGHUP use the
same handler. The current OS operation or native segment seal must finish first.
`--stop-after BYTES` also pauses at an absolute accepted-byte offset, at chunk
granularity; it is useful for scheduled acquisition windows and reproducible
recovery testing. On resume, use a larger offset or omit it to finish.

Preserve both `.case.E01.ewf-session.json` and the entire
`.case.E01.ewf-acquisition` directory. The immutable versioned JSON manifest
stores source identity, geometry, and metadata; resume restores these options.
A persistent `.case.E01.ewf-cli.lock` coordinates CLI operations in addition to
the library's output lock. Do not remove control files while a command is active.

File identity uses canonical path, size, modification/creation metadata, and
Unix inode/device/change metadata where available. It is not a content digest
and cannot prove an unchanged source when metadata is deliberately preserved.
Use a stable source snapshot. Identity is checked before resume and after source
acquisition; changed sources are rejected. Inspection and validation do not need
the source to be present.

An interrupted manifest-only initialization can be started by `resume`. After
publication has retired the journal, use `verify` if verification was interrupted;
`resume` refuses to infer that an existing image belongs to this session.
Filesystem and power-loss limitations are described in [acquisition](acquisition.md).

## Read policy

`--retries N` allows 0 through 100 additional attempts per failed sector (default
2). The default stops on an unreadable sector. `--zero-fill` explicitly enables
zero substitution and native bad-sector records. EOF, permission, seek, and
configuration/disconnection errors remain fatal. `--checkpoint-interval BYTES` must be a
positive chunk-size multiple within the segment namespace limit. Read policies
can change on resume and apply only to future reads.

## Results

Progress is written to stderr at most once per second; `--quiet` suppresses it.
One JSON object is written to stdout on completion, cancellation, or operational
failure. Argument errors and help follow normal command-line conventions.
The report has `schema_version: 1`; additive fields may appear in that version.
It includes status, phase, elapsed time, accepted/checkpointed bytes when known,
publication status, acquisition-error ranges, and verification results.
`published: false` means this invocation has not confirmed publication; a failed
finish may still need recovery to resolve publication state.

| Exit | Meaning |
| --- | --- |
| 0 | Completed and verified, or checkpoint operation succeeded |
| 1 | Operational failure or result-output failure |
| 2 | Invalid command-line arguments |
| 3 | Verification failure, missing reference digests, or unreadable image |
| 4 | Completed/verified with recorded substituted sectors |
| 130 | Cancelled, including a requested `--stop-after` pause |

For example, `ewf-image acquire disk.raw case.E01 > result.json` retains the
report. Choose a new report path outside the source and image set: the shell
opens redirections before the program can check them. Manifests and reports can
contain case metadata and local source paths; handle them with the evidence.

Platform references: [Windows Get-Disk](https://learn.microsoft.com/en-us/powershell/module/storage/get-disk),
[Get-Volume](https://learn.microsoft.com/en-us/powershell/module/storage/get-volume),
[Get-Partition](https://learn.microsoft.com/en-us/powershell/module/storage/get-partition),
and [Linux sysfs block ABI](https://www.kernel.org/doc/Documentation/ABI/testing/sysfs-block).

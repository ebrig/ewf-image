# EWF command line

Build this checkout with `cargo build --release --features cli --locked`, or install with
`cargo install --path . --features cli --locked`. The optional CLI dependencies
are excluded from ordinary library builds.

Choose an operation below. Commands emit JSON results; progress goes to stderr.
Read [results and exit codes](#results) before automating decisions based on output.
The CLI has no password option for encrypted EWF1 images.

| Task | Commands |
| --- | --- |
| Inspect or check an image | `info`, `verify`, `analyze` |
| Export decoded media | `export` |
| Browse or extract logical files | `files`, `verify-file`, `extract-file` |
| Acquire/resume physical E01 | `acquire`, `resume`, `checkpoint inspect`, `checkpoint validate` |
| Create one-shot EWF2 output | `acquire-sequential`, `collect` |
| Resolve EWF2 publication | `recover-publication` |
| Export damaged EWF1 with provenance | `recover` |
| Read saved E01 acquisition history | `report` |

```text
ewf-image acquire disk.raw case.E01 --case-number CASE-123 --examiner "A. Examiner"
ewf-image resume case.E01
ewf-image checkpoint inspect case.E01
ewf-image checkpoint validate case.E01
ewf-image verify case.E01
ewf-image info case.E01
ewf-image files case.L01 --limit 1000
ewf-image verify-file case.L01 1
ewf-image extract-file case.L01 1 selected.bin
ewf-image analyze case.E01 --maximum-findings 1024
ewf-image recover damaged.E01 recovered-case --maximum-output-bytes 107374182400
ewf-image export case.E01 disk.raw
ewf-image report case.E01
ewf-image report case.E01 --write
```

## Logical file operations

`files IMAGE` lists L01/Lx01 catalog entries in preorder, with root index 0.
Each entry includes `parent_index` (null for the root), preserving hierarchy
across pages. Use `--offset` and `--limit` to page through the catalog; `next_offset` is null
on the last page. Indices select entries even when names collide or contain
characters that cannot be represented as destination filenames. They remain
stable only for the same unchanged image. Listing does not verify file data.

`verify-file IMAGE INDEX` strictly reads that regular file, computes MD5, SHA1,
and SHA256, and compares available file MD5/SHA1 references. Container hashes
are not file references. Missing file hashes return `file_hashes_missing`
(exit 4); mismatches, invalid references, and unreadable data fail (exit 3).
Directories and unknown entry types cannot be verified as regular files.

`extract-file IMAGE INDEX OUTPUT` streams the file into a temporary file beside
OUTPUT, checks its stored file hashes, synchronizes it, and publishes without
overwriting an existing destination. Cancellation, corruption, and hash mismatch
leave no final output. Missing references permit extraction but return
`extracted_without_reference` (exit 4), with computed digests for independent
comparison. Successful extraction with matching references returns exit 0.
The caller chooses OUTPUT; catalog names are never interpreted as destination
paths. Extraction copies content only, without restoring timestamps, ACLs,
links, extended attributes, or alternate streams. Both commands bypass decoded
caches and zero-on-error recovery. They verify only the selected file, not all
container media, and report `media_verified: false`.

## Metadata inspection

`info IMAGE` reports the format/profile, segment paths and sizes, media geometry,
case metadata, stored hashes, encryption detection, acquisition error ranges,
and whether a logical file catalog is present. It opens with strict structural
checks but does not scan media bytes: `media_verified` is false and `verification`
is null, even when inspection succeeds. Use `verify` for a full media check.
Legacy password headers and raw metadata sections are omitted. Encrypted images
are detected, but the CLI currently has no password input; inspection exits 1
with the encryption flag and open error when metadata cannot be opened.

## Integrity analysis

`analyze IMAGE` scans media and compares redundant tables, continuing after
individual chunk failures. Its `analysis` object reports complete, incomplete,
or unavailable coverage, findings, counts, and reference comparisons. Incomplete
scans never report whole-media hashes. Structural open failures become a finding
with unavailable coverage; analysis does not carve a broken descriptor chain.
`--maximum-findings` bounds retained findings (default 1024, maximum 100000),
while total and suppressed counts still cover the scan. Missing reference
hashes and recorded acquisition errors are warnings. Exit 3 indicates error
findings, exit 4 warnings only, and exit 0 no findings. Filesystem/operational
errors exit 1. Cancellation exits 130 with progress but no completed analysis;
opening and redundant-table inspection precede cancellable media progress.

## Strict raw export

`export IMAGE OUTPUT` copies the complete decoded media stream into a new raw
file. It supports the reader's unencrypted physical, SMART, and logical-image
formats. For logical images this is the flat media stream, not extraction of
individual files. Incomplete acquisitions are rejected. No damaged chunks are
silently zero-filled; use `recover` for explicit damaged-image recovery.

The command computes MD5, SHA1, and SHA256 while writing and compares every
supported stored digest. A mismatch exits 3 without publishing the destination.
Without reference digests, export can succeed, but `references_match` is null
and `media_verified` remains false. Hashes describe the decoded stream accepted
by the output writer; the raw file is not reread. Recorded acquisition errors
are preserved in the JSON report; a successful export with substitutions exits 4.

Existing destinations, including source segments, hard links, and symlinks,
are refused. Image control paths are also protected. A temporary file beside
the destination is flushed and installed without overwriting, even if another
process creates the destination during export. Unix also flushes the parent
directory. A directory-flush failure can report failure with `published: true`.
Use stable input segments and a stable destination directory throughout export.

Handled failures and cancellation remove the owned temporary file. A forced
termination can leave an unpublished `.ewf-export-*` file; export is not resumable.
Ctrl+C (also SIGTERM/SIGHUP on Unix) is checked between chunks and writes of at
most 1 MiB, and before publication. Opening, decoding one chunk, synchronous I/O,
and filesystem flushes must return before cancellation can complete. Buffering
retains one decoded chunk plus encoded data/decoder scratch space and the bounded
table cache, rather than the full media stream; at most 16 segment handles remain
open. Memory therefore also depends on the image's chunk size and metadata.

## Damaged-image recovery

`recover IMAGE OUTPUT_DIRECTORY` creates a new directory containing a raw image,
a JSON-lines provenance map, and a result report. It currently supports physical,
unencrypted raw/zlib EWF1 with reliable geometry and separate sectors sections.
It can use validated redundant tables and intact descriptor-chain prefixes;
it does not carve missing descriptors or recover ambiguous missing middle segments.
Logical/SMART, EWF2, encrypted, and Zstandard recovery are rejected.

Unrecoverable chunks become zeros. `--preserve-checksum-suspect` explicitly allows
decodable bytes with suspect checksums when no validated alternate exists.
`--maximum-output-bytes` rejects excessive declared media size before creating
the directory. Existing output directories/files, source aliases, and source
control paths are refused. Output files are created and published exclusively.
The destination filesystem must support hard links for publication.

During recovery, `image.raw.partial` contains emitted bytes and
`map.jsonl.partial` starts with a header identifying source paths and policy.
Every fully emitted chunk receives a record with its index, logical offset,
length, and `Primary`, `Redundant`, `SuspectPrimary`, `SuspectRedundant`, or
`ZeroFilled` status. This on-disk map is not subject to the bounded in-memory
summary's record limit. Map-write failure stops recovery. Successful completion
synchronizes both files, installs `image.raw` and `map.jsonl`, and writes
`result.json` with `recovery_complete: true` and output/map SHA256 values.
All three artifacts are required for a completed bundle. The hashes identify
the resulting artifacts; `media_verified` remains false and no authenticity or
recovery of substituted data is implied.

Cancellation exits 130 and retains partial files with a result report when the
filesystem permits. Output/map failures exit 1 and also preserve available
partial evidence. `raw_bytes_written` includes short writes; `mapped_bytes`
covers only chunks with fully written provenance records. Bytes beyond that
coverage have no completed provenance record. Prefix hashes describe bytes
accepted by each file writer. A disk-full or crash may leave a truncated final
map line; do not treat that line as a record. Neither recovery nor its map is
resumable, and source segments must remain stable throughout the operation.

Absence of `result.json`, or `recovery_complete: false`, means incomplete work,
even if publication had already installed one or both final filenames. The CLI
never infers completion from filenames alone. Report-save failure exits 1 with
`status: reporting_failed`; stdout retains the recovery result and artifact
paths. Cancellation is cooperative at chunk boundaries; opening, chunk decoding,
synchronous reads/writes and synchronization must return first. Unix synchronizes
directories; power-loss durability and Windows directory synchronization are not
certified. Keep partial bundles for inspection and choose a fresh directory to retry.

Exit 0 means recovery completed without reported recovery findings. Exit 4 means
it completed using redundant or suspect data, zero substitution, or with structural
notices. Neither status is a successful verification verdict. Detailed summaries
retain up to 1024 ranges and notices each and explicitly count omitted records;
the map still records every emitted chunk.

## Acquisition

Regular-file sources must be nonempty and sector aligned. The default
sector size is 512; `--sector-size` accepts 512, 1024, 2048, or 4096. Output is
physical E01 with raw (`--compression raw`) or default zlib compression.
`--sectors-per-chunk` and `--chunks-per-segment` set acquisition geometry.
Existing images and CLI session manifests are never overwritten.

Windows physical disks (`\\.\PhysicalDriveN`) and Linux block devices (`/dev/sdX`,
partitions, or device-mapper paths) can also be opened read-only. Geometry is
discovered; an explicit sector size must agree with it. The source must expose a
stable hardware or loop-backing identifier. Device IDs and geometry are checked before resume,
after opening, after a failed read, and after acquisition. The CLI does not
elevate privileges, lock/dismount volumes, or freeze a live filesystem. Use a
stable, appropriately write-protected source.

Windows queries the exact opened source handle for device number, size, sector
size, and device-associated storage identifiers (falling back to a device serial).
It uses native volume queries for destination disk extents; acquisition does not
require PowerShell or the Storage module.
Linux uses sysfs geometry and WWID/serial/DM UUID metadata; loop devices use the
backing file identity and mapping geometry. The opened Linux handle is checked
against the named device and kernel-reported size/sector size. Missing identifiers
or unresolved destination storage cause preflight failure. Source paths/device
numbers must remain stable across resume. Windows device checkpoints created by
the earlier PowerShell adapter use a different identity token and are rejected
by this adapter; complete those sessions with the original binary. File-source
and Linux checkpoint identities are unchanged.

Device reads use a 4096-byte-aligned bounce buffer with Linux `O_DIRECT` or
Windows `FILE_FLAG_NO_BUFFERING`. Sector-sized retries bypass buffered block
reads that can spread a single bad-sector error to neighboring sectors.

Device acquisition rejects destinations on the source disk. Linux also checks
partition parents, stacked-device slaves, and loop backing-file aliases. A loop
source can share a host filesystem with a separate output file; it does not
represent every sector of that host disk. Windows checks every disk extent
reported for the destination volume; hidden controller, SAN, and virtual-storage
relationships are outside that check. Network destinations and
unresolved storage layouts are unsupported for device acquisition. File sources
can use any destination supported by the writer.

See [device acceptance](device-acceptance.md) for Linux loop/device-mapper and
Windows VHDX validation. Physical hot-unplug, hardware write blockers, and
power-loss behavior require separate acceptance.

`acquire` automatically reopens the published image, decodes all media, and
compares its hashes with embedded references and the acquisition SHA256.
`verify` performs a fresh read using the same library, not an independent
implementation; libewf interoperability is tested separately. Successful hashes
cover zero substitutions too and do not establish recovery of unreadable data.

## One-shot EWF2 acquisition and collection

```text
ewf-image acquire-sequential source.raw case.Ex01 --compression zlib --chunks-per-segment 1024
ewf-image collect snapshot-directory case.Lx01 --case-number CASE-001
ewf-image recover-publication case.Ex01
```

`acquire-sequential` uses the existing source identity and device overlap checks,
then streams known-size input into Ex01 with bounded payload scratch. Sources
must be nonempty and sector-aligned, as with E01 acquisition; a regular file's
sector size defaults to 512. Source reads support cancellation and optional
`--read-timeout-ms`, with no retries or zero substitution. Chunks are fixed at
32 KiB; the default 1024 chunks per segment represents 32 MiB of raw capacity,
not an encoded segment-size limit. `raw` and `zlib` are available. The CLI
requires `.Ex01` for physical output and `.Lx01` for logical output.

`collect` inventories regular files and directories, records available basic
timestamps and Unicode names, streams each file into a logical catalog, and
verifies both the full media and every file after publication. It rejects links,
Windows reparse points, special files, inaccessible entries, delimiter-containing
names, and outputs within the source tree. Discovery is limited to 100,000
entries including the root and 127 directory levels. Inventory, opened handles
and named paths are checked for metadata changes; use a stable snapshot because
these checks do not guarantee cross-file consistency or defeat every concurrent
filesystem substitution. ADS, xattrs, ACLs, and sparse allocation are not captured.
Collection cancellation is checked between buffers; blocking filesystem calls
do not have a deadline. Collection metadata grows with entry count.

Both commands create new outputs only, support case/evidence/examiner metadata,
and report `resumable: false`. They use publication journals but do not create
E01 checkpoint/history sessions, and have no mirror option. Source cancellation
or a handled pre-publication failure removes staging. A finish error can leave
the publication decision unresolved: JSON then has `published: null`,
`publication_state: "unresolved"`, and a recovery command. After process death,
run `recover-publication OUTPUT` to resolve/discard the transaction, then `verify`
any retained output before use. Recovery itself does not verify evidence and
does not continue the original source acquisition. A verification failure after
publication retains the output and reports `published: true` with exit 3;
cancellation returns 130. Successful acquisition and verification return 0.

Fixed 32 KiB geometry is the independently validated CLI profile. Other library
geometries retain their existing behavior, with no broader consumer-compatibility
claim; tiny-chunk split logical output is not certified against libewf.

## Cancellation and resume

Ctrl+C requests cancellation. On Unix, SIGTERM and SIGHUP use the same handler.
Source seeks/reads run in one dedicated worker with owned buffers. The acquisition
thread checks its stop flag while waiting (every 20 ms), discards any late result,
and checkpoints complete accepted chunks. Windows also requests cancellation of
the worker's synchronous I/O. The stopped worker cannot write output or receive
another read request; it retains its handle and buffer until the OS call returns.

This does not bound total shutdown time: opening and identifying a source,
identity checks after media errors, destination writes/flushes, segment sealing,
and verification still use synchronous operations. Drivers may ignore native
cancellation, and OS process teardown may wait for outstanding I/O. Linux does
not forcibly cancel the worker's kernel read. The library's caller-supplied
`Read + Seek` API retains cooperative cancellation between operations.
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
configuration/disconnection errors and timeouts remain fatal. `--checkpoint-interval BYTES` must be a
positive chunk-size multiple within the segment namespace limit. Read policies
can change on resume and apply only to future reads.

`--read-timeout-ms N` sets a positive deadline for each worker request (a seek
followed by a read of at most 16 KiB), including worker scheduling time. It applies
to files and devices; there is no deadline by default. Expiry stops acquisition
with exit 1, without retries or zero substitution, and preserves the last valid
checkpoint. Supply the desired timeout again on `resume`; it is a per-invocation
read policy, not part of source identity. Cancellation uses exit 130. A stop or
deadline observed before a completed read is accepted takes precedence over it.
The report includes `read_policy.read_timeout_ms`; a worker stop additionally
sets `source_read_stop` to `timeout` or `cancelled`.

## Results

Acquisition and resume automatically keep `.case.E01.ewf-history/` and write
`.case.E01.ewf-report.json`. Preserve these alongside the session manifest and
image. History contains a session binding and immutable, sequential JSON records
for each run's start time (Unix milliseconds), tool version, read policy, phases,
checkpoints, read-error kinds and offsets, substituted ranges, publication, and
closing result. Closing results include hashes and verification when available.
Read-error records describe attempts; an error does not itself imply substitution.
The log stores error kinds, not every driver's diagnostic message; a fatal error's
message is included in the closing result. Failures before a new session can be
established are reported only on stdout.

Each record is written to a temporary file, flushed, and installed exclusively.
Unix also flushes directories; this does not certify Windows power-loss durability.
An unfinished temporary record is retained and reported as pending. A run without
a closing record is `interrupted`, even if its last recorded phase was publication.
Malformed, truncated committed records, gaps, and session mismatches are rejected
without modifying image checkpoints. History is not authenticated and cannot prove
that records were never edited or removed. Older sessions resumed without history
explicitly report `prior_history_unavailable`.

The consolidated report lists all runs and their individual results, with a
`latest_run` convenience field. Read/retry counters sum the recorded attempts
across runs, including repeated reads after resume; they do not represent unique
sectors. `counters_complete: false` identifies missing prior history, interrupted
runs, or pending records. Accepted bytes can include an unsealed tail; only
checkpoint bytes are resumable. Per-run substitution totals are cumulative and
must not be summed across runs.

`report` rebuilds this summary in memory without opening the source or verifying
the image. `report --write` also atomically refreshes the saved report under the
session lock. A stale or missing report can therefore be regenerated after a
crash. It refuses to replace an unrelated file. Its `recorded_only: true` field
distinguishes logged verification from a fresh `verify` operation. Standalone
`verify` does not append to acquisition history.

If logging fails, acquisition stops through checkpoint handling; a destination
failure can still prevent a new checkpoint, leaving the last valid one usable.
Exit 1 with `status: reporting_failed` reports persistence failure and preserves
the acquisition status, exit code, and publication/verification fields separately.
In particular, a failed report write does not undo a successfully verified image.
Free disk space before resuming or regenerating the report. History is bounded
to 100,000 records and 64 MiB of committed records; each record and saved report
is limited to 16 MiB. Reaching a limit stops logging rather than dropping events.

Progress is written to stderr at most once per second; `--quiet` suppresses it.
One JSON object is written to stdout on completion, cancellation, or operational
failure. Argument errors and help follow normal command-line conventions.
The report has `schema_version: 1`; additive fields may appear in that version.
It includes status, phase, elapsed time, accepted/checkpointed bytes when known,
publication status, acquisition-error ranges, and verification results.
Acquisition error ranges on a paused/failed run describe accepted input, which
may include an unsealed tail; checkpoint inspection reports only sealed ranges.
`published: false` means this invocation has not confirmed publication; a failed
finish may still need recovery to resolve publication state.

| Exit | Meaning |
| --- | --- |
| 0 | Operation succeeded; analysis/recovery reported no findings |
| 1 | Operational failure or result-output failure |
| 2 | Invalid command-line arguments |
| 3 | Verification/extraction mismatch or unreadable selected file, export digest mismatch, or analysis errors |
| 4 | Substitutions, missing logical-file references, analysis warnings, or recovery findings; inspect the command result |
| 130 | Cancelled, including a requested `--stop-after` pause |

For example, `ewf-image acquire disk.raw case.E01 > result.json` retains the
report. Choose a new report path outside the source and image set: the shell
opens redirections before the program can check them. Manifests and reports can
contain case metadata and local source paths; handle them with the evidence.

Platform references: [Windows storage properties](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-ioctl_storage_query_property),
[volume disk extents](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-ioctl_volume_get_volume_disk_extents),
and [Linux sysfs block ABI](https://www.kernel.org/doc/Documentation/ABI/testing/sysfs-block).

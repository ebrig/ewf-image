# EWF command line

This page documents the advanced EWF commands under `ewf-cli ewf`. For the
standard acquisition, conversion, and verification workflow, see the
[unified CLI guide](ewf-cli.md), which also explains how to build `ewf-cli`.

Commands print concise text summaries. Add `--json` for machine-readable results.
Progress is written to stderr, and `-q` or `--quiet` hides it. Run
`<command> --help` to list options. Read [results and exit codes](#results)
before automating decisions based on command output. For supported encrypted
X-Ways EWF1 input, pass `--password-file PATH` to an image-reading command.
Use `-` as PATH to read from stdin. The input is 1 to 32 password bytes; one
trailing LF or CRLF is removed. The password is never placed in CLI arguments
or the result report. Protect a password file with host filesystem permissions.

| Task | Commands |
| --- | --- |
| Inspect or check an image | `info`, `verify`, `analyze` |
| Export decoded media | `export` |
| Browse or extract logical files | `files`, `verify IMAGE ENTRY`, `extract` |
| Acquire/resume physical E01 | `acquire`, `resume`, `checkpoint inspect`, `checkpoint validate` |
| Create one-shot EWF2 output | `acquire-sequential`, `collect` |
| Resolve EWF2 publication | `recover-publication` |
| Export damaged EWF1 with provenance | `recover` |
| Read saved E01 acquisition history | `report` |

```text
ewf-cli ewf acquire disk.raw case.E01 --case-number CASE-123 --examiner "A. Examiner"
ewf-cli ewf resume case.E01
ewf-cli ewf checkpoint inspect case.E01
ewf-cli ewf checkpoint validate case.E01
ewf-cli ewf verify case.E01
ewf-cli ewf info case.E01
ewf-cli ewf files case.L01 --limit 1000
ewf-cli ewf verify case.L01 1
ewf-cli ewf extract case.L01 1 selected.bin
ewf-cli ewf --password-file password.txt verify encrypted.E01
ewf-cli ewf analyze case.E01 --maximum-findings 1024
ewf-cli ewf recover damaged.E01 recovered-case --maximum-output-bytes 107374182400
ewf-cli ewf export case.E01 disk.raw
ewf-cli ewf report case.E01
ewf-cli ewf report case.E01 --write
```

## Logical file operations

Use `verify IMAGE ENTRY` and `extract IMAGE ENTRY OUTPUT` for logical files. The
older `verify-file` and `extract-file` commands remain accepted. Scripts must add
`--json`, because redirecting stdout does not select JSON output.

`files IMAGE` lists L01 and Lx01 catalog entries in preorder. The root has index
0. Each entry includes `parent_index`, which is null for the root, so the
hierarchy can be rebuilt across pages. Use `--offset` and `--limit` to page
through the catalog. `next_offset` is null on the last page. An index selects an
entry even when names collide or contain characters that are invalid in
destination filenames. Indices are stable only for the same unchanged image.
Listing does not verify file data.

`verify IMAGE ENTRY` strictly reads the selected regular file, computes MD5, SHA1,
and SHA256, and compares the available file MD5 and SHA1 references. Container
hashes are not file references. When the file has no stored hashes, the command
reports `file_hashes_missing` and exits with code 4. Mismatches, invalid
references, and unreadable data exit with code 3. Directories and unknown entry
types cannot be verified as regular files.

`extract IMAGE ENTRY OUTPUT` streams the file into a temporary file beside
OUTPUT, checks the stored file hashes, synchronizes the file, and publishes it
without overwriting an existing destination. Cancellation, corruption, and hash
mismatches leave no final output. When stored references are missing, extraction
still completes but reports `extracted_without_reference` and exits with code 4.
The report includes computed digests for independent comparison. Extraction with
matching references exits with code 0.

The caller chooses OUTPUT, and catalog names are never used as destination
paths. Extraction copies content and can restore recorded access and modification
times with `--restore-times`. The `restored_times` result lists applied fields.
ACLs, links, extended attributes, and alternate streams are not restored.
`verify IMAGE ENTRY` and
`extract` bypass decoded caches and zero-on-error recovery. Both commands check
only the selected file, not the container media, and report `media_verified: false`.

## Metadata inspection

`info IMAGE` reports the format and profile, segment paths and sizes, media
geometry, case metadata, stored hashes, encryption detection, acquisition error
ranges, and whether a logical file catalog is present. `info` applies strict
structural checks when opening the image but does not scan media bytes. A
successful inspection therefore reports `media_verified: false` and
`verification: null`. Use `verify` for a full media check. Legacy password
headers and raw metadata sections are omitted.

Without `--password-file`, encrypted metadata cannot be opened: `info` exits
with code 1 and reports the encryption flag and the open error. Encrypted EWF2
remains unsupported.

## Integrity analysis

`analyze IMAGE` scans media and compares redundant tables, continuing after
individual chunk failures. The `analysis` object reports whether coverage is
complete, incomplete, or unavailable, along with findings, counts, and reference
comparisons. An incomplete scan never reports whole-media hashes. A structural
open failure becomes a single finding with unavailable coverage. Analysis does
not carve a broken descriptor chain.

`--maximum-findings` limits the number of retained findings. The default is 1024
and the maximum is 100,000. Total and suppressed counts still cover the entire
scan. Missing reference hashes and recorded acquisition errors are reported as
warnings.

`analyze` exits with code 3 for error findings, code 4 for warnings only, and
code 0 for no findings. Filesystem and operational errors exit with code 1.
Cancellation exits with code 130 and reports progress but no completed analysis.
Opening the image and inspecting redundant tables happen before the cancellable
media scan begins.

## Strict raw export

`export IMAGE OUTPUT` copies the complete decoded media stream into a new raw
file. The command supports every unencrypted physical, SMART, and logical format
the reader supports. For logical images, the output is the flat media stream, not
the individual files. Incomplete acquisitions are rejected. Damaged chunks are
never silently zero-filled. Use `recover` for explicit damaged-image recovery.

`export` computes MD5, SHA1, and SHA256 while writing and compares them with every
supported stored digest. A mismatch exits with code 3 and does not publish the
destination. When the image has no reference digests, export can succeed, but
`references_match` is null and `media_verified` remains false. The hashes
describe the decoded stream passed to the output writer. The raw file is not
reread. Recorded acquisition errors are included in the JSON report, and a
successful export that contains substituted data exits with code 4.

Existing destinations are refused, including source segments, hard links, and
symbolic links. Image control paths are also protected. Output is written to a
temporary file beside the destination, flushed, and installed without
overwriting, even if another process creates the destination during export. On
Unix, the parent directory is also flushed. A failed directory flush can report
failure with `published: true`. Keep the input segments and the destination
directory unchanged throughout the export.

Handled failures and cancellation remove the temporary file. A forced termination
can leave an unpublished `.ewf-export-*` file. Export cannot be resumed.

Ctrl+C requests cancellation, as do SIGTERM and SIGHUP on Unix. Cancellation is
checked between chunks, between writes of at most 1 MiB, and before publication.
Opening the image, decoding one chunk, synchronous I/O, and filesystem flushes
must finish before cancellation completes.

Export buffers one decoded chunk, its encoded data, decoder scratch space, and
the bounded table cache. The full media stream is never buffered. At most 16
segment handles remain open. Memory use also depends on the image's chunk size
and metadata.

## Damaged-image recovery

`recover IMAGE OUTPUT_DIRECTORY` creates a new directory that contains a raw
image, a JSON-lines provenance map, and a result report. Recovery supports
physical, unencrypted raw or zlib EWF1 images with reliable geometry and
separate sectors sections. Recovery can use validated redundant tables and
intact prefixes of a descriptor chain. Recovery does not carve missing
descriptors or recover ambiguous missing middle segments. Logical, SMART, EWF2,
encrypted, and Zstandard images are rejected.

Unrecoverable chunks are replaced with zeros. `--preserve-checksum-suspect`
allows decodable bytes with suspect checksums when no validated alternative
exists. `--maximum-output-bytes` rejects an excessive declared media size before
the directory is created. Existing output directories and files, source aliases,
and source control paths are refused. Output files are created and published
exclusively. Publication requires a destination filesystem that supports hard links.

During recovery, `image.raw.partial` contains the emitted bytes, and
`map.jsonl.partial` begins with a header that identifies the source paths and
policy. Each fully emitted chunk receives a map record with its index, logical
offset, length, and status. The status is `Primary`, `Redundant`,
`SuspectPrimary`, `SuspectRedundant`, or `ZeroFilled`. The on-disk map records
every chunk and is not subject to the record limit of the in-memory summary. A
failure to write the map stops recovery.

On successful completion, recovery synchronizes both files, installs `image.raw`
and `map.jsonl`, and writes `result.json` with `recovery_complete: true` and the
SHA256 values of the output and the map. A completed bundle requires all three
files. The hashes identify the resulting files only. `media_verified` remains
false, and the bundle makes no claim about authenticity or about recovery of
substituted data.

Cancellation exits with code 130. When the filesystem permits, cancellation
keeps the partial files and writes a result report. Output and map failures exit
with code 1 and also keep the available partial files. `raw_bytes_written`
includes short writes. `mapped_bytes` covers only chunks with fully written
provenance records, so bytes beyond that point have no provenance record. Prefix
hashes describe the bytes accepted by each file writer. A full disk or a crash
can leave a truncated final map line, which must not be treated as a record.
Recovery and its map cannot be resumed. The source segments must remain unchanged
throughout recovery.

A missing `result.json` or `recovery_complete: false` means the recovery is
incomplete, even if one or both final filenames were already installed. The CLI
never infers completion from filenames alone. A failure to save the report exits
with code 1 and `status: reporting_failed`, and stdout still contains the
recovery result and file paths.

Cancellation is checked at chunk boundaries. Opening the image, decoding a chunk,
synchronous reads and writes, and synchronization must finish first. Directories
are synchronized on Unix. Power-loss durability and Windows directory
synchronization are not certified. Keep partial bundles for inspection, and use
a new directory to retry.

Exit code 0 means recovery completed without findings. Exit code 4 means recovery
completed but used redundant or suspect data, substituted zeros, or reported
structural notices. Neither code is a verification verdict. Detailed summaries
keep up to 1024 ranges and 1024 notices and count any omitted records. The map
still records every emitted chunk.

## Acquisition

Regular-file sources must be nonempty and sector-aligned. The default sector size
is 512 bytes, and `--sector-size` accepts 512, 1024, 2048, or 4096. Output is
physical E01 with zlib compression by default, or raw with `--compression raw`.
`--sectors-per-chunk` and `--chunks-per-segment` set the acquisition geometry.
Existing images and CLI session manifests are never overwritten.

The CLI can also open Windows physical disks (`\\.\PhysicalDriveN`) and Linux
block devices (`/dev/sdX`, partitions, or device-mapper paths) read-only. Device
geometry is discovered automatically, and an explicit sector size must agree with
it. The source must expose a stable hardware identifier or loop-backing
identifier. Device IDs and geometry are checked before resume, after opening,
after a failed read, and after acquisition. The CLI does not elevate privileges,
lock or dismount volumes, or freeze a live filesystem. Use a stable source that
is appropriately write-protected.

On Windows, the CLI queries the opened source handle for the device number, size,
sector size, and storage identifiers, falling back to the device serial number.
Native volume queries supply the destination's disk extents. Acquisition does not
require PowerShell or the Storage module.

On Linux, the CLI reads geometry and WWID, serial, or DM UUID metadata from sysfs.
Loop devices use the backing file identity and mapping geometry. The opened
handle is checked against the named device and the size and sector size reported
by the kernel. Missing identifiers or unresolved destination storage cause the
preflight check to fail. Source paths and device numbers must remain the same
across resume.

Windows device checkpoints created by the earlier PowerShell adapter use a
different identity token, and the native adapter rejects them. Complete those
sessions with the original binary. File-source and Linux checkpoint identities
are unchanged.

Device reads use a 4096-byte-aligned bounce buffer with Linux `O_DIRECT` or
Windows `FILE_FLAG_NO_BUFFERING`. Sector-sized retries bypass buffered block
reads, which can spread a single bad-sector error to neighboring sectors.

Device acquisition rejects destinations on the source disk. On Linux, the check
also covers partition parents, stacked-device slaves, and loop backing-file
aliases. A loop source can share a host filesystem with a separate output file,
because the loop source does not represent every sector of the host disk. On
Windows, the check covers every disk extent reported for the destination volume.
Controller, SAN, and virtual-storage relationships that Windows does not report
are outside the check. Network destinations and unresolved storage layouts are
unsupported for device acquisition. File sources can use any destination the
writer supports.

See [device acceptance](device-acceptance.md) for Linux loop and device-mapper
validation and Windows VHDX validation. Physical hot-unplug, hardware write
blockers, and power-loss behavior require separate acceptance testing.

After publication, `acquire` reopens the image, decodes all media, and compares
the hashes with the embedded references and the acquisition SHA256. `verify`
performs a fresh read with the same library, not with an independent
implementation. Interoperability with libewf is tested separately. Successful
hashes also cover substituted zeros and do not show that unreadable data was
recovered.

## One-shot EWF2 acquisition and collection

```text
ewf-cli ewf acquire-sequential source.raw case.Ex01 --compression zlib --chunks-per-segment 1024
ewf-cli ewf collect snapshot-directory case.Lx01 --case-number CASE-001
ewf-cli ewf recover-publication case.Ex01
```

`acquire-sequential` applies the source identity and device-overlap checks
described above, then streams known-size input into Ex01 with bounded payload
scratch space. As with E01 acquisition, sources must be nonempty and
sector-aligned, and the sector size of a regular file defaults to 512 bytes.
Source reads support cancellation and an optional `--read-timeout-ms`. Failed
reads are not retried or replaced with zeros. Chunks are fixed at 32 KiB. The
default of 1024 chunks per segment represents 32 MiB of raw capacity, not a limit
on encoded segment size. The `raw` and `zlib` compression options are available.
The CLI requires the `.Ex01` extension for physical output and `.Lx01` for
logical output.

`collect` inventories regular files and directories, records available basic
timestamps and Unicode names, and streams each file into a logical catalog. After
publication, `collect` verifies the full media and every file. `collect` rejects
links, Windows reparse points, special files, inaccessible entries, names that
contain catalog delimiters, and outputs inside the source tree. Discovery is
limited to 100,000 entries, including the root, and 127 directory levels.

The inventory, opened handles, and named paths are checked for metadata changes.
These checks do not guarantee consistency across files or prevent every
concurrent filesystem substitution, so collect from a stable snapshot. ADS,
xattrs, ACLs, and sparse allocation are not captured. Cancellation is checked
between buffers, and blocking filesystem calls have no deadline. Collection
metadata grows with the number of entries.

`acquire-sequential` and `collect` create new outputs only, accept case,
evidence, and examiner metadata, and report `resumable: false`. Both commands use
publication journals but do not create E01 checkpoint or history sessions, and
neither has a mirror option. Source cancellation or a handled failure before
publication removes the staged files.

A finish error can leave the publication decision unresolved. In that case the
JSON result contains `published: null`, `publication_state: "unresolved"`, and a
recovery command. If the process ends unexpectedly, run
`recover-publication OUTPUT` to resolve or discard the transaction, then run
`verify` on any retained output before use. `recover-publication` does not verify
evidence and does not continue the original acquisition.

A verification failure after publication keeps the output, reports
`published: true`, and exits with code 3. Cancellation exits with code 130.
Successful acquisition and verification exit with code 0.

The fixed 32 KiB geometry is the CLI profile validated with independent tools.
The library's EWF2 logical writer requires chunks of at least 8 KiB. Split
logical output at 8 KiB also passed the pinned libewf 20260924 exporter; older
consumer behavior remains under investigation.

## Cancellation and resume

Ctrl+C requests cancellation. On Unix, SIGTERM and SIGHUP are handled the same
way. Source seeks and reads run in one dedicated worker thread with its own
buffers. The acquisition thread checks its stop flag every 20 ms while waiting,
discards any result that arrives late, and checkpoints the complete accepted
chunks. On Windows, the CLI also requests cancellation of the worker's
synchronous I/O. A stopped worker cannot write output or receive another read
request. The worker keeps its handle and buffer until the operating-system call
returns.

The stop mechanism does not limit total shutdown time. Opening and identifying a
source, identity checks after media errors, destination writes and flushes,
segment sealing, and verification are synchronous. Drivers may ignore native
cancellation, and process teardown may wait for outstanding I/O. Linux does not
forcibly cancel the worker's kernel read. The library's `Read + Seek` API, which
takes a caller-supplied source, supports cooperative cancellation between
operations.

`--stop-after BYTES` pauses acquisition at an absolute offset of accepted bytes,
rounded to chunk granularity. The option supports scheduled acquisition windows
and reproducible recovery testing. On resume, supply a larger offset, or omit the
option to finish.

Preserve both `.case.E01.ewf-session.json` and the entire
`.case.E01.ewf-acquisition` directory. The immutable, versioned JSON manifest
stores the source identity, geometry, and metadata, and `resume` restores these
options. A persistent `.case.E01.ewf-cli.lock` file coordinates CLI operations in
addition to the library's output lock. Do not remove control files while a
command is running.

File identity is based on the canonical path, size, modification and creation
metadata, and, where available, the Unix inode, device, and change metadata. File
identity is not a content digest and cannot prove that a source is unchanged when
metadata has been deliberately preserved. Use a stable source snapshot. Identity
is checked before resume and after the source has been read, and changed sources
are rejected. Checkpoint inspection and validation do not require the source.

`resume` can restart an initialization that was interrupted after only the
manifest was written. If publication has already retired the journal and
verification was interrupted, run `verify`. `resume` does not assume that an
existing image belongs to the session. [Acquisition](acquisition.md) describes
filesystem and power-loss limitations.

## Read policy

`--retries N` sets the number of additional attempts for each failed sector, from
0 through 100. The default is 2. By default, acquisition stops at an unreadable
sector. `--zero-fill` enables zero substitution and native bad-sector records.
EOF, permission, seek, configuration, and disconnection errors are always fatal,
as are timeouts. `--checkpoint-interval BYTES` must be a positive multiple of the
chunk size within the segment namespace limit. Read policies can change on resume
and apply only to subsequent reads.

`--read-timeout-ms N` sets a positive deadline for each worker request, which is
a seek followed by a read of at most 16 KiB. The deadline includes worker
scheduling time and applies to both files and devices. There is no deadline by
default. When the deadline expires, acquisition stops with exit code 1, without
retries or zero substitution, and preserves the last valid checkpoint. The
timeout is a per-invocation read policy, not part of the source identity, so
supply it again when running `resume`. Cancellation exits with code 130.

A stop or deadline that is observed before a completed read is accepted takes
precedence over that read. The report includes `read_policy.read_timeout_ms`.
When the worker stops, the report also sets `source_read_stop` to `timeout` or
`cancelled`.

## Results

Acquisition and resume maintain a `.case.E01.ewf-history/` directory and write
`.case.E01.ewf-report.json`. Preserve both with the session manifest and the
image. The history contains a session binding and immutable, sequential JSON
records. Each run records its start time (Unix milliseconds), tool version, read
policy, phases, checkpoints, read-error kinds and offsets, substituted ranges,
publication, and closing result. Closing results include hashes and verification
results when available.

Read-error records describe read attempts. A read error does not by itself imply
substitution. The history stores error kinds rather than every driver's
diagnostic message. The message for a fatal error is included in the closing
result. Failures that occur before a session is established are reported only
on stdout.

Each history record is written to a temporary file, flushed, and installed
exclusively. Directories are also flushed on Unix. Windows power-loss durability
is not certified. An unfinished temporary record is kept and reported as
pending. A run without a closing record is reported as `interrupted`, even if its
last recorded phase was publication. Malformed or truncated committed records,
gaps, and session mismatches are rejected without modifying image checkpoints.
The history is not authenticated and cannot prove that records were never edited
or removed. Older sessions resumed without history report
`prior_history_unavailable`.

The consolidated report lists every run and its result, and a `latest_run` field
identifies the most recent run. Read and retry counters sum the recorded
attempts across runs, including repeated reads after resume. The counters do not
count unique sectors. `counters_complete: false` indicates missing prior history,
interrupted runs, or pending records. Accepted bytes can include an unsealed
tail, but only checkpointed bytes can be resumed. Per-run substitution totals are
cumulative and must not be summed across runs.

`report` rebuilds the summary in memory without opening the source or verifying
the image. `report --write` also replaces the saved report atomically while
holding the session lock, so a stale or missing report can be regenerated after
a crash. `report --write` refuses to replace an unrelated file. The
`recorded_only: true` field distinguishes logged verification results from a
fresh `verify` run. Standalone `verify` does not add records to the acquisition
history.

If logging fails, acquisition stops through checkpoint handling. A destination
failure can prevent a new checkpoint, but the last valid checkpoint remains
usable. A persistence failure exits with code 1 and `status: reporting_failed`,
and the acquisition status, exit code, and publication and verification fields
are reported separately. A failed report write does not undo a successfully
verified image. Free disk space before resuming or regenerating the report.

History is limited to 100,000 records and 64 MiB of committed records. Each
record and the saved report are limited to 16 MiB. When a limit is reached,
logging stops rather than dropping events.

Progress is written to stderr at most once per second, and `--quiet` suppresses
it. With `--json`, one JSON object is written to stdout on completion,
cancellation, or operational failure. Text summaries show the outcome,
verification scope, and relevant failures. Detailed findings and history are
available only in JSON. Argument errors and help output follow normal
command-line conventions.

The JSON report has `schema_version: 1`, and later releases may add fields
within that version. The report includes the status, phase, elapsed time,
accepted and checkpointed bytes when known, publication status,
acquisition-error ranges, and verification results. For a paused or failed run,
acquisition-error ranges describe accepted input, which may include an unsealed
tail. Checkpoint inspection reports only sealed ranges. `published: false` means
the current invocation has not confirmed publication. A failed finish may still
require recovery to resolve the publication state.

| Exit | Meaning |
| --- | --- |
| 0 | Operation succeeded; analysis/recovery reported no findings |
| 1 | Operational failure or result-output failure |
| 2 | Invalid command-line arguments |
| 3 | Verification/extraction mismatch or unreadable selected file, export digest mismatch, or analysis errors |
| 4 | Substitutions, missing logical-file references, analysis warnings, or recovery findings; inspect the command result |
| 130 | Cancelled, including a requested `--stop-after` pause |

For example, `ewf-cli ewf --json acquire disk.raw case.E01 > result.json` saves the
report. Choose a new report path outside the source and the image set, because
the shell opens redirections before the program can check them. Manifests and
reports can contain case metadata and local source paths. Handle them with the
same care as the evidence.

Platform references: [Windows storage properties](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-ioctl_storage_query_property),
[volume disk extents](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-ioctl_volume_get_volume_disk_extents),
and [Linux sysfs block ABI](https://www.kernel.org/doc/Documentation/ABI/testing/sysfs-block).

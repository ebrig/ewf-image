# Acquisition and writing

Choose a writer according to input access, output family, and resume needs.
The CLI uses these APIs with additional source checks and JSON reporting; see
[CLI acquisition](cli.md#acquisition).

## Choose a writer

| API | Use when | Main constraint |
| --- | --- | --- |
| `EwfWriter` | You need seek/patch, encoded chunks, or general EWF1/EWF2 authoring | Full-source spooling; EWF1 resume rewrites output |
| `AcquisitionWriter` | You need resumable physical E01 acquisition | Known aligned size, raw/zlib, one destination, hard links |
| `SequentialWriter` | You need append-only Ex01/Lx01 with bounded payload scratch | Known size; no checkpoint resume |
| `LogicalWriter` | You are supplying files and metadata for L01/Lx01 | Retained catalog; general or sequential EWF2 backend |

Library finalization computes hashes but does not independently reread output.
EWF CLI acquisition/collection verifies after publication. AFF4 is a separate
[sibling crate](https://github.com/ebrig/ewf-image/tree/main/crates/aff4-image)
with its own single-volume writer and opt-in verification before publication.

## Resumable E01 acquisition

`AcquisitionWriter` creates physical E01 images from an append-only source with
a known, nonzero, sector-aligned size. It supports raw and zlib compression,
one destination, and MD5/SHA1/SHA256 media digests. `EwfWriter` remains available
for positioned writes, other container families, mirroring, and rewrite-based
resume.

```rust,no_run
use ewf_image::{AcquisitionOptions, AcquisitionWriter};
use std::fs::File;

fn main() -> ewf_image::Result<()> {
    let mut source = File::open("disk.raw")?;
    let options = AcquisitionOptions::new(source.metadata()?.len());
    // Obtain this from the acquisition application's stable source identity.
    // It must change if the source snapshot changes; this is not a source hash check.
    let identity = [0x51; 32];
    let mut writer = AcquisitionWriter::create("case.E01", &options, identity)?;
    std::io::copy(&mut source, &mut writer)?;
    writer.finish()?;

    Ok(())
}
```

After an interruption, reopen the source and reconstruct the same `options` and
`identity`. In the acquisition application, continue from the sealed offset:

```rust,ignore
// Continuation: source, options, and identity come from the acquisition session.
use std::io::{Seek, SeekFrom};
let mut writer = AcquisitionWriter::resume("case.E01", &options, identity)?;
source.seek(SeekFrom::Start(writer.checkpoint_offset()))?;
std::io::copy(&mut source, &mut writer)?;
writer.finish()?;
```

## Source reads, progress, and cancellation

For a caller-opened file or seekable device handle, use `acquire_from` or
`acquire_with_progress`. These methods seek to the writer's accepted offset on
every read attempt, including after resume; callers do not need to position the
source themselves. Device opening, privileges, source-size discovery, and stable
source identification remain the application's responsibility.

```rust,ignore
// Continuation: writer and source are the open acquisition handles.
use std::ops::ControlFlow;
use ewf_image::{AcquisitionReadOptions, AcquisitionStatus, UnreadableSectorPolicy};

let read_options = AcquisitionReadOptions {
    retries: 2,
    unreadable_sector_policy: UnreadableSectorPolicy::Stop,
    checkpoint_interval: Some(64 * 1024 * 1024),
    ..AcquisitionReadOptions::default()
};
let outcome = writer.acquire_with_progress(&mut source, &read_options, |progress| {
    eprintln!("{} accepted; {} checkpointed; {} substituted sectors",
        progress.bytes_written, progress.checkpoint_bytes,
        progress.substituted_sectors);
    // Return ControlFlow::Break(()) when the application's stop flag is set.
    ControlFlow::Continue(())
})?;
if outcome.status == AcquisitionStatus::Complete {
    writer.finish()?;
}
```

Normal reads are chunk-sized. A failed bulk read is discarded, then retried one
sector at a time to isolate damaged sectors. `retries` is the number of additional
attempts per sector (0 through 100); the initial bulk failure does not consume that
allowance. Short successful reads are completed before any bytes from the attempt
enter the image. Progress includes read/retry counts for the current call, the
current read offset and error kind, total accepted/checkpointed bytes, and the
cumulative substituted-sector count.

The default policy stops on an unreadable sector. `ZeroFill` must be selected
explicitly: it substitutes exactly one sector and records its location. EOF,
failed seeks, permission/configuration failures, timeouts, and interrupted or nonblocking
reads always stop; these conditions are never padded to the declared source
size. Error-range storage is bounded by `maximum_error_ranges` (65,536 by
default); reaching the limit stops before another disjoint substitution. Adjacent
substitutions share a range. EWF1 error tables use 32-bit sector addresses;
substitution stops if a failed sector cannot be represented.

`Complete` means the declared output range is filled, possibly with substitutions.
It does not mean every source sector was successfully read. `acquisition_errors()`
exposes those ranges, including accepted but unsealed substitutions. Each sealed
segment carries a cumulative native EWF `error2` table; the final table describes
all substituted sectors and is compatible with libewf. Resume restores only the
sealed ranges. Media hashes describe the bytes actually written, including zeros;
a successful hash verification does not prove that substituted source data was
recovered. The library does not persist per-attempt diagnostics or retry counts.
The optional CLI records read-error kinds, offsets, retry counters, and run results
in separate [acquisition history](cli.md#results); those records are not EWF metadata.

A callback returning `Break(())` stops between source I/O operations, checkpoints
complete accepted chunks, and returns `Cancelled`. A partial chunk remains in the
live writer; continue with it, or drop it and resume from `checkpoint_offset()`.
Source failures similarly checkpoint full chunks and leave the writer usable.
Destination failures poison it. Callbacks run on the calling thread; an in-flight
OS read, seek, or native segment seal cannot be interrupted by the callback.
Applications can check an atomic stop flag or forward progress to their own UI.

`checkpoint_interval` can force earlier seals. It must be a positive multiple of
the chunk size and must fit the native segment-number namespace. The interval is
measured in accepted bytes since the last checkpoint; regular segment boundaries
still apply. Changing read policy after resume affects only new input.

## Inspecting and validating a checkpoint

Close the writer before calling these functions so inspection can acquire the
output lock. Supply the original acquisition options and source identity:

```rust,ignore
// Supply the original acquisition options and identity.
let checkpoint = AcquisitionWriter::inspect_checkpoint("case.E01", &options, identity)?;
// Metadata-only: records, segment lengths, geometry, and recorded bad-sector ranges.
assert!(!checkpoint.segment_hashes_validated);
let checkpoint = AcquisitionWriter::validate_checkpoint(
    "case.E01", &options, identity, |_| std::ops::ControlFlow::Continue(())
)?;
assert!(checkpoint.segment_hashes_validated);
```

Inspection reports the resumable offset, sealed count/size, readiness to finish,
whether publication started, and normalized substituted-sector ranges. It never
cleans scratch, rewrites checkpoints, reads the source, or publishes output.
Metadata-only inspection does not scan media payloads or certify their contents.
Validation additionally compares each entire sealed file with its checkpointed
SHA256; it does not decode and rehash logical media.

`resume_with_progress` reports container validation and logical-media rehash
phases. `finish_with_progress` reports validation and publication. Their callbacks
receive `AcquisitionOperationProgress`; byte units and totals belong to the
reported phase. Returning `Break(())` returns `EwfError::Aborted` and leaves the
journal resumable. Even cancellation after output links have been installed is
recoverable through resume and finish. Journal retirement is the publication
commit point; final cleanup after that point is not cancellable.

## Checkpoints and resource use

Writes encode one chunk at a time into a segment-sized scratch file. A full
segment is sealed as native EWF data, synchronized, and recorded by an immutable
checkpoint. At default geometry, a segment holds at most 16,375 chunks of
32 KiB each. A fixed number of chunk buffers and one segment's descriptors are
kept in RAM; per-segment checkpoint metadata grows with the number of segments.
Scratch space is limited to one encoded segment and its native copy during
sealing. Previously sealed native segments occupy the space needed for the
eventual image; the source is never spooled in full.

`position()` includes all accepted input. `checkpoint_offset()` includes only
successfully sealed input. `checkpoint()` and `Write::flush()` seal complete
chunks early, but retain a partial chunk in memory. The last chunk is sealed
automatically when the exact source size is reached. `finish()` rejects short
input, and writes reject excess input. Errors from the low-level `Write` API
poison the writer; drop it and resume before supplying more data.

Resume checks the configuration, caller-supplied source identity, checkpoint
records, and the full SHA256 of every sealed container segment. It then decodes
the sealed prefix to rebuild all three media hash states. This costs a complete
read/hash of the acquired prefix but never rewrites it. Input after the last
successful checkpoint must be supplied again. The caller must prevent source
changes or provide a new identity; the library does not inspect a physical
device's identity or compare the live source with previous input.

## Publication and recovery boundaries

During acquisition, native segments and checkpoint records are stored beside
the destination in `.case.E01.ewf-acquisition`. Preserve that whole directory.
It is a private versioned implementation format, not a libewf resume file. The
first path, source identity, and configuration must match on resume.

`finish()` exclusively creates destination hard links to the sealed segments,
then retires the checkpoint directory after synchronizing publication. This
requires a filesystem supporting file locking and hard links. Existing output,
including higher-numbered segments after gaps, is rejected. An interrupted
publication is resumed with the same `resume(...).finish()` sequence. Readers
in this library reject a pending acquisition; other EWF tools do not honor its
sidecar, so use the output only after successful completion. A failure during
final cleanup can leave a completed image and inert
`.ewf-acquisition-cleanup-*` directories. Interrupted initialization can leave
inert `.ewf-acquisition-init-*` directories.

Files are flushed before checkpoints are acknowledged. Unix also flushes
directory entries. Windows power-loss durability, network filesystems, and
devices that disregard flushes are not certified. This API does not open
device handles itself, perform positioned output writes, replace an existing
image, mirror targets, or resume arbitrary E01 files from other producers.
## Bounded EWF2 writing

`SequentialWriter` accepts an exact known source length and streams physical
Ex01 or logical Lx01 output into staged native segments. Select
`SequentialOptions::chunks_per_segment` (raw capacity at most 512 MiB); this is
not an encoded segment size limit. Chunks are limited to 16 MiB. The example
`cargo run --release --example sequential -- SOURCE OUTPUT [zlib]` uses 32 MiB
raw capacity per segment and verifies the published image's SHA256.

Only the current encoded segment is spooled, alongside output staging. Memory
contains a chunk, codec buffers, current chunk descriptors, and the segment path
list. Logical catalogs remain in memory and are written in the final segment.
`LogicalWriter::create_sequential` builds that catalog while accepting files;
declare the sum of file lengths as `source_size`. Empty files/directories work.
An incomplete file, cancellation, or oversize write prevents publication. Final
sector padding is zero and included in image hashes, but not file hashes.

`finish` uses the existing recoverable publication transaction, including
mirrors and optional replacement. Completed staged native segments occupy the
eventual output size; this is bounded *payload scratch*, not bounded total disk
use or catalog memory. No seeking, checkpoint resume, or encoded-size split
limit is supported. After process interruption call `EwfWriter::recover_output`
to discard uncommitted staging or recover interrupted publication. Existing
`EwfWriter` remains available for seek/patch operations and EWF1 logical output.
The E01 `AcquisitionWriter` remains the resumable path. The sequential EWF2
transaction uses renames rather than hard links; removable filesystems still
need their own durability acceptance tests.

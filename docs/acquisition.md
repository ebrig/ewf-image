# Acquisition and writing

Choose a writer based on how the input is accessed, the output format family,
and whether the operation must be resumable. The CLI uses these APIs and adds
source checks and JSON reporting. See [CLI acquisition](cli.md#acquisition).

## Choose a writer

| API | Use when | Main constraint |
| --- | --- | --- |
| `EwfWriter` | You need seek/patch, encoded chunks, or general EWF1/EWF2 authoring | Full-source spooling; EWF1 resume rewrites output |
| `AcquisitionWriter` | You need resumable physical E01 acquisition | Known aligned size, raw/zlib, one destination, hard links |
| `SequentialWriter` | You need append-only E01/Ex01/Lx01 with bounded payload scratch | Known size; no seek or checkpoint resume |
| `LogicalWriter` | You are supplying files and metadata for L01/Lx01 | Retained catalog; general or sequential EWF2 backend |

Library finalization computes hashes but does not reread the output. The EWF
acquisition and collection commands in `ewf-cli` verify output after publication. AFF4 support
is provided by a separate
[sibling crate](https://github.com/ebrig/ewf-image/tree/main/crates/aff4-image),
which has its own single-volume writer and optional verification before publication.

## Resumable E01 acquisition

`AcquisitionWriter` creates physical E01 images from an append-only source whose
size is known, nonzero, and sector-aligned. The writer supports raw and zlib
compression, one destination, and MD5, SHA1, and SHA256 media digests. Use
`EwfWriter` for positioned writes, other container families, mirroring, and
resume by rewriting.

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
`identity`. Then continue from the sealed offset:

```rust,ignore
// Continuation: source, options, and identity come from the acquisition session.
use std::io::{Seek, SeekFrom};
let mut writer = AcquisitionWriter::resume("case.E01", &options, identity)?;
source.seek(SeekFrom::Start(writer.checkpoint_offset()))?;
std::io::copy(&mut source, &mut writer)?;
writer.finish()?;
```

## Source reads, progress, and cancellation

For a file or seekable device handle opened by the caller, use `acquire_from` or
`acquire_with_progress`. These methods seek to the writer's accepted offset
before every read attempt, including after resume, so the caller does not need to
position the source. The application is responsible for opening devices,
obtaining privileges, discovering the source size, and identifying the source.

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

Normal reads are chunk-sized. When a bulk read fails, its data is discarded and
the range is reread one sector at a time to isolate damaged sectors.
`AcquisitionReadOptions::bulk_read_bytes` optionally groups chunks into a
sector-aligned healthy read of at most 16 MiB. Its default leaves reads
chunk-sized. Larger failed attempts are still re-read sector by sector and can
delay progress on slow media. The option does not change on-disk geometry.
`AcquisitionWriter` also exposes current-instance durations for chunk processing,
scratch writes, and segment sealing. A resumed writer starts new timing counters;
these are diagnostic measurements, not evidence metadata.
`AcquisitionOptions::compression_level` can select `WriteCompressionLevel::Fast`
for zlib. The level is fixed by the checkpoint identity and must be supplied
unchanged on resume. Compare its throughput and stored size against the default
on representative source data before using it for a long acquisition.

`retries` is the number of additional attempts for each sector, from 0 through 100. The
initial bulk failure does not count against that number. Short successful reads
are completed before any bytes from the attempt enter the image. Progress reports
include read and retry counts for the current call, the current read offset and
error kind, the total accepted and checkpointed bytes, and the cumulative count
of substituted sectors.

The default policy stops at an unreadable sector. `ZeroFill` must be selected
explicitly. `ZeroFill` substitutes zeros for exactly one sector and records its
location. EOF, failed seeks, permission and configuration failures, timeouts,
and interrupted or nonblocking reads always stop acquisition. These conditions
are never padded to the declared source size.

`maximum_error_ranges` limits error-range storage and defaults to 65,536. When
the limit is reached, acquisition stops before another separate substitution.
Adjacent substitutions share a range. EWF1 error tables use 32-bit sector
addresses, so acquisition stops if a failed sector cannot be represented.

`Complete` means that the declared output range is filled, possibly with
substitutions. `Complete` does not mean that every source sector was read
successfully. `acquisition_errors()` returns the substituted ranges, including
accepted substitutions that are not yet sealed. Each sealed segment carries a
cumulative native EWF `error2` table. The final table describes all substituted
sectors and is compatible with libewf. Resume restores only the sealed ranges.

Media hashes describe the bytes actually written, including substituted zeros.
A successful hash verification does not prove that substituted source data was
recovered. The library does not store per-attempt diagnostics or retry counts.
The CLI records read-error kinds, offsets, retry counters, and run results in a
separate [acquisition history](cli.md#results). These records are not EWF metadata.

When the callback returns `Break(())`, the writer stops between source I/O
operations, checkpoints the complete accepted chunks, and returns `Cancelled`. A
partial chunk remains in the open writer. Either continue writing, or drop the
writer and resume from `checkpoint_offset()`. Source failures also checkpoint
full chunks and leave the writer usable. Destination failures poison the writer.
Callbacks run on the calling thread, so the callback cannot interrupt an
operating-system read or seek in progress or a native segment seal. Applications
can check an atomic stop flag in the callback or forward progress to their own
user interface.

`checkpoint_interval` forces segments to be sealed earlier. The interval must be
a positive multiple of the chunk size and must fit the native segment-number
namespace. The interval counts accepted bytes since the last checkpoint. Regular
segment boundaries still apply. A read policy changed after resume affects only
new input.

## Inspecting and validating a checkpoint

Close the writer before calling these functions so that inspection can acquire
the output lock. Supply the original acquisition options and source identity:

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

Inspection reports the resumable offset, the sealed segment count and size,
whether the acquisition is ready to finish, whether publication has started, and
the normalized ranges of substituted sectors. Inspection never cleans scratch
files, rewrites checkpoints, reads the source, or publishes output.
Metadata-only inspection does not scan media payloads or certify their contents.
Validation also compares each complete sealed file with its checkpointed SHA256.
Validation does not decode or rehash the logical media.

`resume_with_progress` reports the container validation and logical-media rehash
phases. `finish_with_progress` reports validation and publication. Both callbacks
receive `AcquisitionOperationProgress`, whose byte units and totals refer to the
reported phase. Returning `Break(())` returns `EwfError::Aborted` and leaves the
journal resumable. Cancellation remains recoverable through resume and finish
even after output links have been installed. Retiring the journal is the commit
point for publication. The final cleanup after that point cannot be cancelled.

## Checkpoints and resource use

The writer encodes one chunk at a time into a scratch file sized for one segment.
A full segment is sealed as native EWF data, synchronized, and recorded by an
immutable checkpoint. At the default geometry, a segment holds at most 16,375
chunks of 32 KiB each. A fixed number of chunk buffers and the descriptors for
one segment are kept in memory. Checkpoint metadata grows with the number of
segments. Scratch space is limited to one encoded segment plus its native copy
during sealing. Sealed native segments occupy the space of the final image, and
the source is never spooled in full.

`position()` includes all accepted input. `checkpoint_offset()` includes only
input that has been sealed successfully. `checkpoint()` and `Write::flush()` seal
complete chunks early but keep a partial chunk in memory. The last chunk is
sealed automatically when the exact source size is reached. `finish()` rejects
short input, and writes reject excess input. An error from the low-level `Write`
API poisons the writer. Drop the writer and resume before supplying more data.

Resume checks the configuration, the source identity supplied by the caller, the
checkpoint records, and the full SHA256 of every sealed container segment.
Resume then decodes the sealed prefix to rebuild all three media hash states.
This step reads and hashes the entire acquired prefix but never rewrites it. Any
input after the last successful checkpoint must be supplied again. The caller
must prevent source changes or supply a new identity. The library does not
inspect a physical device's identity or compare the live source with earlier input.

## Publication and recovery boundaries

During acquisition, native segments and checkpoint records are stored in a
`.case.E01.ewf-acquisition` directory beside the destination. Preserve the whole
directory. The directory uses a private, versioned format and is not a libewf
resume file. The first segment path, source identity, and configuration must
match on resume.

`finish()` exclusively creates hard links from the destination paths to the
sealed segments, synchronizes the publication, and then retires the checkpoint
directory. The filesystem must support file locking and hard links. Existing
output is rejected, including higher-numbered segments after a gap. An
interrupted publication is completed with the same `resume(...).finish()`
sequence.

Readers in this library reject an image with a pending acquisition. Other EWF
tools do not recognize the acquisition directory, so use the output only after
acquisition completes successfully. A failure during final cleanup can leave a
completed image and inert `.ewf-acquisition-cleanup-*` directories. An
interrupted initialization can leave inert `.ewf-acquisition-init-*` directories.

Files are flushed before checkpoints are acknowledged. On Unix, directory
entries are also flushed. Windows power-loss durability, network filesystems,
and devices that ignore flushes are not certified. `AcquisitionWriter` does not
open device handles, perform positioned output writes, replace an existing
image, mirror output, or resume E01 files from other producers.

## Bounded EWF1 and EWF2 writing

`SequentialWriter` accepts an exact source length and streams physical E01 or
Ex01, or logical Lx01, into staged native segments. Set
`SequentialOptions::chunks_per_segment` to at most 512 MiB of raw capacity. This
setting is not a limit on encoded EWF2 segment size. For E01,
`WriteOptions::maximum_segment_size` conservatively lowers the chunks per
segment to account for encoded bytes and metadata. Chunks are limited to 16 MiB. The
example `cargo run --release --example sequential -- SOURCE OUTPUT [zlib]` uses
32 MiB of raw capacity per segment and verifies the SHA256 of the published image.

Only the current encoded segment is spooled, in addition to the staged output.
Memory holds one chunk, codec buffers, the current chunk descriptors, and the
list of segment paths. Logical catalogs remain in memory and are written in the
final segment. `LogicalWriter::create_sequential` builds the catalog while
accepting files. Declare the sum of all file lengths as `source_size`. Empty
files and directories are supported. An incomplete file, cancellation, or an
oversize write prevents publication. Final sector padding is zero and is
included in image hashes but not in file hashes.

`finish` uses the recoverable publication transaction, which supports mirrors and
optional replacement. Staged native segments occupy the size of the final output.
Only payload scratch space is bounded. Total disk use and catalog memory are not.
Seeking and checkpoint resume are unsupported. EWF2 splitting by encoded size
is unsupported. After
a process interruption, call `EwfWriter::recover_output` to discard uncommitted
staging or complete an interrupted publication.

Use `EwfWriter` for seek and patch operations and for EWF1 logical output. Use
the E01 `AcquisitionWriter` for resumable acquisition. The sequential writer's
transaction uses renames rather than hard links. Removable filesystems require
their own durability acceptance tests.

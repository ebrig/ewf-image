# Streaming acquisition

`AcquisitionWriter` creates physical E01 images from an append-only source with
a known, nonzero, sector-aligned size. It supports raw and zlib compression,
one destination, and MD5/SHA1/SHA256 media digests. `EwfWriter` remains available
for positioned writes, other container families, mirroring, and rewrite-based
resume.

```rust,no_run
use std::fs::File;
use ewf_image::{AcquisitionOptions, AcquisitionWriter};

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

After an interruption, reopen the source and reconstruct the same options and
identity, then use:

```rust
use std::io::{Seek, SeekFrom};
let mut writer = AcquisitionWriter::resume("case.E01", &options, identity)?;
source.seek(SeekFrom::Start(writer.checkpoint_offset()))?;
std::io::copy(&mut source, &mut writer)?;
writer.finish()?;
```

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
input, and writes reject excess input. Errors poison the writer; drop it and
resume before supplying more data.

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
devices that disregard flushes are not certified. This API does not acquire
devices itself, retry bad sectors, add error ranges, seek, replace an existing
image, mirror targets, or resume arbitrary E01 files from other producers.

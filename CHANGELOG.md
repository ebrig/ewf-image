# Changelog

All notable changes to this project are documented here.

## Unreleased

### Fixed

- Recovered EWF1 chunk offsets that overflow 31 bits in large unsegmented
  images with a zero table base offset, matching FTK-style single-file `.E01`
  output larger than 2 GiB.
- Replaced the per-read linear table-range scan with a binary search so chunk
  lookups stay fast on multi-terabyte images with thousands of chunk tables.
- Table-entry checksums are now validated with a fixed 64 KiB streaming buffer
  instead of allocating the complete table-entry region in memory.
- The EWF1 writer now emits multiple `sectors`/`table`/`table2` groups per
  segment, each with its own 64-bit table base offset, so non-segmented images
  larger than 2 GiB can be written and read back. Groups use the conservative
  16,375-entry compatibility limit documented for FTK Imager and legacy EnCase
  formats; later EnCase versions permit more entries per table. Previously such
  writes failed with a 31-bit offset error.
- EWF1 maximum-segment-size estimation now follows the actual ordered table
  group boundaries, preventing interacting payload and entry limits from
  producing segments slightly larger than the configured maximum.
- Writer segment planning now retains index ranges into the original chunk
  descriptor vector instead of building per-segment descriptor vectors,
  reducing peak memory without writing an additional descriptor index to disk.

### Known Limitations

- Large writes still require temporary space for the raw and encoded media and
  retain one in-memory descriptor per logical chunk, so descriptor memory grows
  with the image's chunk count.

## 0.1.1 - 2026-07-16

### Fixed

- Removed the fixed EWF1 section-chain limit so large unsegmented images can be
  opened, while still rejecting non-advancing and overlapping section chains.
- Corrected raw-chunk checksum failures to report `raw chunk checksum mismatch`.
- Preserved complete fixture paths and underlying errors in external-corpus
  test failures.

## 0.1.0 - 2026-07-09

Initial release.

### Added

- Rust reader for EWF1 physical/logical/SMART and EWF2 physical/logical images.
- Rust writer for EWF1 physical/logical/SMART and EWF2 physical/logical images.
- Multi-segment discovery, positioned reads, `Read + Seek` cursors, and bounded decoded-chunk caching.
- Metadata, stored hash, acquisition error, session, track, memory extent, and logical single-file APIs.
- Raw, zlib, EWF2 BZip2, EWF1 empty-block, and EWF2 pattern-fill chunk support.
- Optional streamed MD5/SHA1 verification through the default `verify` feature.
- External fixture and command-line oracle tests behind the `external-fixtures` feature.
- Secondary/shadow target mirroring for file-backed writer output.

### Known Limitations

- Encrypted EWF2 images are detected and rejected; decryption is not currently
  implemented.
- Encrypted writing and base-plus-overlay delta/shadow behavior are not
  currently implemented.
- EWF2 BZip2 external oracle coverage depends on broader external tool support
  for that path.

# Changelog

All notable changes to this project are documented here.

## 0.4.0 - 2026-09-12

- Added SHA256 media hashing, external reference comparisons, cancellable
  progress, and optional bounded parallel verification through `VerifyOptions`.
- Verification now bypasses decoded-chunk caches and zero-fill policies, so
  recovery substitutes cannot be accepted as verified media.
- Added typed integrity reports with scan coverage, bounded findings, matching
  EWF1 redundant-table comparison, and optional Serde serialization.
- Added positioned segment sources, bounded subranges, and section summaries
  for EWF1/EWF2 images, preserving table-cache and path-reader handle limits.
- Added a separate physical raw/zlib EWF1 recovery API with redundant-table
  fallback, explicit suspect-data policy, bounded provenance, cancellation,
  output-size controls, and exclusive creation of raw output files.

## 0.3.0 - 2026-08-28

### Breaking Changes

- `CompressionMethod` and `DataChunkEncoding` now expose a `Zstd` variant and
  are non-exhaustive. Downstream matches must include a wildcard arm. See
  [Migrating to 0.3](docs/migrating-to-0.3.md).

### Added

- Added reader support for X-Ways Forensics 20.9+ EWF1 images that use
  Zstandard-compressed metadata, magicless Zstandard media frames, and the
  X-Ways one-byte zero-chunk marker. Decoding uses a pure-Rust implementation.
- Added password-aware reading for X-Ways EWF1 AES-128 and AES-256 CTR images,
  including `x_encryption` metadata validation, constant-time password
  verifier checks, transparent chunk decryption, and non-secret
  `EncryptionInfo` reporting. Password storage is zeroized on drop.
- Added `Image::open_with_password` and password-aware variants for explicit
  options, segment lists, and caller-supplied readers.
- Verifier-less encrypted images now validate their first mandatory media
  chunk during open, so an incorrect password cannot produce a successful
  image handle before structural validation.

### Fixed

- EWF1 continuation segments may now inherit media geometry from the first
  segment when they omit their own `volume`, `disk`, or `data` section.
  Repeated media sections are checked for consistent geometry.
- Reject unknown or contradictory EWF1 compression markers instead of guessing
  a decoder, and cap Zstandard decoder windows before allocation.

## 0.2.0 - 2026-07-17

### Breaking Changes

- `OpenOptions` fields are now private. Configure reader behavior with the
  `with_*` builders and inspect it with getters. This prevents future reader
  controls from breaking struct-literal callers. See
  [Migrating to 0.2](docs/migrating-to-0.2.md).

### Added

- Added byte-based decoded-chunk cache sizing and a bounded 4 MiB table-entry
  page cache shared by all clones and cursors from an image. Both limits are
  configurable through `OpenOptions`.
- Added opt-in `ReaderStatistics` snapshots for cache, I/O, parsing, handle,
  checksum, and decompression diagnostics. Collection is disabled by default.
- Added `ReaderCacheInfo` snapshots for the configured chunk/table cache
  capacities and current/peak retained table-page payload bytes.

### Fixed

- Recovered EWF1 chunk offsets that overflow 31 bits in large unsegmented
  images with a zero table base offset, matching FTK-style single-file `.E01`
  output larger than 2 GiB.
- Replaced the per-read linear table-range scan with a binary search so chunk
  lookups stay fast on multi-terabyte images with thousands of chunk tables.
- Cached segment lengths after their first lookup so repeated encoded-chunk and
  table-page reads do not seek to the end of a segment again.
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

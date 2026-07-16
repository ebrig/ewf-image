# Changelog

All notable changes to this project are documented here.

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

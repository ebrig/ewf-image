# Compatibility

This crate targets practical read/write compatibility with common Expert
Witness Format image families while keeping a Rust-first API. The tables below
summarize the public support surface.

## Reader Support

| Area | Status | Notes |
| --- | :---: | --- |
| EWF1 physical `.E01` / EVF | ✓ | Raw/zlib chunks, split segments, metadata, hashes, acquisition errors, sessions, tracks, and table variants. |
| EWF1 logical `.L01` / LVF | ✓ | Logical single-file catalogs and path lookup. |
| EWF1 SMART `.S01` | ✓ | SMART media profile handling and table-resident chunks. |
| EWF2 physical `.Ex01` | ✓ | Raw, zlib, BZip2, pattern-fill chunks, EWF2 metadata, memory extents, and split segments. |
| EWF2 logical `.Lx01` | ✓ | Logical single-file catalogs and auxiliary single-file tables. |
| Multi-segment discovery | ✓ | EWF1 and EWF2 sibling naming schemes, plus explicit segment lists. |
| Metadata and hashes | ✓ | Typed fields and compatibility-oriented generic header/hash value maps. |
| Stored MD5/SHA1 parsing | ✓ | Available with or without default features. |
| Streamed MD5/SHA1 verification | ✓ | `Image::verify()` and `VerifyResult`, enabled by the default `verify` feature. |
| EWF2 section integrity checks | ✓ | Available with or without default features. |
| Corruption and encryption probes | ✓ | Lightweight file and segment probes that run without fully opening an image. |
| Encrypted EWF2 decryption | — | Not yet implemented; encrypted images are detected and rejected. |
| Base-plus-overlay delta/shadow images | — | Not yet implemented; no confirmed public reference surface is available. |

## Writer Support

| Area | Status | Notes |
| --- | :---: | --- |
| EWF1 physical `.E01` | ✓ | Raw/zlib chunks, metadata, hashes, range sections, and segment splitting. |
| EWF1 logical `.L01` | ✓ | Logical single-file catalog emission. |
| EWF1 SMART `.S01` | ✓ | SMART profile output. |
| EWF2 physical `.Ex01` | ✓ | Raw, zlib, BZip2, pattern-fill chunks, metadata, memory extents, and segment splitting. |
| EWF2 logical `.Lx01` | ✓ | Logical single-file metadata and auxiliary tables. |
| Metadata and hashes | ✓ | Typed metadata, generic header values, stored MD5/SHA1, and generic hash values; available with or without default features. |
| Acquisition errors, sessions, tracks | ✓ | EWF1 and EWF2 range-style metadata. |
| Incomplete and resumed EWF1 output | ✓ | `finish_incomplete` writes `next`; `resume` appends and rewrites a complete image. |
| Secondary/shadow target mirroring | ✓ | `WriteOptions::secondary_segment_filename` writes a byte-identical secondary segment set for file-backed finishes. |
| Encrypted writing | — | Not yet implemented; EWF2 encrypted section emission is unavailable. |
| Base-plus-overlay delta/shadow writing | — | Not yet implemented; a verified reference format/API is needed first. |

## Oracle Coverage

Compatibility is tested in layers:

- Synthetic unit fixtures cover malformed inputs, boundary checks, table
  variants, chunk encodings, logical files, metadata, hashes, and writer round
  trips.
- Ignored external corpus tests compare raw stream output, metadata, hash
  values, and verification behavior against external EWF tools.
- Generated fixture tests exercise externally-created EWF1 and extended EWF
  profiles, plus writer-created EWF2 and logical single-file cases.

EWF2 BZip2 support is covered locally. Some external tools cannot produce or
export EWF2 BZip2 images, so BZip2 external oracle coverage is tracked
separately from local read/write behavior.

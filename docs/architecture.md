# Architecture

The workspace contains two independent format libraries. `ewf-image` exposes EWF
media streams and file catalogs. `aff4-image` exposes identified AFF4 resources,
RDF metadata, Maps, and volume sets. Each library has its own verification model.
The AFF4 crate is experimental and is not a dependency of the EWF package.

The `ewf-cli` workspace package, whose name is provisional, depends on both
libraries. The package provides acquisition, conversion, collection, inspection,
and verification. The package includes the EWF operational runtime and native
device adapter at the source level and runs them in the same process. `ewf-cli`
is the only command-line executable; it is unpublished and must be built from
this workspace. Conversion reads decoded
streams, maps supported metadata, and reports omissions without making either
format library depend on the other.

## EWF reading

`Image` is an immutable shared handle. Opening an image either discovers sibling
segments or uses an explicit ordered list. Opening validates signatures and
section records and builds a lazy chunk index. Table ranges avoid allocating one
record per media chunk. `ImageInfo` exposes geometry, metadata, digests,
acquisition errors, sessions, tracks, memory extents, and logical-file details.

A positioned read locates the chunk and reads its encoded bytes from the segment.
The reader then decrypts X-Ways EWF1 data when configured, validates applicable
checksums, and decodes the payload. Supported codecs are raw, zlib, X-Ways
Zstandard, BZip2, and pattern-fill. Decoded chunks and table pages use bounded
caches that are shared by clones and cursors. Table checksums use a fixed 64 KiB
streaming buffer.

`OpenOptions` controls caches and open handles. The optional `ReaderStatistics`
records cumulative I/O, cache, parsing, checksum, and decompression counters.
`ReaderCacheInfo` reports retained cache usage. Statistics are disabled by
default. `SegmentSource` supports immutable files, memory buffers, and bounded
subranges. After opening, supplied sources serve positioned reads directly.

Verification bypasses decoded caches and zero-fill policies, decodes strictly,
and hashes media in order. Optional worker threads process bounded batches.
Analysis adds findings and redundant-table comparisons, and incomplete scans
report no whole-media digests. `EwfRecovery` is a separate recovery path for
physical raw or zlib EWF1 images and records explicit provenance. See
[verification and recovery](reader-analysis.md).

## EWF writing

| API | Input model | Retained state | Publication / resume |
| --- | --- | --- | --- |
| `EwfWriter` | Sequential, positioned, or chunk writes | Full raw spool, then encoded spool and descriptors | Recoverable replacement/mirroring transaction; incomplete EWF1 resume rewrites output |
| `SequentialWriter` | Exact known length, append-only EWF2 | One encoded segment, pending chunk, current descriptors, output paths | Same transaction; no checkpoint resume |
| `AcquisitionWriter` | Known, sector-aligned physical E01 source | One segment's scratch and descriptors; growing checkpoint records | Exclusive hard-link publication; sealed-prefix resume |
| `LogicalWriter` | Declared-length files with authored metadata | Catalog plus selected backend's state | General L01/Lx01 or sequential Lx01 backend; no checkpoint resume |

`LogicalWriter` assigns identifiers and contiguous extents and computes MD5 and
SHA1 for each file. The root identifier is 1. Empty files are represented
explicitly, and nesting is limited to 128 levels. Names that contain NUL, tab,
CR, or LF are rejected. Short reads, write errors, and cancellation poison the
builder. The library accepts metadata authored by the caller. The library does
not capture filesystem ACLs, ADS, xattrs, sparse allocation, or snapshots.
Sequential logical output writes its catalog in the final segment.

The general and sequential writers stage native segments and use a publication
journal that keeps backups when output is replaced. Recovery rolls back
uncommitted work, or keeps committed output and completes cleanup. The journal
does not provide an atomic multi-file switch for other software. Destinations
owned by the caller bypass the transaction.

Acquisition checkpoints bind the configuration and the caller-supplied source
identity to the sizes and SHA256 values of sealed segments. Resume validates
those files and rehashes their logical media to rebuild the digest state without
rewriting the prefix. A private reader accepts exactly the checkpointed prefix.
Public open functions require complete media coverage. See
[acquisition](acquisition.md) for limits.

## AFF4 reading and writing

Opening an AFF4 container preflights ZIP directory allocation. The library
defaults enforce budgets for metadata, entries, members, chunks, Maps, and
verification work. The command-line tools set these limits from platform
capacity instead of applying application quotas. RDF resources are identified by URI, and
original paths are stored as metadata. `VolumeSet` preserves the graph of each
volume and resolves cross-volume ImageStreams by ownership, using a complete Map
in the primary volume.

The writer streams declared-length payloads into one temporary ZIP file and keeps
metadata in memory until finalization. `finish_verified` verifies the finalized
staging file under reader budgets before publishing it exclusively. `finish` does
not verify. The collector adds discovery budgets and incremental metadata
budgets. Multi-volume writing and resume are unsupported. See the
[AFF4 guide](https://github.com/ebrig/ewf-image/tree/main/crates/aff4-image).

## Code boundaries

| EWF module | Responsibility |
| --- | --- |
| `segment`, `source` | Segment discovery, handles, and positioned backings |
| `format`, `sections`, `metadata` | On-disk structures, descriptor summaries, and metadata |
| `index`, `decode`, `encryption` | Chunk lookup, bounded decoding, and X-Ways password handling |
| `image`, `reader_cache`, `reader_statistics` | Shared reader state, caches, diagnostics |
| `single_files`, `logical_verify` | Catalog parsing and per-file reads/verification |
| `writer`, `publication` | Format emission, writer backends, and transaction recovery |
| `verify`, `integrity`, `image::recovery` | Media checks, findings, and recovery provenance |
| `crates/ewf-cli/src/ewf` | EWF device access, source policy, sessions, and history |

Both libraries forbid unsafe Rust. The EWF CLI has a narrow native Windows
boundary for read-only device queries and cancellation of source I/O. The
application is responsible for source consistency, privileges, and the
interpretation of results.

EWF APIs use `Result<T>` and `EwfError`. AFF4 has its own result and error types.
Bounds and integrity checks reject malformed structures and unsupported profiles.
A successful open, readable bytes, matching references, completed publication,
and storage durability are separate outcomes.

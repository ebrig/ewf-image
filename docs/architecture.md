# Architecture

The workspace contains two independent format libraries. `ewf-image` exposes EWF
media streams and file catalogs; `aff4-image` exposes identified AFF4 resources,
RDF metadata, Maps, and volume sets. Each retains its own verification model.
AFF4 remains experimental and is not a dependency of the EWF package.

The provisional `ewf-cli` workspace package depends on both libraries and
provides acquisition, conversion, collection, inspection, and verification.
It shares the existing EWF operational runtime and native device adapter by
source inclusion, in process, while the two legacy executables remain available.
The unified package is unpublished and must be built from this workspace.
Conversion reads decoded streams, maps supported metadata, and reports omissions;
it does not make either format library depend on the other.

## EWF reading

`Image` is an immutable shared handle. Opening discovers sibling segments or uses
an explicit ordered list, validates signatures and section records, and builds a
lazy chunk index. Table ranges avoid allocating one record per media chunk.
`ImageInfo` exposes geometry, metadata, digests, acquisition errors, sessions,
tracks, memory extents, and logical-file details.

A positioned read locates the chunk, reads encoded bytes from its segment,
decrypts X-Ways EWF1 when configured, validates applicable checksums, and decodes
the payload. Codecs include raw, zlib, X-Ways Zstandard, BZip2, and pattern-fill.
Decoded chunks and table pages use bounded caches shared by clones and cursors.
Table checksums use a fixed 64 KiB streaming buffer.

`OpenOptions` controls caches and open handles. Optional `ReaderStatistics`
records cumulative I/O, cache, parsing, checksum, and decompression counters;
`ReaderCacheInfo` exposes retained cache usage. Statistics are disabled by default.
`SegmentSource` also supports immutable files, memory, and bounded subranges.
After opening, supplied sources serve positioned reads directly.

Verification bypasses decoded caches and zero-fill policies, decodes strictly,
and hashes media in order. Optional workers process bounded batches. Analysis
adds findings and redundant-table comparisons; incomplete scans have no whole-media
digests. `EwfRecovery` is a separate physical raw/zlib EWF1 recovery path with
explicit provenance. See [verification and recovery](reader-analysis.md).

## EWF writing

| API | Input model | Retained state | Publication / resume |
| --- | --- | --- | --- |
| `EwfWriter` | Sequential, positioned, or chunk writes | Full raw spool, then encoded spool and descriptors | Recoverable replacement/mirroring transaction; incomplete EWF1 resume rewrites output |
| `SequentialWriter` | Exact known length, append-only EWF2 | One encoded segment, pending chunk, current descriptors, output paths | Same transaction; no checkpoint resume |
| `AcquisitionWriter` | Known, sector-aligned physical E01 source | One segment's scratch and descriptors; growing checkpoint records | Exclusive hard-link publication; sealed-prefix resume |
| `LogicalWriter` | Declared-length files with authored metadata | Catalog plus selected backend's state | General L01/Lx01 or sequential Lx01 backend; no checkpoint resume |

`LogicalWriter` assigns identifiers and contiguous extents and computes per-file
MD5/SHA1. Root identifier is 1. Empty files are explicit; nesting is limited to
128. Names containing NUL, tab, CR, or LF are rejected. Short reads, write errors,
and cancellation poison the builder. The library accepts caller-authored metadata;
it does not capture filesystem ACLs, ADS, xattrs, sparse allocation, or snapshots.
Sequential logical output emits its catalog in the final segment.

The general and sequential writers stage native segments and use a publication
journal with backups for replacement. Recovery rolls back uncommitted work or
retains committed output and completes cleanup. This is not an atomic multi-file
switch for other software. Caller-owned destinations bypass the transaction.

Acquisition checkpoints bind configuration and caller-supplied source identity
to sealed segment sizes and SHA256 values. Resume validates those files and
rehashes their logical media to rebuild digest state without rewriting the prefix.
A private reader accepts exactly that checkpointed prefix; public opens still
require complete media coverage. See [acquisition](acquisition.md) for limits.

## AFF4 reading and writing

AFF4 opens preflight ZIP directory allocation. Library defaults enforce metadata,
entry, member, chunk, Map, and verification-work budgets; the local CLI uses
platform capacity without application quotas. RDF resources remain identified by URI;
original paths are metadata. `VolumeSet` preserves each volume's graph and resolves
cross-volume ImageStreams by ownership, using a complete Map in the primary volume.

The writer streams declared-length payloads into one temporary ZIP and retains
metadata until finish. `finish_verified` verifies finalized staging under reader
budgets before exclusive publication; `finish` does not verify. The collector
adds discovery and incremental metadata budgets. Multi-volume writing and resume
are unsupported. See the
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
| `bin/ewf-image` | CLI, device access, source policy, sessions, and history |

Both libraries forbid unsafe Rust. The EWF CLI has a narrow native Windows
boundary for read-only device queries and source-I/O cancellation. Application
code owns source consistency, privileges, and interpretation of results.

EWF APIs use `Result<T>` and `EwfError`; AFF4 has its own result/error types.
Bounds and integrity checks reject malformed structures and unsupported profiles.
Successful opening, readable bytes, matching references, completed publication,
and storage durability are distinct outcomes.

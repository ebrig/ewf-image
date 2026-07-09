# Architecture

`ewf-image` reads and writes Expert Witness Format images through a small,
explicit public API centered on immutable `Image` values and a dedicated writer
type:

- `Image` opens one or more EWF segments and provides `read_at`, `Read + Seek`
  cursors, metadata accessors, logical single-file access, and optional hash
  verification.
- `ImageInfo` collects format, geometry, segment, metadata, hash, acquisition,
  session, track, memory, and logical single-file details.
- `EwfWriter` creates EWF output from streamed writes, positioned writes, or
  decoded/encoded chunk values.
- `WriteOptions` selects output format, compression, segment sizing, media
  profile, metadata, hashes, and optional logical single-file catalogs.

## Reader Flow

Opening starts with sibling segment discovery or an explicit segment list. Each
segment is signature-checked, parsed into EWF1 or EWF2 section records, and
assembled into a lazy logical chunk index. The lazy index keeps large image
opens bounded by storing table ranges instead of eagerly materializing one
record per logical chunk.

Reads go through `Image::read_at` or a cursor. The reader locates the logical
chunk, reads the encoded bytes from the owning segment, validates checksums
where applicable, decodes raw/zlib/BZip2/pattern-fill payloads, and caches the
decoded chunk in a bounded LRU cache.

## Writer Flow

The writer accepts sequential writes, positioned writes, and chunk-oriented
writes. Input data is spooled while complete chunks are encoded and tracked
with enough metadata to emit EWF tables and segment descriptors at finish time.

`finish` writes complete images with `done` terminal sections.
`finish_incomplete` writes an incomplete EWF1 acquisition with a `next`
terminal section, and `resume` reopens that image, preserves compatible media
values, appends data, and rewrites a complete image. File-backed finishes can
also mirror the completed primary segment set to a secondary/shadow target.

## Internal Boundaries

- `segment`: segment discovery, ordering, and handle pooling.
- `format`: low-level EWF1/EWF2 descriptors, tables, signatures, and primitive
  parsing.
- `metadata`: EWF header/case/device/hash/range metadata parsing.
- `index`: lazy logical chunk lookup across table ranges.
- `decode`: bounded raw, zlib, BZip2, and pattern-fill decoding.
- `image`: open flow, immutable image state, cache ownership, and read APIs.
- `writer`: EWF output generation, segment splitting, secondary target
  mirroring, metadata emission, and resume support.
- `single_files`: logical single-file catalog parsing and lookup.
- `verify`: optional streamed MD5/SHA1 verification through `Image::verify`;
  stored hash parsing, EWF2 section integrity checks, and writer hash support
  are part of the normal reader/writer implementation.

## Error Model

The crate uses `ewf_image::Result<T>` and `EwfError` for all fallible public APIs.
Malformed images, unimplemented format features, invalid signatures, I/O
errors, and aborts are reported without panics. Bounds checks are applied to
offsets, section chains, chunk sizes, decompression limits, table coverage,
segment references, and media geometry arithmetic.

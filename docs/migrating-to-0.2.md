# Migrating to 0.2

Version 0.2 makes `OpenOptions` fields private and replaces struct-literal
configuration with builders. This is the only intentional reader API break in
the current unreleased changes. `Image`, cursor, error, checksum-recovery, and
writer behavior remain source-compatible unless a caller constructs or reads
`OpenOptions` fields directly.

## Replace struct literals

Before:

```rust
let options = ewf_image::OpenOptions {
    strictness: ewf_image::OpenStrictness::Lenient,
    maximum_open_handles: Some(32),
    ..ewf_image::OpenOptions::default()
};
```

After:

```rust
let options = ewf_image::OpenOptions::default()
    .with_strictness(ewf_image::OpenStrictness::Lenient)
    .with_maximum_open_handles(Some(32));
```

Every previous field has a corresponding getter and builder:

| Previous field | Getter | Builder |
| --- | --- | --- |
| `strictness` | `strictness()` | `with_strictness(...)` |
| `chunk_cache_size` | `chunk_cache_capacity()` | `with_chunk_cache_size(...)` |
| `read_zero_chunk_on_error` | `read_zero_chunk_on_error()` | `with_read_zero_chunk_on_error(...)` |
| `header_codepage` | `header_codepage()` | `with_header_codepage(...)` |
| `header_values_date_format` | `header_values_date_format()` | `with_header_values_date_format(...)` |
| `maximum_open_handles` | `maximum_open_handles()` | `with_maximum_open_handles(...)` |

`chunk_cache_capacity()` returns `ChunkCacheCapacity::Chunks` or
`ChunkCacheCapacity::Bytes`, depending on the most recently used chunk-cache
builder.

## New reader controls

The decoded-chunk cache can now be set by byte target:

```rust
let options = ewf_image::OpenOptions::default()
    .with_chunk_cache_size_bytes(64 * 1024 * 1024);
```

At least one decoded chunk is retained even when the requested target is
smaller than a chunk. `Image::reader_cache_info()` reports the resulting
capacity.

Table entries use a shared, byte-bounded 4 MiB page cache by default. Set its
limit explicitly, or pass zero to disable page retention:

```rust
let options = ewf_image::OpenOptions::default()
    .with_table_entry_cache_size_bytes(0);
```

Optional cumulative counters can be enabled at open time:

```rust
let options = ewf_image::OpenOptions::default()
    .with_reader_statistics(true);
let image = ewf_image::Image::open_with_options("case.E01", options)?;
let statistics = image.reader_statistics().expect("statistics enabled");
```

Statistics are shared across `Image` clones and their cursors. They are
disabled by default so normal reads do not perform atomic counter updates or
timing calls.

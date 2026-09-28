# Migrating to 0.2

This historical guide covers the upgrade from 0.1 to 0.2. For later changes,
see [Migrating to 0.3](migrating-to-0.3.md), the
[0.4 release notes](../CHANGELOG.md#040---2026-09-12), and
[Migrating to 0.5](migrating-to-0.5.md).

Version 0.2 makes the `OpenOptions` fields private and replaces struct-literal
configuration with builder methods. This change was the intentional reader API
break in 0.2. `Image`, cursors, errors, checksum recovery, and writers remain
source-compatible unless a caller constructs `OpenOptions` or reads its fields
directly.

## Replace struct literals

Before (0.1 API, which does not compile against current releases):

```rust,ignore
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

Each former field has a matching getter and builder:

| Previous field | Getter | Builder |
| --- | --- | --- |
| `strictness` | `strictness()` | `with_strictness(...)` |
| `chunk_cache_size` | `chunk_cache_capacity()` | `with_chunk_cache_size(...)` |
| `read_zero_chunk_on_error` | `read_zero_chunk_on_error()` | `with_read_zero_chunk_on_error(...)` |
| `header_codepage` | `header_codepage()` | `with_header_codepage(...)` |
| `header_values_date_format` | `header_values_date_format()` | `with_header_values_date_format(...)` |
| `maximum_open_handles` | `maximum_open_handles()` | `with_maximum_open_handles(...)` |

`chunk_cache_capacity()` returns `ChunkCacheCapacity::Chunks` or
`ChunkCacheCapacity::Bytes`, depending on which chunk-cache builder was called
most recently.

## New reader controls

The decoded-chunk cache size can be set in bytes:

```rust
let options = ewf_image::OpenOptions::default()
    .with_chunk_cache_size_bytes(64 * 1024 * 1024);
```

At least one decoded chunk is retained, even when the requested size is smaller
than a chunk. `Image::reader_cache_info()` reports the resulting capacity.

By default, table entries use a shared 4 MiB page cache bounded by size. Set the
limit explicitly, or pass zero to disable page retention:

```rust
let options = ewf_image::OpenOptions::default()
    .with_table_entry_cache_size_bytes(0);
```

Cumulative reader statistics can be enabled when the image is opened:

```rust,no_run
fn main() -> ewf_image::Result<()> {
    let options = ewf_image::OpenOptions::default().with_reader_statistics(true);
    let image = ewf_image::Image::open_with_options("case.E01", options)?;
    let statistics = image.reader_statistics().expect("statistics enabled");
    println!("cache misses: {}", statistics.chunk_cache_misses());
    Ok(())
}
```

Statistics are shared across `Image` clones and their cursors. Statistics are
disabled by default so that normal reads avoid atomic counter updates and timing
calls.

# ewf-image

**A pure-Rust library for reading and writing Expert Witness Format (EWF) forensic images.**

[![Crates.io](https://img.shields.io/crates/v/ewf-image.svg)](https://crates.io/crates/ewf-image)
[![Documentation](https://docs.rs/ewf-image/badge.svg)](https://docs.rs/ewf-image)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

![ewf-image project banner](https://raw.githubusercontent.com/ebrig/ewf-image/main/docs/assets/ewf-image-banner.png)

`ewf-image` reads and writes the EWF/E01 image formats used in digital
forensics — the physical, logical, SMART, and EWF2 families — with no unsafe
code and no external tool dependencies. It exposes focused APIs for raw stream
access, metadata inspection, logical single-file catalogs, and EWF output
creation.

## Highlights

- **Full format coverage.** Reads and writes EWF1 (`.E01`, `.L01`, `.S01`) and
  EWF2 (`.Ex01`, `.Lx01`), including raw, zlib, BZip2, and pattern-fill chunks.
- **Streaming reads.** Immutable `Image` handles offer positioned reads,
  `Read + Seek` cursors, and bounded decoded-chunk caching.
- **Rich metadata.** Inspect acquisition headers, stored MD5/SHA1 hashes,
  acquisition errors, sessions, and tracks.
- **Integrity verification.** Recompute and compare stored hashes with a single
  `Image::verify()` call.
- **Flexible writing.** Compression, segment splitting, secondary/shadow
  mirroring, authored metadata, and resume-by-rewrite.
- **Safe by construction.** `#![forbid(unsafe_code)]`, linted under clippy
  pedantic/nursery, and tested against external EWF tool oracles.

> This release ships the Rust library. Command-line, mount, and service runtime
> layers are planned but not yet implemented.

## Installation

```toml
[dependencies]
ewf-image = "0.2"
```

The default `verify` feature enables streamed MD5/SHA1 verification through
`Image::verify()`. Drop it if you don't need that API:

```toml
[dependencies]
ewf-image = { version = "0.2", default-features = false }
```

## Quick Start

Open an image, inspect its format, and read from the logical media stream:

```rust
use std::io::Read;

let image = ewf_image::Image::open("case.E01")?;
let info = image.info();

println!("{:?}: {} bytes across {} segment(s)",
    info.format, info.logical_size, info.segment_count);

// Read the first sector from the cursor...
let mut first_sector = vec![0; 512];
image.cursor().read_exact(&mut first_sector)?;

// ...or read directly at any offset.
let mut sector_at_offset = vec![0; 512];
image.read_at(&mut sector_at_offset, 4096)?;
```

Read forensic metadata and verify stored hashes:

```rust
let image = ewf_image::Image::open("case.E01")?;

if let Some(case_number) = image.header_value("case_number") {
    println!("case: {case_number}");
}

let result = image.verify()?;
println!("MD5 match: {:?}", result.md5_match);
```

### Reader tuning and diagnostics

Reader caches are shared by every clone and cursor created from an `Image`.
The decoded-chunk cache defaults to 64 chunks, and table entries use a bounded
4 MiB page cache. Configure byte limits and opt into cumulative diagnostics
with `OpenOptions`:

```rust
let options = ewf_image::OpenOptions::default()
    .with_chunk_cache_size_bytes(32 * 1024 * 1024)
    .with_table_entry_cache_size_bytes(8 * 1024 * 1024)
    .with_maximum_open_handles(Some(32))
    .with_reader_statistics(true);

let image = ewf_image::Image::open_with_options("case.E01", options)?;
let before = image.reader_statistics().expect("statistics enabled");

// Perform the reads being measured.
let mut sector = [0; 512];
image.read_at(&mut sector, 0)?;

let delta = image
    .reader_statistics()
    .expect("statistics enabled")
    .saturating_delta(before);
println!("chunk cache misses: {}", delta.chunk_cache_misses());

let cache = image.reader_cache_info();
println!(
    "table cache: {} / {} bytes",
    cache.table_entry_cache_current_bytes(),
    cache.table_entry_cache_capacity_bytes()
);
```

Statistics collection is disabled by default. `reader_statistics()` returns
`None` unless it was enabled when the image was opened. See
[Migrating to 0.2](docs/migrating-to-0.2.md) for the `OpenOptions` builder
migration.

Write a new compressed EWF2 image from raw bytes:

```rust
use std::fs::File;

let mut input = File::open("disk.raw")?;

let mut options = ewf_image::WriteOptions::default();
options.format = ewf_image::WriteFormat::Ewf2Physical;
options.compression = ewf_image::WriteCompression::Zlib;
options.metadata.set_header_value("case_number", "CASE-001");

let mut writer = ewf_image::EwfWriter::create("case.Ex01", options)?;
std::io::copy(&mut input, &mut writer)?;
let result = writer.finish()?;

println!("wrote {} segment(s)", result.segment_paths.len());
```

See [`examples/`](examples) for complete, runnable programs, including reading,
raw export, logical inspection, and mirrored secondary output.

## Supported Formats

| Family | Read | Write | Notes |
| --- | :---: | :---: | --- |
| EWF1 physical `.E01` / EVF | ✓ | ✓ | Segment discovery, raw/zlib chunks, metadata, hashes, acquisition errors, sessions, tracks, and split output. |
| EWF1 logical `.L01` / LVF | ✓ | ✓ | Logical single-file catalogs and path lookup. |
| EWF1 SMART `.S01` | ✓ | ✓ | SMART media profile handling. |
| EWF2 physical `.Ex01` | ✓ | ✓ | Raw, zlib, BZip2, and pattern-fill chunks; EWF2 metadata, memory extents, and split output. |
| EWF2 logical `.Lx01` | ✓ | ✓ | Logical single-file catalogs and auxiliary single-file tables. |

The writer additionally supports SMART output, compression, segment splitting,
secondary/shadow mirroring, authored metadata, stored hashes, incomplete EWF1
output, and resume-by-rewrite.

## Limitations

- Encrypted EWF2 images are detected and rejected; decryption is not yet
  implemented.
- Encrypted writing is not yet implemented.
- Base-plus-overlay delta/shadow images are not yet implemented.
- EWF2 BZip2 chunks are supported locally, but some external EWF tools cannot
  generate or export BZip2 fixtures, so external oracle coverage for them is
  tracked separately.

See [docs/limitations.md](docs/limitations.md) for details.

## Testing

Compatibility is covered by local synthetic fixtures, writer round trips, and
optional external EWF tool oracle tests. The routine checks are:

```bash
cargo fmt --check
cargo test --no-default-features
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo check --examples --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --all-features --no-deps
```

External corpus and tool oracle checks are ignored by default, since they
require local fixtures and installed EWF tools. See
[docs/testing.md](docs/testing.md) and
[docs/compatibility.md](docs/compatibility.md).

## Documentation

- [Architecture](docs/architecture.md)
- [Compatibility](docs/compatibility.md)
- [Limitations](docs/limitations.md)
- [Testing](docs/testing.md)
- [Migrating to 0.2](docs/migrating-to-0.2.md)
- [API reference (docs.rs)](https://docs.rs/ewf-image)

## Contributing

Contributions are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md) to get started,
and [SECURITY.md](SECURITY.md) for reporting security issues.

## License

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE).

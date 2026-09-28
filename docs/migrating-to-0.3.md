# Migrating to 0.3

This historical guide covers the upgrade from 0.2 to 0.3. For later changes,
see the [0.4 release notes](../CHANGELOG.md#040---2026-09-12) and
[Migrating to 0.5](migrating-to-0.5.md).

Version 0.3 adds reader support for X-Ways EWF1 Zstandard compression. Images
that use this encoding report `CompressionMethod::Zstd`, and their decoded chunks
report `DataChunkEncoding::Zstd`.

Both enums are now non-exhaustive, so future on-disk encodings can be added
without another source-breaking change. Downstream `match` expressions must
include a wildcard arm:

```rust,no_run
fn describe(image: &ewf_image::Image) {
    match image.compression_method() {
        Some(ewf_image::CompressionMethod::Zlib) => println!("zlib"),
        Some(ewf_image::CompressionMethod::Zstd) => println!("Zstandard"),
        Some(_) => println!("another compression method"),
        None => println!("compression method unavailable"),
    }
}
```

The reader accepts X-Ways Zstandard chunk payloads without the frame magic number
and the X-Ways one-byte zero-chunk marker. Writer behavior is unchanged, and the
writers do not produce this X-Ways-specific encoding.

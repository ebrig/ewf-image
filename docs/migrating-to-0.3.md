# Migrating to 0.3

Version 0.3 adds X-Ways EWF1 Zstandard reader support. Images using that
encoding now report `CompressionMethod::Zstd`, and decoded chunks report
`DataChunkEncoding::Zstd`.

Both public enums are now non-exhaustive so future on-disk encodings can be
reported without adding another source-breaking exhaustive-match requirement.
Downstream matches must therefore include a wildcard arm:

```rust
match image.compression_method() {
    Some(ewf_image::CompressionMethod::Zlib) => println!("zlib"),
    Some(ewf_image::CompressionMethod::Zstd) => println!("Zstandard"),
    Some(_) => println!("another compression method"),
    None => println!("compression method unavailable"),
}
```

The reader accepts X-Ways magicless Zstandard chunk payloads and the X-Ways
one-byte zero-chunk marker. Writer behavior is unchanged and does not emit this
X-Ways-specific encoding.

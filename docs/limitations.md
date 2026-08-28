# Limitations

This document records behavior that is not yet implemented, so users can
separate current product boundaries from defects.

## Encrypted EWF2 Images

`ewf-image` detects encrypted EWF2 images and returns an unsupported-feature
error instead of attempting partial reads. A lightweight probe can check for
encryption before opening an image:

```rust
if ewf_image::check_file_encryption("case.Ex01")? {
    println!("encrypted image");
}
```

Decrypting EWF2 section payloads is not yet implemented, and encrypted EWF2
writing is not yet implemented.

## Delta, Shadow, and Secondary Output

Secondary/shadow target mirroring is supported for file-backed writer finishes
through `WriteOptions::secondary_segment_filename`. The secondary segment set is
byte-identical to the primary segment set and can be opened independently.

Base-plus-overlay delta/shadow images are not yet implemented. A verified
public API, fixture, or format contract for mutable overlay behavior has not
been identified. Resume-by-rewrite is supported separately through
`EwfWriter::resume` for incomplete EWF1 output.

## BZip2 External Oracles

The reader and writer support EWF2 BZip2 chunks, and local tests cover BZip2
decode and writer round trips. Some external EWF tools cannot generate or
export EWF2 BZip2 images, so external oracle coverage for this path is tracked
separately.

## X-Ways Zstandard Output

The reader supports the X-Ways Forensics 20.9+ EWF1 Zstandard profile,
including its magicless media frames and zero-chunk marker. The writer does not
generate or preserve this producer-specific encoding. Copying X-Ways
compression settings or encoded Zstandard chunks into a writer returns an
unsupported-feature error instead of silently producing a different format.

## Additional Interfaces

CLI, filesystem mount, service runtime, and native FFI-wrapper layers are not
yet implemented. The current public surface is the Rust library API.

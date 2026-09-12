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

## X-Ways Encrypted EWF1 Images

The reader supports X-Ways EWF1 AES-128 and AES-256 CTR images through the
password-aware `Image` open methods. Deflate-compatible and X-Ways Zstandard
compressed images are supported. Password verifiers are checked before media
data is exposed; when a valid image omits a verifier, the first decrypted chunk
must pass structural decoding validation.

Native X-Ways fixtures currently cover compressed images with password
verifiers. X-Ways 21.9 Beta 2 did not expose an uncompressed-output or
skip-verifier option in the tested imaging workflow, so native uncompressed and
verifier-less variants remain external-oracle gaps. A synthetic verifier-less
test covers the reader's fallback validation behavior.

X-Ways encrypted output is not implemented, and no writer encryption option is
exposed.

The encrypted segment path supports per-segment contexts, but the committed
native fixture set is single-segment. Authentic encrypted split-image coverage
remains an external-oracle gap.

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

The library includes positioned segment backings, section summaries, and
structured media analysis. Structural opening failures produce one finding;
analysis does not resynchronize a broken descriptor chain or enumerate all
otherwise-inaccessible defects. Recovery currently supports physical,
unencrypted raw/zlib EWF1 with separate sectors sections and intact geometry.
It does not recover logical/SMART, EWF2, encrypted, Zstandard, or table-resident
images. See [reader analysis](reader-analysis.md) for coverage and output rules.

CLI, filesystem mount, service runtime, and native FFI-wrapper layers are not
yet implemented. The current public surface is the Rust library API.

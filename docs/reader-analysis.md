# Verification, analysis, and recovery

## Media verification

`Image::verify()` preserves its MD5/SHA1 result type. It now reads backing
chunks without consulting the decoded-chunk cache or the zero-fill recovery
policy. Corrupt media cannot pass verification using cached substitute bytes.

`verify_with_options` and `verify_with_progress` additionally compute SHA256
and compare caller-supplied reference digests. Embedded and external references
remain separate in `VerificationReport::comparisons`. A mismatch is a result;
unreadable media is an error. `references_match()` returns `None` when no
supported reference exists, rather than treating the absence of references as
a successful match. SHA256 is computed over logical media; no additional EWF
stored-hash section layout is implied.

```rust
use std::ops::ControlFlow;
use ewf_image::{Image, VerifyOptions};

# fn verify(image: &Image, acquisition_sha256: [u8; 32]) -> ewf_image::Result<()> {
let options = VerifyOptions::default()
    .with_expected_sha256(acquisition_sha256);
let report = image.verify_with_progress(&options, |progress| {
    println!("{} / {} bytes", progress.bytes_verified, progress.bytes_total);
    ControlFlow::Continue(())
})?;
println!("references match: {:?}", report.references_match());
# Ok(())
# }
```

Progress starts at zero and advances in logical chunk order on the calling
thread. Returning `ControlFlow::Break(())` cancels only that operation.
`Image::signal_abort()` still cancels all operations using that image.
Already-running worker reads finish before cancellation returns. Callbacks are
not invoked under reader locks. No completed report is returned on cancellation.

## Parallel scans and memory

The optional `parallel` feature enables worker counts greater than one:

```toml
ewf-image = { version = "0.3", features = ["parallel"] }
```

`VerifyOptions::with_parallelism(4)` selects up to four workers. Defaults remain
single-threaded. A scan owns its worker pool, decompresses bounded batches, and
feeds hashers in logical order. The decoded batch budget defaults to 8 MiB;
`with_chunk_buffer_size_bytes` changes it. At least one chunk is processed, so
a chunk larger than the budget runs alone. Encoded input buffers, decoder
scratch space, reader caches, and thread stacks are additional memory. Batch
size is also capped at twice the worker count. Worker counts outside 1–64 and
zero-byte budgets are rejected before progress starts.

Verification bypasses the decoded cache without clearing it. The bounded table
cache remains shared with normal reads. Path-based images retain their file
pool and handle limits; their seek/read operations serialize while decompression
can overlap. Positioned sources additionally permit concurrent backing reads.
Throughput depends on compression, storage, chunk size, and worker count.

## Structured analysis

`Image::analyze` and `analyze_with_progress` collect chunk failures, disagreements
between matching EWF1 `table`/`table2` entries, acquisition context, and reference
hash mismatches. Chunk failures do not stop analysis. An incomplete pass returns
no media hashes or hash comparisons: omitted or zero-filled bytes are never
represented as a complete media digest.

`IntegrityReport::media_status` distinguishes complete, incomplete, and
unavailable scans. Complete media coverage does not imply that reference hashes
match or that structural findings are absent. Error and warning counts include
findings omitted by `with_maximum_findings`; retention defaults to 1,024 records.
`suppressed_findings` makes that limit visible. The optional `serde` feature
serializes reports, findings, progress, recovery records, and section summaries.

`analyze_path` opens strictly first. A structural opening failure yields one
typed finding and unavailable media coverage. This is not a resynchronizing
structural auditor: it does not enumerate every defect behind a broken chain.
Filesystem errors remain errors. `analyze_path_with_password` handles encrypted
inputs. An already-open `Image` uses its original opening checks; analysis does
not revalidate all descriptors or metadata. Lenient opening is reported.
Table comparison checks pairs with matching counts and base offsets; distinct
`table2` ranges are not treated as mirrors. Media progress starts after table
comparison, which honors `signal_abort`.

## Positioned sources and section inspection

`SegmentReadAt` accepts caller-owned, stable-length positioned backings.
`SegmentSource` supplies memory and file backings plus validated bounded
subranges. `Image::open_sources` and its options/password variants support both
EWF1 and EWF2. Sources must be supplied in segment order; their names are labels
and are never reopened as filesystem paths. All supplied sources remain owned
for the image lifetime. File handle limits below the supplied source count are
rejected. Native file backings use positioned reads on Windows and Unix, with a
serialized fallback on other platforms. Backings must remain immutable while
the image is open.

```rust
use std::fs::File;
use ewf_image::{Image, SegmentSource};

# fn embedded(container: File, offset: u64, length: u64) -> ewf_image::Result<Image> {
let source = SegmentSource::from_file(container)?.subrange(offset, length)?;
Image::open_sources([("embedded.E01", source)])
# }
```

`Image::sections()` exposes accepted descriptor summaries with segment-relative
descriptor and payload locations, original EWF1 names or EWF2 type IDs, and
chain links. These summaries describe opening-time structure; they are not a
fresh integrity verdict and do not expose payload or cryptographic material.

## Damaged-image recovery

`EwfRecovery` is a separate library API for physical, unencrypted raw/zlib EWF1
images with separate sectors sections. It accepts an intact descriptor-chain
prefix and validated first-segment geometry, tries primary and matching
redundant tables, and writes a logical raw image with explicit provenance.
It does not change normal image-opening behavior.

```rust
use ewf_image::{EwfRecovery, RecoveryOptions};

# fn recover() -> ewf_image::Result<()> {
let recovery = EwfRecovery::open("damaged.E01", RecoveryOptions::default())?;
let report = recovery.recover_to_path("recovered.raw")?;
println!("{} bytes substituted", report.bytes_zero_filled);
# Ok(())
# }
```

`recover_to_path` exclusively creates a new file, refusing existing paths and
source aliases. `recover_to_writer` accepts a caller-owned destination, which
must be separate from the evidence. Errors and cancellation leave written
output available for inspection. Recovery does not repair the source EWF or
prove the authenticity of recovered data.

The default substitutes zeros when neither table provides validated data.
`with_preserve_checksum_suspect(true)` permits decoded bytes with a bad raw-data
or table-entry checksum only if no validated alternate succeeds. Those chunks
are separately marked suspect. Primary, redundant, and zero-filled chunk counts
partition the entire output; suspect chunks are a subset of recovered chunks.
Coalesced outcome ranges and structural notices have independent bounded
retention, with omission counts. The progress callback emits every outcome and
supports per-operation cancellation.

`with_maximum_output_bytes` rejects excessive declared output size before
output is created. Missing middle segments, incomplete middle descriptor chains,
ambiguous table coverage before later data, invalid geometry, and unsupported
families are rejected. Recovery does not carve lost descriptors or reconstruct
missing table headers. A missing final range is zero-filled when its placement
is unambiguous. EWF2, logical/SMART images, encryption, X-Ways Zstandard, and
table-resident media recovery are outside this implementation's supported scope.

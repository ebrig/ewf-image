# aff4-image

An experimental Rust library for reading, verifying, and creating AFF4
evidence containers. The crate is versioned separately from `ewf-image` and adds
no AFF4 dependencies or APIs to it. The supported profiles are listed below;
public APIs and JSON output may change in future `0.x` releases.

The workspace source version is 0.1.0.

## Build and use

For physical-device acquisition and for conversion between EWF, AFF4, and raw
images, use the workspace
[ewf-cli](https://github.com/ebrig/ewf-image/tree/main/crates/ewf-cli).

Run these commands from the repository root with Rust 1.96 or later:

```sh
cargo build --release -p ewf-cli --locked
cargo test -p aff4-image --locked
```

The executable is `target/release/ewf-cli` (`ewf-cli.exe` on Windows).

```text
ewf-cli info case.aff4
ewf-cli metadata case.aff4
ewf-cli verify case.aff4
ewf-cli verify case.aff4 --metadata-sha256 HASH
ewf-cli collect snapshot-directory files.aff4 --exclude cache
ewf-cli extract case.aff4 RESOURCE_ID recovered.bin
```

`info` lists the resources that can be selected. Select a resource by its
identifier, which remains unique when original filenames collide. Commands print
concise text by default. Add `--json` for machine-readable reports.
`metadata --json` returns one JSON report that contains the metadata records.
Run `<command> --help` to list options. `verify` checks all supported resources
and integrity structures.

Extraction writes to a new filename chosen by the caller and never interprets
evidence paths as output paths. Add `--restore-times` to restore recorded access
and modification times on the selected file. ACLs, xattrs, alternate streams,
and directory metadata are not restored.

The library and an example program provide physical acquisition:

```sh
cargo run -p aff4-image --example acquire -- disk.raw disk.aff4
cargo run -p aff4-image --example acquire -- note.txt files.aff4 logical
cargo run -p aff4-image --example inspect -- disk.aff4
cargo run -p aff4-image --example inspect -- disk.aff4 --verify-all
```

The examples are demonstration programs, not stable CLI interfaces. Use a stable
source snapshot and new output paths.

## Embed a physical disk reader

The default `write` feature adds writers. Disable default features for a
reader-only dependency. The command-line program is the separate `ewf-cli`
workspace package.

For a published 0.1.x release, a reader-only application can use:

```toml
[dependencies]
aff4-image = { version = "0.1", default-features = false }
```

The crate exposes a library API; installing this dependency does not add a
command-line executable. Enable the default `write` feature if the application
also needs the AFF4 writer.

`DiskImageSet::discover(inputs, candidates)` finds every physical disk connected
to the input containers. It matches companion containers by volume identifiers
and stream references, including when an input is itself a companion. The caller
supplies candidate paths. The library does not search directories or fetch
network resources. Unrelated candidates are excluded, and missing companions or
conflicting owners are errors.

`into_readers()` returns independent `Read` and `Seek` cursors that share one
automatic, byte-bounded payload cache. Small positioned reads use 1 MiB logical
read-ahead pages; direct ImageStream reads retain recently decoded chunks. The
cache is allocated lazily and has a 128 MiB ceiling across the complete volume
set. Each cursor reports its image identity, geometry, backing
paths, cumulative reader statistics, and cache usage. Save its descriptor and
call `reopen()` to reopen exactly those files and confirm their identities.
Discovery accepts up to 128 containers and 128 disks, which share metadata
budgets. Split and striped disks require a primary Map whose targets are
ImageStreams in the resolved containers.

When a caller needs only one container, `Container::disk_images()` lists that
container's explicitly typed physical disks. `Container::into_disk_reader(None)`
selects the only disk and returns an error if the selection is ambiguous. Pass
`Some(resource_id)` to select a specific disk. The returned `DiskImageReader`
implements `Read` and `Seek` over decoded media bytes. Its `info()` method
reports the selected resource, volume, logical size, and declared sector size.

`DiskImageReader` supports one physical AFF4 1.0 volume. Storage streams, memory
images, and logical collections are not disk candidates. Opening and reading an
image do not establish whole-image integrity. Use the verification APIs for that
assessment.

## Supported profiles

| Profile | Read | Write |
| --- | --- | --- |
| Physical AFF4 1.0, one ZIP volume | ImageStreams, Maps, zero/repeated-byte symbolic streams | ImageStreams with contiguous Maps |
| Physical AFF4 1.0, explicit volume set | Sequential and striped Maps | Unsupported |
| Logical AFF4-L 1.1 | ZIP segments, inline content, ImageStreams, Maps | Small ZIP segments and large ImageStreams |
| September 2026 AFF4-L 2.1 draft | Supported logical storage and metadata subset | Unsupported; the tested independent consumer rejects this output profile |

ImageStream codecs are Stored, Zlib, Snappy, and LZ4. Logical output stores files
below a configurable threshold as Stored or Deflate ZIP segments. The threshold
defaults to 1 MiB and can be set up to 1 GiB. Larger files use
block-hashed ImageStreams, so the threshold is not a file-size limit. Generated
storage identifiers avoid path collisions. Generic exporters may use those
identifiers as filenames.

Logical metadata retains RDF properties for files, folders, and substreams. The
reader accepts draft namespaces and the draft's `dataSteam` spelling. Imported
metadata counts toward the metadata and triple budgets, and remote resources are
never fetched. Inline base64 content and caller-authored ADS or xattr substreams
are limited to 1 KiB.

The following are unsupported: pre-standard AFF4, directory volumes, encryption,
signatures, appended ZIP histories, shifted ZIP offsets, trailing bytes,
ambiguous end records, and ZIP64 extensible end data. ZIP files must be
single-disk, with unshifted offsets and one terminal end record. Files must
remain unchanged while open. A missing referenced member fails when it is read,
and unknown or unreadable ranges fail closed. Full AFF4 and AFF4-L conformance is
not claimed.

## Verification scope

| Library API | What it checks |
| --- | --- |
| `verify` | One selected resource's linear digests |
| `verify_metadata` | Exact uncompressed metadata bytes and available references |
| `verify_all` | Resources, metadata, and supported block/map/index integrity structures |
| `Container::scan_metadata` | Streaming RDF inventory only; no retained graph or integrity verdict |

Linear verification supports MD5, SHA1, SHA256, SHA512, and BLAKE2b-512.
Container verification also checks per-bevy block digests, virtual BlockHashes,
map, index, and path hashes, and SHA256/SHA512 block-map constructions. Coverage
distinguishes stored bytes, explicitly described bytes, and bytes filled for map
gaps. Missing references, mismatches, unreadable resources, and unsupported
constructions are reported explicitly. Legacy `imageStreamHash` values and
signatures are unsupported. Matching internal digests establish consistency, not
independent authenticity.

Writers emit `information.turtle.hashes` with SHA256 and return `metadata_sha256`
so the caller can record it externally. Readers also accept legacy Gemino
`container.hashes` JSON. When both hash files exist, both must agree. Imported
metadata requires hashes in the primary store. Comparisons use the exact member
bytes, not reserialized RDF. The metadata digest does not cover the entire ZIP
file. An independently recorded expected SHA256 provides an external reference.

Sequential verification decodes ZIP content once. A positioned read of a
compressed ZIP member decodes from the start of the member, so random access to
large ZIP-backed files is expensive. ImageStream reads cache one bevy and its
index. Positioned physical disk reads automatically retain 1 MiB logical pages;
other ImageStream reads retain decoded chunks. Both use one byte-bounded LRU
across companion volumes. Verification bypasses payload caches and rereads the
backing data. Metadata scanning retains statement source members and repeated
subjects without building the graph. Full verification uses the bounded graph.

## Multi-volume reads

Supply the primary volume first. The primary volume must contain the complete Map
for the selected image. `VolumeSet` resolves ImageStreams by volume identity. It
rejects conflicting owners or geometry and missing companions, and it retains the
volume context of each graph. Contiguous images reject implicit Map gaps.
Metadata, triples, ZIP directory bytes, and entries share one budget across the
set. Bevy and index scratch data retain only the most recently used volume;
decoded pages and chunks share one volume-aware LRU across the set.

```text
ewf-cli verify-set PRIMARY.aff4 COMPANION.aff4 --image IMAGE_ID --sha256 HASH
ewf-cli verify-set PRIMARY.aff4 COMPANION.aff4 --image IMAGE_ID --full
```

Without `--full`, `verify-set` checks only the SHA256 of the assembled image and
returns exit code 4 if no external digest is supplied. `--full` also checks
per-volume metadata, owned streams, foreign block references, map members,
per-stripe composites, and the composite root of the selected image. Contributor
volumes are used in the supplied order, which is recorded as `stripe_order`. An
incorrect order is reported as a mismatch and is not reordered automatically.

The canonical striped sample matches an independent aff4tools export and
exercises the supported recorded constructions. The sample cannot pass full
verification because it lacks metadata sidecars and uses the unsupported legacy
`imageStreamHash`. Automatic discovery, nested cross-volume Maps, logical
multi-volume containers, and multi-volume writing are unsupported.

## Collection and publication

The portable collector records regular files, folders, available basic
timestamps, and the Unix mode. `LogicalMetadata` can preserve nanosecond
timestamps, roots, and raw path and name bytes. `add_substream` and
`add_case_metadata` accept caller-authored data. The collector does not capture
ACLs, ADS, xattrs, or sparse allocation. The collector never follows links,
requires opt-in to skip entries during discovery, and records
exclusions. Windows non-Unicode paths and names that contain control characters
are rejected. Repeated RDF subjects used for substreams are preserved, even if
another tool flags their ordering.

Read errors, detected source changes, and cancellation prevent publication. These
checks apply to individual files and do not guarantee a consistent snapshot
across files. Payloads stream into a temporary ZIP file beside the destination,
and metadata stays in memory until finalization. Each input has a declared size
and is read for exactly that length. Cancellation is checked between buffers.

CLI collection verifies the finalized temporary container before publishing it
exclusively. `Writer::finish_verified` applies the same policy for library
callers, and `Writer::finish` finalizes without verification. Existing output is
never replaced. The file is synchronized before publication, and on Unix the
parent directory is also synchronized. Splitting and checkpoint resume are
unsupported.

| Outcome | Publication and report |
| --- | --- |
| Verified collection | `published: true`; `verification` contains the staged-container checks |
| Resource or staged verification failure | `published: false`; exit 4; handled temporary output removed |
| Failed/incomplete verification report | `Error::VerificationFailed { report }`; CLI retains report in `verification` |
| Staging error before a report exists | `verification: null` with `error` |
| Parent-directory sync fails after publication | `Error::PublishedButUnsynced` retains output path/digests; CLI exits 4 with published, verified output and a durability error |
| Collection cancellation | Exit 130; final output absent; handled temporary output removed |

A crash can leave inert staging files. If synchronization fails, preserve the
published output and its diagnostics. Process-crash tests do not certify
power-loss durability. Treat diagnostic resource identifiers and paths as case data.

## Resources

`ewf-cli` has no quota options for memory, metadata, entry count, collection
depth, or verification work. For AFF4 operations, `ewf-cli` uses
`Limits::unrestricted()` and leaves memory and disk capacity to the operating
system. There is no RAM-percentage ceiling and no memory reservation at startup.
Checked arithmetic, address-space capacity, format and parser constraints, cycle
detection, and verification still apply.

Payloads stream in chunks. Metadata, indexes, and verification reports use memory
in proportion to the evidence. Large buffer allocations in this crate are
fallible, but allocation failures in the operating system or in dependencies can
still terminate the process. `ewf-cli` does not guarantee graceful recovery from
memory exhaustion.

`Limits::default()` and `CollectionLimits::default()` keep conservative quotas
for embedding applications. Callers can customize them with `open_with_limits`
and `add_directory_tree_with_limits`. Pass the same reader limits to
`finish_verified`. Resource-limit failures prevent publication even when partial
collection is enabled. CLI JSON reports the collection and verification results.

```text
ewf-cli collect SOURCE OUTPUT.aff4
ewf-cli verify OUTPUT.aff4
ewf-cli verify OUTPUT.aff4 --json
```

To compare against an independently recorded digest, use `--metadata-sha256`
for container metadata or `--sha256` for a selected image.

## Exit codes

| Exit | Meaning |
| --- | --- |
| 0 | Requested operation and its required checks completed |
| 1 | Operational failure |
| 2 | Invalid command-line arguments |
| 3 | Extraction or external image-digest mismatch |
| 4 | Failed/incomplete verification, resource rejection, collection omissions, or published-output durability failure |
| 130 | Interrupted |

Metadata-only collections do not require content hashes. Reports explicitly
identify partial collections and extractions that lack some references. Interpret
the exit status together with the publication and verification fields.

## Validation and references

The [testing guide](https://github.com/ebrig/ewf-image/blob/main/docs/testing.md#aff4-interoperability) covers canonical
references, pinned independent aff4tools producer and consumer checks, storage
tests, and benchmarks. These checks do not establish broad compatibility with
commercial tools.

Format references: [AFF4 specification](https://github.com/aff4/Standard) and
[canonical images](https://github.com/aff4/ReferenceImages). AFF4-L is versioned
separately. Compatibility claims apply only to the profiles listed above.

# Testing

Run all checks from the repository root with Rust 1.96 or later. Self-contained
tests use synthetic fixtures. External-tool, corpus, and privileged storage tests
have their own prerequisites. Run relevant checks against the revision being
changed.

## Workspace checks

The root package is the default workspace member, so plain `cargo test` tests
only the EWF library. Add `--workspace` to include AFF4 and the unified CLI.

```sh
cargo fmt --all --check
cargo test -p ewf-image --no-default-features --locked
cargo test -p ewf-image --locked
cargo test --workspace --all-features --locked
cargo test -p ewf-cli --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo check --workspace --examples --all-features --locked
cargo test --workspace --doc --all-features --locked
```

The unified CLI tests cover every physical output and conversion pairing, logical
round trips, sector geometry, metadata omissions, mismatched source references,
destination collisions, and cancellation. The `ewf-cli` package also contains
unit tests for native sources and for EWF history and recovery. These tests use
local files and synthetic images. Physical-device acceptance is a separate gate.

Build the API documentation with warnings treated as errors:

```sh
# POSIX shell
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps --locked
```

```powershell
# PowerShell
$env:RUSTDOCFLAGS = '-D warnings'
cargo doc --workspace --all-features --no-deps --locked
Remove-Item Env:RUSTDOCFLAGS
```

Verify both library packages from a clean candidate:

```sh
cargo package -p ewf-image --list --locked
cargo publish -p ewf-image --dry-run --all-features --locked
cargo package -p aff4-image --list --locked
cargo publish -p aff4-image --dry-run --all-features --locked
```

The dry run does not publish the package. `Cargo.lock` is tracked and included
in the package. To check a tree with uncommitted changes, add `--allow-dirty`.
A result produced with `--allow-dirty` does not establish a clean release
candidate. `aff4-image` is experimental and versioned independently; `ewf-cli`
is not published. The
[local acceptance runner](local-acceptance.md) records these checks and optional
oracle checks in a bundle with revision, log, executable, and artifact hashes.

## External fixtures

External corpus tests are ignored by default and require the `external-fixtures`
feature. Set `EWF_CORPUS_DIR` to one directory, or set `EWF_CORPUS_DIRS` to a
list of directories in the platform's path-list format:

```bash
EWF_CORPUS_DIR=/path/to/images \
cargo test --features external-fixtures --test corpus -- --ignored

EWF_CORPUS_DIRS="/path/to/corpus-a:/path/to/corpus-b" \
cargo test --features external-fixtures --test corpus -- --ignored
```

`EWF_CORPUS_DIRS` takes precedence over `EWF_CORPUS_DIR`.

## Independent EWF tools

When external EWF tools are available, the ignored corpus tests compare decoded
bytes, stable metadata, and verification results:

```bash
EWF_CORPUS_DIR=/path/to/images EWFEXPORT=/usr/local/bin/ewfexport \
cargo test --features external-fixtures --test corpus external_corpus_matches_ewfexport_stdout -- --ignored

EWF_CORPUS_DIR=/path/to/images EWFINFO=/usr/local/bin/ewfinfo \
cargo test --features external-fixtures --test corpus external_corpus_matches_ewfinfo_metadata -- --ignored

EWF_CORPUS_DIR=/path/to/images EWFVERIFY=/usr/local/bin/ewfverify \
cargo test --features external-fixtures --test corpus external_corpus_matches_ewfverify -- --ignored
```

The generated fixture tests require `ewfacquirestream`, `ewfexport`, `ewfinfo`,
and `ewfverify`:

```bash
EWFACQUIRESTREAM=/usr/local/bin/ewfacquirestream \
EWFEXPORT=/usr/local/bin/ewfexport \
EWFINFO=/usr/local/bin/ewfinfo \
EWFVERIFY=/usr/local/bin/ewfverify \
cargo test --features external-fixtures --test corpus ewf_tool_generated_fixture_matrix_matches_oracles -- --ignored --nocapture
```

The opt-in logical corpus test reads every file entry strictly and compares any
stored file MD5 and SHA1. The test also reads non-file entries that carry data
and compares the complete media SHA256 and byte count with independently
computed sidecar files. Keep the images and sidecars outside the repository.

For each first segment, such as `case.L01`, place a `case.L01.media.sha256`
sidecar in the matching relative directory under `EWF_LOGICAL_MEDIA_HASH_DIR`.
Each sidecar contains one line with the lowercase SHA256 and the decimal byte
count, separated by whitespace. Include split sets in the image directory. Set
`EWF_LOGICAL_REQUIRE_SPLIT=1` to require at least one split set. For example:

```bash
EWF_LOGICAL_SINGLE_FILES_DIR=/path/to/images \
EWF_LOGICAL_MEDIA_HASH_DIR=/path/to/independent-hashes \
EWF_LOGICAL_REQUIRE_SPLIT=1 \
cargo test --features external-fixtures --test corpus \
  external_logical_entry_bytes_and_media_match_references -- --ignored --nocapture
```

An additional opt-in gate compares the complete logical entry tree and each
entry's SHA256 with an independently produced JSON Lines manifest. For a first
segment named `case.L01`, place `case.L01.entries.jsonl` in the matching relative
directory under `EWF_LOGICAL_ENTRY_MANIFEST_DIR`. Keep manifests and images
outside the repository. Produce the manifest from the original source files or
an independent reader; do not derive it from this library's parsed entries.

The first line is a header with `schema` set to `ewf-logical-entries-v1`, decimal
`media_size`, lowercase `media_sha256`, decimal `segment_count`, and decimal
`entry_count`. Following lines list every entry in preorder, including the
root. Each has a zero-based `index`, its parent's index (`null` for the root),
`name_utf16` as four lowercase hexadecimal digits per UTF-16 code unit, `kind`
(`file`, `directory`, `unknown`, or `unspecified`), decimal `size`, and lowercase
`sha256` of its content. `size` is the entry's declared size, or zero if none is
declared; an extent alone does not imply content for this manifest. Hash the
empty byte string for zero-size entries. Each line is one JSON object with
exactly these fields. For example:

```jsonl
{"schema":"ewf-logical-entries-v1","media_size":3,"media_sha256":"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad","segment_count":1,"entry_count":2}
{"index":0,"parent":null,"name_utf16":"","kind":"directory","size":0,"sha256":"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"}
{"index":1,"parent":0,"name_utf16":"0061002e007400780074","kind":"file","size":3,"sha256":"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"}
```

Run the independent entry gate with:

```bash
EWF_LOGICAL_SINGLE_FILES_DIR=/path/to/images \
EWF_LOGICAL_ENTRY_MANIFEST_DIR=/path/to/independent-manifests \
EWF_LOGICAL_REQUIRE_SPLIT=1 \
cargo test --features external-fixtures --test corpus \
  external_logical_entries_match_independent_manifests -- --ignored --nocapture
```

`EWF_LOGICAL_REQUIRE_SPLIT=1` requires at least one multi-segment image. A
writer-created split set checks our output against a consumer, while a split
image from another producer is needed for independent reader coverage.

The writer oracle tests compare images created by this library with the output
of external EWF tools:

```bash
EWFEXPORT=/usr/local/bin/ewfexport \
cargo test --features external-fixtures --test corpus external_writer_outputs_match_ewfexport_stdout -- --ignored --nocapture

EWFINFO=/usr/local/bin/ewfinfo \
cargo test --features external-fixtures --test corpus external_writer_metadata_matches_ewfinfo -- --ignored --nocapture
```

External tests are opt-in because they depend on local corpora, tool versions,
and environment configuration. Corpus tests requested with `--ignored` fail if
the configured corpus is missing or empty. The strict closeout tests also require
every documented fixture family.

## Interoperability CI

The interoperability CI job builds libewf **20260924** from its release archive,
checks the archive against a pinned SHA256, and runs the generated-fixture and
writer-consumer comparisons:

```bash
bash scripts/install-libewf-oracle.sh /absolute/path/to/oracle
bash scripts/test-interoperability.sh /absolute/path/to/oracle
```

The scripts require Linux with Bash, a C build toolchain, curl, tar, sha256sum,
and the zlib, BZip2, and OpenSSL development packages. The test script rejects
other tool versions and missing test names. The script checks raw exports,
metadata, range sections, logical catalogs, resumed output, and recovery of
truncated images.

The streaming-acquisition check exports and verifies resumed raw and zlib E01
sets, including sets with an early checkpoint and a short final chunk. Another
required check cancels and resumes raw and zlib acquisitions with injected
unreadable sectors, then uses libewf to check the exported bytes, cumulative
error tables, and digests. The generated matrix includes a SHA256 reference
written by libewf. Proprietary and native fixtures and the full strict closeout
corpus are separate opt-in checks. Fixtures generated by libewf do not establish
compatibility with every producer.

Rust CI runs on Windows, Linux, and macOS, with an additional Linux MSRV job. To
make GitHub block merges or releases without successful checks, repository
administrators must mark the CI jobs as required in branch or release protection.

## AFF4 interoperability

```sh
cargo test -p aff4-image --locked
bash scripts/test-aff4-reference.sh
bash scripts/test-aff4-interoperability.sh
```

The canonical reference check compares media and hashes against independently
produced images. The interoperability script pins aff4tools to
`ed844c0b77a9e3034fa763e7c23dadde4778a9f9` and tests both producer and consumer
directions. Coverage includes physical and logical output, compressible and
incompressible chunks, multiple bevies, and final padding. The canonical fixtures
also exercise every supported leaf digest and a two-volume striped image. These
checks do not establish full AFF4 or AFF4-L conformance or support in commercial tools.

## Regression coverage

| Area | Main tests | Scope |
| --- | --- | --- |
| Reader and sources | `reader`, `sources`, `reader_performance`, `signature` | Format boundaries, positioned backings, caches, and segment access |
| Verification and recovery | `verification`, `recovery`, `encryption` | Digest references, cancellation, corrupt data, provenance, and encrypted vectors |
| Writers | `writer`, `sequential`, `acquisition` | Formats, splitting, padding, resume, resource use, and publication |
| CLI | `ewf-cli` integration and unit tests | Process results, export, collection, history, cancellation, and recovery |
| AFF4 | Sibling crate integration and unit tests | ZIP/RDF limits, streams, volume sets, collection, staged verification, and publication |

Process-exit and injected-I/O tests exercise checkpoint, publication, history,
and recovery-bundle boundaries. These tests do not simulate power loss. CLI tests
include source-read deadlines and cancellation races. Physical-driver behavior
requires separate acceptance testing. Logical-file verification is tested
separately from whole-media checks.

Additional focused checks:

```sh
cargo test -p ewf-cli --test ewf_cli --test aff4_cli --test cli --locked
cargo test --all-features --test recovery external_acquisition_and_truncated_recovery_match_ewfexport -- --ignored
cargo test --release --features parallel --test verification benchmark_verification_workers -- --ignored --nocapture
```

The recovery oracle test requires the libewf tools. The worker benchmark checks
that digests are equal and reports timing without a machine-dependent speed threshold.

## Storage and device acceptance

See [device acceptance](device-acceptance.md) for the Linux loop and
device-mapper and Windows VHDX requirements, ownership checks, commands, and
remaining hardware gaps. Run privileged harnesses on disposable hosts.

The Linux one-shot storage checks require root mount privileges and Python 3.9
or later:

```sh
cargo build --release -p ewf-cli --locked
cargo build --release -p aff4-image --example acquire --locked
sudo python3 scripts/test-sequential-storage.py --binary "$PWD/target/release/ewf-cli"
sudo python3 scripts/test-aff4-storage.py --acquire "$PWD/target/release/examples/acquire" --binary "$PWD/target/release/ewf-cli"
```

These harnesses fill new private tmpfs filesystems, check failure cleanup or
transaction recovery, retry, and compare source hashes. The harnesses do not open
existing physical disks. The EWF2 harness also tests SIGINT. Windows sequential
NTFS and exFAT VHDX acceptance is a separate check that requires elevation.

## Benchmarks

Run the benchmarks on Linux or WSL with Python 3.9 or later. The harnesses create
synthetic data in new temporary directories and remove their own files afterward.
Allow space for the source, the output, and segment scratch files. Use `--help`
to list directory and size options.

```sh
cargo build --release --locked -p ewf-cli
cargo build --release --locked -p ewf-image --example sequential --example catalog_scale
cargo build --release --locked -p aff4-image --example acquire
python3 scripts/benchmark-acquisition.py --binary target/release/ewf-cli --mib 1024 --compression raw
python3 scripts/benchmark-acquisition.py --binary target/release/ewf-cli --mib 1024 --compression zlib
python3 scripts/benchmark-acquisition.py --binary target/release/ewf-cli --mib 2048 --compression raw --repeat-resume
python3 scripts/benchmark-streams.py --release target/release --mib 1024 --files 10000
python3 scripts/benchmark-logical-catalog.py --binary target/release/ewf-cli --files 50000
python3 scripts/benchmark-logical-catalog.py --binary target/release/ewf-cli --files 50000 --aff4
```

The acquisition benchmarks compare the final media SHA256 with an independently
computed source hash and record elapsed time, peak RSS, and sampled scratch
usage. The repeated-resume benchmark measures the cost of validating and
rehashing the growing sealed prefix. The catalog benchmarks check file counts,
source hashes, and reported publication against the filesystem. The CLI runs
with `--json` in these scripts. AFF4 operations do not require budget overrides
for the 50,000-file case, and a nonzero exit status counts as a failed run.

Process RSS excludes kernel caches, and sampled scratch usage is a lower bound.
File count alone does not predict metadata size.

## X-Ways encrypted fixtures

The self-contained test suite uses derived vectors with a public password. Full
readback of the four native fixtures requires a password supplied by the operator:

```powershell
$env:EWF_IMAGE_XWAYS_TEST_PASSWORD = Read-Host 'Fixture password'
try {
    cargo test --test encryption authentic_xways_fixtures_read_with_operator_supplied_password
} finally {
    Remove-Item Env:EWF_IMAGE_XWAYS_TEST_PASSWORD
}
```

The test checks the decoded SHA256 and does not print the password. See the
[fixture provenance](../tests/data/xways-encrypted/README.md) for native and
derived coverage and file hashes. No independent producer has supplied native
uncompressed, verifier-less, or split encrypted images.

## Fuzzing

Run `bash scripts/fuzz-smoke.sh 30` on Linux or WSL for bounded smoke testing of
the EWF and AFF4 parsers. The isolated fuzz workspace uses pinned tooling,
synthetic seeds, input-size limits, process limits, and per-input timeouts. See
the [fuzz guide](https://github.com/ebrig/ewf-image/blob/main/fuzz/README.md) for
setup. Longer campaigns and additional producer seeds require separate acceptance work.

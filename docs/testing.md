# Testing

The default test suite is self-contained and does not require external forensic
images:

```bash
cargo fmt --check
cargo test --no-default-features
cargo test
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --all-features --no-deps
```

Examples should compile before release:

```bash
cargo check --examples --all-features
```

Package and publish dry runs should be clean in a release-ready tree:

```bash
cargo package --list
cargo publish --dry-run
```

For pre-commit local checks on an intentionally dirty tree, add
`--allow-dirty`.

`Cargo.lock` is tracked intentionally and is expected in `cargo package --list`;
Cargo includes lockfiles in packaged crates by default.

## External Fixture Tests

External corpus tests are ignored by default and require the
`external-fixtures` feature. Set `EWF_CORPUS_DIR` to one directory, or
`EWF_CORPUS_DIRS` to a platform path-list of directories:

```bash
EWF_CORPUS_DIR=/path/to/images \
cargo test --features external-fixtures --test corpus -- --ignored

EWF_CORPUS_DIRS="/path/to/corpus-a:/path/to/corpus-b" \
cargo test --features external-fixtures --test corpus -- --ignored
```

`EWF_CORPUS_DIRS` takes precedence over `EWF_CORPUS_DIR`.

## External Tool Oracle Tests

When external EWF tools are available, the ignored corpus tests can compare
decoded bytes, stable metadata, and verification results:

```bash
EWF_CORPUS_DIR=/path/to/images EWFEXPORT=/usr/local/bin/ewfexport \
cargo test --features external-fixtures --test corpus external_corpus_matches_ewfexport_stdout -- --ignored

EWF_CORPUS_DIR=/path/to/images EWFINFO=/usr/local/bin/ewfinfo \
cargo test --features external-fixtures --test corpus external_corpus_matches_ewfinfo_metadata -- --ignored

EWF_CORPUS_DIR=/path/to/images EWFVERIFY=/usr/local/bin/ewfverify \
cargo test --features external-fixtures --test corpus external_corpus_matches_ewfverify -- --ignored
```

Generated external fixture coverage requires `ewfacquirestream`, `ewfexport`,
`ewfinfo`, and `ewfverify`:

```bash
EWFACQUIRESTREAM=/usr/local/bin/ewfacquirestream \
EWFEXPORT=/usr/local/bin/ewfexport \
EWFINFO=/usr/local/bin/ewfinfo \
EWFVERIFY=/usr/local/bin/ewfverify \
cargo test --features external-fixtures --test corpus ewf_tool_generated_fixture_matrix_matches_oracles -- --ignored --nocapture
```

Writer output oracles compare writer-created images against external EWF tools:

```bash
EWFEXPORT=/usr/local/bin/ewfexport \
cargo test --features external-fixtures --test corpus external_writer_outputs_match_ewfexport_stdout -- --ignored --nocapture

EWFINFO=/usr/local/bin/ewfinfo \
cargo test --features external-fixtures --test corpus external_writer_metadata_matches_ewfinfo -- --ignored --nocapture
```

External tests are intentionally opt-in because they depend on local corpora,
tool versions, and environment configuration.

## X-Ways Encrypted Fixtures

The self-contained suite verifies encrypted-marker detection, authentic
`x_encryption` metadata, password rejection, AES-128/AES-256 key and counter
behavior, verifier-less structural validation, and complete reads of
deterministic public-password reference images. Those reads include forward
and reverse random access at AES-block and EWF-chunk boundaries plus the
caller-supplied-reader API.

Four X-Ways-created fixtures additionally cover AES-128/AES-256 crossed with
compatible Deflate and X-Ways Zstandard compression. Their password is not
stored in the repository. Set it only for the test process to run the complete
external-oracle readback:

```powershell
$env:EWF_IMAGE_XWAYS_TEST_PASSWORD = Read-Host "Fixture password"
cargo test --test encryption authentic_xways_fixtures_read_with_operator_supplied_password
Remove-Item Env:EWF_IMAGE_XWAYS_TEST_PASSWORD
```

The test asserts the decoded SHA-256 and does not print the password.

## Reader analysis regression tests

`verification`, `sources`, and `recovery` cover external-reference semantics,
cancellation, suppressed findings, incomplete media, cache isolation, known
encrypted vectors, split images, positioned-source bounds/concurrency,
redundant-table fallback, suspect-data policy, and output alias protection.
The default-only test run covers verification without the optional worker pool.

An additional ignored oracle test creates raw/zlib images with libewf, exports
their media independently, removes the terminal descriptor, and checks recovery
against the original exported bytes:

```bash
cargo test --all-features --test recovery external_acquisition_and_truncated_recovery_match_ewfexport -- --ignored
```

The manual throughput test reports median serial/four-worker verification on
32 MiB synthetic zlib and BZip2 images. It asserts digest equality but sets no
machine-dependent speed threshold:

```bash
cargo test --release --features parallel --test verification benchmark_verification_workers -- --ignored --nocapture
```

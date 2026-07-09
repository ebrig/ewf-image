# Testing

The default test suite is self-contained and does not require external forensic
images:

```bash
cargo fmt --check
cargo test --no-default-features
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

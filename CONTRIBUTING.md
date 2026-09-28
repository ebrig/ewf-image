# Contributing

Keep the public APIs of both libraries documented and testable. EWF and AFF4 are
separate workspace crates with separate format and verification contracts. The
command-line package is `ewf-cli`. The AFF4 crate is experimental and has its
own version and publication checks.

## Development checks

Install Rust 1.96 or later with `rustfmt` and `clippy`, then run:

```sh
rustup component add rustfmt clippy
cargo fmt --all --check
cargo test -p ewf-image --no-default-features --locked
cargo test -p ewf-image --locked
cargo test --workspace --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo check --workspace --examples --all-features --locked
cargo test --workspace --doc --all-features --locked
```

Plain `cargo test` checks only the default EWF member. Add `--workspace` to
include AFF4. Fuzzing uses a separate workspace. [Testing](docs/testing.md)
lists the rustdoc, package, oracle, corpus, storage, and benchmark commands.

For reproducible candidate evidence, run the
[local acceptance runner](docs/local-acceptance.md) from a clean commit. Record
any skipped or blocked checks. An earlier run does not validate new changes.

## Preparing a change

- Keep changes focused and preserve unrelated work.
- Cover changed parser and writer behavior with boundary, malformed-input, and
  independent-tool regressions where applicable. Test CLI process contracts separately.
- Update the relevant user guide, the public API documentation, and the
  Unreleased section of the changelog.
- Describe supported profiles precisely. Distinguish local round trips from
  independent producer and consumer evidence.
- Keep private images, raw dumps, generated corpora, passwords, and case data out of Git.

## Documentation

The README provides orientation. The focused guides define contracts and
commands. Keep dated local measurements, release signoff records, and external
fixture manifests outside Git. Released changelog entries and tracked fixture
hashes are historical records and must not be rewritten.

Document development-only functionality separately from published releases.
Prefer complete examples, and label continuation fragments. Check relative links
and anchors. Add new packaged guides to the `include` list in `Cargo.toml`. Links
to repository-only resources must also work from the published package.

## Native code and evidence handling

Both libraries forbid unsafe Rust. `ewf-cli` permits a narrow Windows native
boundary in `crates/ewf-cli/src/ewf/source/windows/native.rs`. Limit calls in that
module to the intended read-only device operations. Document buffer and handle
invariants, and decode returned bytes in safe Rust. Changes to the module require
relevant parser regressions and elevated VHDX acceptance in addition to the
ordinary Rust checks.

External fixtures are opt-in and must be identified by provenance and hashes.
Use synthetic data for new shareable fixtures, and preserve the labels that
distinguish native fixtures from derived ones. Report vulnerabilities privately
as described in the [security policy](SECURITY.md).

## Releases

Maintainers should follow the [release process](RELEASING.md) for each crate.

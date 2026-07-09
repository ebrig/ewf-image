# Contributing

Thanks for helping improve `ewf`. The package currently exposes a Rust library
crate, so changes should keep the public API small, documented, and directly
testable.

## Setup

Install a current Rust toolchain with `rustfmt` and `clippy`:

```bash
rustup component add rustfmt clippy
```

Run the self-contained checks before opening a pull request:

```bash
cargo fmt --check
cargo test --no-default-features
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo check --examples --all-features
cargo test --doc --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --all-features --no-deps
```

## External Fixture Tests

External corpus tests are ignored by default because they require local EWF
fixtures and external EWF tools. When you change parser, writer, metadata, or
compatibility-sensitive behavior, run the relevant ignored tests described in
[docs/testing.md](docs/testing.md).

## Pull Request Guidelines

- Keep changes focused. Avoid unrelated refactors in compatibility or parser
  changes.
- Add regression coverage for malformed input, format edge cases, and writer
  output behavior.
- Update `README.md`, crate-level rustdoc, and public docs when public behavior
  changes.
- Document intentional limitations in [docs/limitations.md](docs/limitations.md).
- Do not commit private images, generated fixture corpora, or local raw dumps.

## Security Issues

Do not file public issues for suspected vulnerabilities. Follow
[SECURITY.md](SECURITY.md).

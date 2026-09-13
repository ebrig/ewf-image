# Release process

This checklist keeps the source tree, crates.io package, Git tag, and GitHub
release synchronized.

## Prepare

1. Update the version in `Cargo.toml` and refresh `Cargo.lock`.
2. Move the relevant entries from `Unreleased` in `CHANGELOG.md` into a dated
   version section, leaving an empty `Unreleased` section at the top.
3. Update dependency examples and version-specific links in `README.md` and
   other public documentation.
4. Run the checks in [docs/testing.md](docs/testing.md), including
   `cargo package --list` and `cargo publish --dry-run`.
5. Review the packaged files for private fixtures, credentials, workstation
   metadata, and unintended large artifacts.

## Publish

1. Commit the release preparation.
2. Create and push an annotated `vMAJOR.MINOR.PATCH` tag for that commit.
3. Publish with `cargo publish` and confirm the new version on crates.io and
   docs.rs.
4. Create a GitHub release from the same tag. Copy the version's changelog
   section, link the crates.io artifact, and record its SHA-256 checksum.
5. Confirm that GitHub identifies the new release as latest and that CI passed
   for the tagged commit.

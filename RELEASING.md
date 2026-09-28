# Release process

This checklist keeps the source tree, crates.io packages, Git tags, and GitHub
releases synchronized. `ewf-image` and the experimental `aff4-image` are
versioned and validated independently. `ewf-cli` is an unpublished workspace
executable. Keep release evidence for each crate outside the public source tree.

## Prepare

1. Integrate the remote branch and start with a clean, committed candidate.
2. Update the affected package version and `Cargo.lock` when needed. Once its
   release gates pass, move that package's changes from `Unreleased` in
   `CHANGELOG.md` into a version section. Add the release date before the final
   checks on the exact release commit.
3. Update dependency examples and version-specific links in public documentation.
4. Run the checks in [docs/testing.md](docs/testing.md) on the exact candidate,
   including `cargo package --list` and `cargo publish --dry-run` for each crate.
5. Review packaged files for private fixtures, credentials, workstation
   metadata, and unintended large artifacts. Record the checks and any gaps.

## Publish

1. Commit the release preparation and confirm the tree is clean.
2. Create an annotated `vMAJOR.MINOR.PATCH` tag for `ewf-image`, or an annotated
   `aff4-image-vMAJOR.MINOR.PATCH` tag for `aff4-image`, at its validated commit.
3. Push the validated commit and tag, then publish that crate with `cargo publish`.
4. Confirm the version on crates.io and docs.rs. Create a GitHub release from
   the same tag with its changelog entry and the package checksum.
5. Confirm CI passed for the tagged commit and the GitHub release is current.

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

### One-time ewf-image setup

Configure a GitHub trusted publisher under the `ewf-image` crate's crates.io
settings with these exact values:

- Repository owner: `ebrig`
- Repository name: `ewf-image`
- Workflow filename: `publish.yml`
- Environment: `release`

The workflow uses OpenID Connect to obtain a short-lived crates.io token; do not
store a long-lived registry token in GitHub. The GitHub `release` environment
may additionally require reviewers for a manual approval boundary.

### Each release

1. Commit the release preparation and confirm the tree is clean.
2. Confirm the six required CI checks passed on the exact `main` commit to be
   released.
3. Create and push an annotated `vMAJOR.MINOR.PATCH` tag for `ewf-image` at that
   commit. The `Publish ewf-image` workflow verifies that the tag matches the
   package version and current `main`, rechecks every required status from
   `.github/main-branch-protection.json`, runs a publish dry run, and publishes
   through crates.io trusted publishing. It fails closed on a missing, pending,
   failed, or unrecognized check provider.
4. For `aff4-image`, create an annotated
   `aff4-image-vMAJOR.MINOR.PATCH` tag at its validated commit, push the commit
   and tag, and publish that crate manually with `cargo publish`.
5. Confirm the version on crates.io and docs.rs. Create a GitHub release from
   the same tag with its changelog entry and the package checksum.
6. Confirm the automated publish workflow succeeded and the GitHub release is
   current.

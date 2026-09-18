#!/usr/bin/env bash
# Independent implementation is a test oracle, not a library dependency.
set -euo pipefail
directory=$(mktemp -d)
trap 'rm -rf -- "$directory"' EXIT
revision=ed844c0b77a9e3034fa763e7c23dadde4778a9f9
git init -q "$directory/oracle"
git -C "$directory/oracle" fetch --depth 1 https://github.com/jbolas/aff4tools.git "$revision"
git -C "$directory/oracle" checkout --detach FETCH_HEAD
test "$(git -C "$directory/oracle" rev-parse HEAD)" = "$revision"
cargo build --locked --release --no-default-features --manifest-path "$directory/oracle/Cargo.toml" --target-dir "$directory/build"
export AFF4_ORACLE="$directory/build/release/aff4tools"
cargo test -p aff4-image --test writer independent_consumer_exports_and_verifies_writer_output -- --exact --ignored
cargo test -p aff4-image --test writer independent_producer_images_and_files_are_readable -- --exact --ignored

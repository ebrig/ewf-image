#!/usr/bin/env bash
# Bounded local parser checks. Longer campaigns use the same checked-in targets.
set -euo pipefail
seconds=${1:-30}
toolchain=${FUZZ_TOOLCHAIN:-nightly-2026-07-16}
[[ "$seconds" =~ ^[1-9][0-9]*$ ]] || { echo 'Expected positive seconds per target' >&2; exit 2; }
cd "$(dirname "$0")/../fuzz"
cargo run --locked --example seeds -- corpus
for target in ewf_read ewf_catalog aff4_read; do
    cargo +"$toolchain" fuzz run "$target" -- \
        -max_total_time="$seconds" -max_len=1048576 -rss_limit_mb=512 -timeout=10
done

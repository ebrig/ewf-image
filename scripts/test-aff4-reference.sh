#!/usr/bin/env bash
# Download the public canonical reference at an immutable upstream revision.
set -euo pipefail
directory=$(mktemp -d)
trap 'rm -rf -- "$directory"' EXIT
curl --fail --location --retry 3 https://raw.githubusercontent.com/aff4/ReferenceImages/84773b088bf6cce551a515d8ebb486bad69b58b8/AFF4Std/Base-Linear.aff4 --output "$directory/Base-Linear.aff4"
printf '%s  %s\n' bcde3297ae95cd9df214bfb79821334628dad08f21ef38374a2c091481e391c0 "$directory/Base-Linear.aff4" | sha256sum --check -
AFF4_REFERENCE_IMAGE="$directory/Base-Linear.aff4" cargo test -p aff4-image --test reader canonical_reference_matches_producer_hashes -- --exact --ignored
curl --fail --location --retry 3 https://raw.githubusercontent.com/aff4/ReferenceImages/84773b088bf6cce551a515d8ebb486bad69b58b8/AFF4-L/deprecated/dream.aff4 --output "$directory/dream.aff4"
printf '%s  %s\n' ff90ec81dd332509a5535e82dc6d3de2ed5ffa3ec9b6246ace8389281c7cf7eb "$directory/dream.aff4" | sha256sum --check -
AFF4_LOGICAL_REFERENCE_IMAGE="$directory/dream.aff4" cargo test -p aff4-image --test reader canonical_logical_reference_matches_producer_hashes -- --exact --ignored

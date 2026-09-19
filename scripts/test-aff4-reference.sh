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

curl --fail --location --retry 3 https://raw.githubusercontent.com/aff4/ReferenceImages/84773b088bf6cce551a515d8ebb486bad69b58b8/AFF4Std/Base-Linear-AllHashes.aff4 --output "$directory/Base-Linear-AllHashes.aff4"
printf '%s  %s\n' d8f098b1bb51eceb1e913c2389d3fb0a8543323d1db01bf8957e21bdccb1846d "$directory/Base-Linear-AllHashes.aff4" | sha256sum --check -
AFF4_ALL_HASHES_REFERENCE="$directory/Base-Linear-AllHashes.aff4" cargo test --release -p aff4-image --test reader canonical_full_integrity_tree_matches_independent_producer -- --exact --ignored

for part in 1 2; do
    curl --fail --location --retry 3 "https://raw.githubusercontent.com/aff4/ReferenceImages/84773b088bf6cce551a515d8ebb486bad69b58b8/AFF4Std/Striped/Base-Linear_${part}.aff4" --output "$directory/stripe${part}.aff4"
done
printf '%s  %s\n' 56fea0e0b4c94fb7ce780a39129054fe869ee2fcfa77ee7c6ede4830b035c8c8 "$directory/stripe1.aff4" 0d46baa88def85b784caf54a3a6c561e08019fbc22424a21f117d00b90c94505 "$directory/stripe2.aff4" | sha256sum --check -
AFF4_STRIPE1="$directory/stripe1.aff4" AFF4_STRIPE2="$directory/stripe2.aff4" cargo test --release -p aff4-image --test volume_set canonical_striped_image_matches_independent_export -- --exact --ignored

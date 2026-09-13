#!/usr/bin/env bash
# Install the exact independent oracle used by CI, without changing system tools.
set -euo pipefail

prefix=${1:?usage: install-libewf-oracle.sh ABSOLUTE_INSTALL_PREFIX}
[[ "$prefix" = /* ]] || { echo "Install prefix must be absolute" >&2; exit 1; }
version=20260924
digest=e2da4fdb8999b855948863d1bcb11e9427af959e789093491ed94dca397905b5
build_dir=$(mktemp -d)
trap 'rm -rf -- "$build_dir"' EXIT

curl --fail --location --retry 3 \
  "https://github.com/libyal/libewf/releases/download/$version/libewf-experimental-$version.tar.gz" \
  --output "$build_dir/libewf.tar.gz"
printf '%s  %s\n' "$digest" "$build_dir/libewf.tar.gz" | sha256sum --check -
mkdir "$build_dir/source"
tar --extract --gzip --file "$build_dir/libewf.tar.gz" --strip-components=1 --directory "$build_dir/source"
cd "$build_dir/source"
./configure --prefix="$prefix" --disable-shared --disable-python
make -j2
make install
"$prefix/bin/ewfexport" -V

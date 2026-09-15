#!/usr/bin/env bash
# No external corpus is needed: these tests generate fixtures with pinned libewf
# and independently inspect/export the Rust writer's output.
set -euo pipefail

prefix=${1:?usage: test-interoperability.sh ABSOLUTE_LIBEWF_PREFIX}
[[ "$prefix" = /* ]] || { echo "Oracle prefix must be absolute" >&2; exit 1; }
for tool in ewfacquirestream ewfexport ewfinfo ewfverify; do
  version=$("$prefix/bin/$tool" -V 2>&1)
  case "$version" in
    "$tool 20260924"*) ;;
    *) echo "Expected $tool 20260924, got: $version" >&2; exit 1 ;;
  esac
done
export EWFACQUIRESTREAM="$prefix/bin/ewfacquirestream"
export EWFEXPORT="$prefix/bin/ewfexport"
export EWFINFO="$prefix/bin/ewfinfo"
export EWFVERIFY="$prefix/bin/ewfverify"

run_case() {
  local suite=$1 name=$2 listing
  listing=$(cargo test --locked --all-features --test "$suite" -- --list)
  grep --fixed-strings --line-regexp "$name: test" <<< "$listing" >/dev/null || {
    echo "Required oracle test is missing: $suite/$name" >&2; exit 1;
  }
  cargo test --locked --all-features --test "$suite" "$name" -- --exact --ignored --nocapture
}

run_case corpus ewf_tool_generated_fixture_matrix_matches_oracles
run_case corpus external_writer_outputs_match_ewfexport_stdout
run_case corpus external_writer_metadata_matches_ewfinfo
run_case corpus external_writer_range_sections_match_ewfinfo
run_case corpus external_writer_resumed_output_matches_ewf_tools
run_case corpus external_streaming_acquisition_matches_ewf_tools
run_case corpus external_acquisition_bad_sectors_match_ewf_tools
run_case corpus external_writer_logical_single_files_match_ewfinfo
run_case recovery external_acquisition_and_truncated_recovery_match_ewfexport
run_case cli external_cli_resumed_acquisition_matches_libewf

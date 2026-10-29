#!/usr/bin/env bash
# Run one real journey of tests/ against the programs this release build
# staged, as a post-build test of .wisent-release.json.
#
# Usage: tests/release/journey.sh CARGO_TEST_TARGET
#
# The journey drives the staged `brama` (BRAMA) and the staged pinned router
# (ENTITLEMENTS_ROUTER_BIN), never a program compiled again for the test. Its
# test crate is compiled the way src/release/build.sh compiled brama: the
# same projected source, release inputs and compiled dependencies, read from
# the record that script leaves beside the build ($WISENT_OUTPUT_DIR/.build/cargo.sh).
# The journey keeps its evidence in WISENT_TEST_EVIDENCE_DIR, the qualification
# output Stado's build worker retains with the release, not in the build tree
# the next build deletes.
set -euo pipefail

say() {
  printf '%s\n' "$*" >/dev/stderr
}

# Every refusal names its cause and fails the test; `set -e` ends the script
# on the failed function.
refuse() {
  say "$@"
  false
}

target=${1:?name the cargo test target of the journey to run}
output_dir=${WISENT_OUTPUT_DIR:?WISENT_OUTPUT_DIR is required: Stado's build worker names the output of the build under test}
evidence_dir=${WISENT_TEST_EVIDENCE_DIR:?WISENT_TEST_EVIDENCE_DIR is required: Stado's build worker names the qualification output it retains}
record="$output_dir/.build/cargo.sh"
[[ -f "$record" ]] || refuse "the build under test left no cargo record at $record; src/release/build.sh writes it"
# shellcheck source=/dev/null
source "$record"
stage="$output_dir/stage/bin"
for program in brama skarbiec-entitlements-router; do
  [[ -x "$stage/$program" ]] || refuse "the build under test staged no executable $stage/$program"
done

BRAMA="$stage/brama" \
  ENTITLEMENTS_ROUTER_BIN="$stage/skarbiec-entitlements-router" \
  CARGO_TARGET_DIR="$brama_target" \
  WISENT_TEST_EVIDENCE_DIR="$evidence_dir" \
  "$cargo" test ${cargo_overrides[@]+"${cargo_overrides[@]}"} --locked --release \
  --manifest-path "$build_source/Cargo.toml" --test "$target" -- --ignored

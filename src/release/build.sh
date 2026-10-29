#!/usr/bin/env bash
set -euo pipefail
source_dir=${WISENT_SOURCE_DIR:?WISENT_SOURCE_DIR is required}
output_dir=${WISENT_OUTPUT_DIR:?WISENT_OUTPUT_DIR is required}
platform=${WISENT_PLATFORM:?WISENT_PLATFORM is required}
version=${WISENT_VERSION:?WISENT_VERSION is required}
# The message is quoted: an apostrophe in an unquoted ${VAR:?word} opens a
# quote for macOS's bash 3.2, which then swallowed the lines below, and the
# Skarbiec source assignment after them never ran.
source_commit="${WISENT_SOURCE_COMMIT:?WISENT_SOURCE_COMMIT is required: the Stado build worker names the commit it built}"
# `stado release catalog input pin --name <name>` archives under `<name>/`
# and the worker extracts it into WISENT_INPUT_<NAME>_DIR, so each pinned
# repository is WISENT_INPUT_<NAME>_DIR/<name>. The release manifest pins
# skarbiec, so a build without it is a build Stado did not hand this script.
skarbiec_source="${WISENT_INPUT_SKARBIEC_DIR:?WISENT_INPUT_SKARBIEC_DIR is required: pin skarbiec with stado release catalog input pin --name skarbiec}/skarbiec"

# What the build says, on standard error, so standard output stays the build's.
say() {
  printf '%s\n' "$*" >/dev/stderr
}

# Every refusal names its cause and fails the build; `set -e` ends the script
# on the failed function.
refuse() {
  say "$@"
  false
}

if ! command -v cargo >/dev/null; then
  PATH="$HOME/.cargo/bin:$PATH"
  export PATH
fi
command -v cargo >/dev/null || refuse 'cargo is not installed for this builder'
case "$platform" in
  darwin-arm64) expected_os=Darwin; expected_arch=arm64 ;;
  linux-amd64) expected_os=Linux; expected_arch=x86_64 ;;
  *) refuse "unsupported release platform: $platform" ;;
esac
actual_os=$(uname -s)
actual_arch=$(uname -m)
if [[ "$actual_os" != "$expected_os" || "$actual_arch" != "$expected_arch" ]]; then
  refuse "builder $actual_os/$actual_arch cannot produce $platform"
fi
for required in "$source_dir/Cargo.toml" "$source_dir/Cargo.lock" \
  "$skarbiec_source/Cargo.toml" "$source_dir/src/release/bin/start-with-skarbiec"; do
  [[ -f "$required" ]] || refuse "release source is missing $required"
done
declared=$(sed -n '/^version = "/{s/^version = "//;s/"$//;p;q;}' "$source_dir/Cargo.toml")
if [[ "$declared" != "$version" ]]; then
  refuse "WISENT_VERSION $version does not match Cargo.toml version $declared"
fi
locked=$(awk '/^name = "brama"$/ { found = "yes"; next } found && /^version = "/ { gsub(/^version = "|"$/, ""); print; exit }' "$source_dir/Cargo.lock")
if [[ "$locked" != "$version" ]]; then
  refuse "Cargo.lock pins brama $locked but this release is $version; update Cargo.toml and run cargo check"
fi

# Two roots, because they have opposite lifetimes. The scratch root holds what
# belongs to this job — the rewritten source tree — and is deleted on every
# run. The cargo root holds compiled dependencies, which belong to the
# builder: Stado hands every build job a per-product, per-platform
# CARGO_TARGET_DIR (stado-rs/src/cli/release_submit/builds/worker/environment.rs)
# precisely so the next release recompiles only what its commit changed.
#
# A script that points cargo at the scratch root it has just deleted, and
# overrides the handed variable to do it, rebuilds the whole dependency graph
# on every release, twice, and the per-product build cache never comes into
# existence on any builder.
build_root="$output_dir/.build"
stage="$output_dir/stage"
if [[ -n "${CARGO_TARGET_DIR+handed}" ]]; then
  cargo_root=$CARGO_TARGET_DIR
  # The defect this script carried has exactly one shape: a cargo root inside
  # the tree the job deletes. Refuse it here rather than rebuild the
  # dependency graph in silence, because the only visible symptom was a slow
  # release.
  if [[ "$cargo_root" == "$output_dir"* ]]; then
    refuse "the builder handed CARGO_TARGET_DIR $cargo_root inside this job output $output_dir; compiled dependencies there are deleted with the job"
  fi
else
  # A worker without a home hands no cache; it says so itself, and this build
  # compiles every dependency in the job's own tree.
  cargo_root=$build_root
  say "the builder handed no CARGO_TARGET_DIR, so every dependency is compiled under $build_root and deleted with the job"
fi
rm -rf "$build_root" "$stage"
mkdir -p "$build_root" "$cargo_root/brama" "$cargo_root/skarbiec" \
  "$stage/bin" "$stage/libexec" "$stage/etc/brama-skarbiec"
cargo_overrides=()
build_source="$source_dir"
if [[ -n "${WISENT_INPUTS_DIR+handed}" ]]; then
  echo_web_source="${WISENT_INPUT_ECHO_WEB_DIR:?release manifest must supply echo-web}/echo-web"
  wisent_errors_source="${WISENT_INPUT_WISENT_ERRORS_DIR:?release manifest must supply wisent-errors}/wisent-errors"
  stado_wait_source="${WISENT_INPUT_STADO_WAIT_DIR:?release manifest must supply stado-wait}/stado-wait"
  build_source="$build_root/source"
  "$source_dir/src/release/prepare_inputs.sh" "$source_dir" "$build_source" \
    "wisent-onboarding-client=$echo_web_source/crates/onboarding-client" \
    "wisent-errors=$wisent_errors_source/rust" \
    "stado-wait=$stado_wait_source/stado-rs/wait"
  cargo_overrides=(--config "$build_source/release-inputs.toml")
fi
# How this build ran cargo, read by the post-build journeys (tests/release/journey.sh)
# so they compile the tests against the same projected source, release inputs and
# compiled dependencies instead of deciding any of them again.
{
  printf 'cargo=%q\n' "$(command -v cargo)"
  printf 'build_source=%q\n' "$build_source"
  printf 'brama_target=%q\n' "$cargo_root/brama"
  printf 'cargo_overrides=('
  for override in ${cargo_overrides[@]+"${cargo_overrides[@]}"}; do
    printf '%q ' "$override"
  done
  printf ')\n'
} >"$build_root/cargo.sh"

export BRAMA_BUILD_TIMESTAMP="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
export BRAMA_BUILD_PLATFORM="$platform"
export BRAMA_SOURCE_REVISION="$source_commit"
# An empty array reads as unset under nounset in the macOS system bash.
CARGO_TARGET_DIR="$cargo_root/brama" \
  cargo build ${cargo_overrides[@]+"${cargo_overrides[@]}"} --locked --release --bin brama --manifest-path "$build_source/Cargo.toml"
CARGO_TARGET_DIR="$cargo_root/skarbiec" \
  cargo build --locked --release --bin skarbiec --manifest-path "$skarbiec_source/Cargo.toml"
"$cargo_root/brama/release/brama" launcher verbs \
  --router "$cargo_root/skarbiec/release/skarbiec" --launcher "$source_dir/src/release/bin/start-with-skarbiec"

# Programs: read and run by anyone, written by the owner. Data: read by anyone.
program_mode=u=rwx,go=rx
data_mode=u=rw,go=r
install -m "$program_mode" "$cargo_root/brama/release/brama" "$stage/bin/brama"
install -m "$program_mode" "$cargo_root/skarbiec/release/skarbiec" "$stage/bin/skarbiec-entitlements-router"
install -m "$program_mode" "$source_dir/src/release/bin/start-with-skarbiec" "$stage/bin/start-with-skarbiec"
install -m "$program_mode" "$source_dir/src/release/bin/provision-skarbiec-trust" "$stage/bin/provision-skarbiec-trust"
# The launcher stages are sourced by start-with-skarbiec (`. file`), never
# run: they are shell text, staged as data, so nothing reads one as a program
# (the release's command-tree check ran each with --help and refused it).
launcher_root="$source_dir/src/release/bin/launcher"
find "$launcher_root" -type f -print0 | while IFS= read -r -d '' stage_file; do
  relative=${stage_file#"$launcher_root/"}
  install -d "$stage/bin/launcher/$(dirname "$relative")"
  install -m "$data_mode" "$stage_file" "$stage/bin/launcher/$relative"
done
libexec_root="$source_dir/src/release/libexec"
find "$libexec_root" -type f -not -path '*/__pycache__/*' -print0 | while IFS= read -r -d '' asset; do
  relative=${asset#"$libexec_root/"}
  install -d "$stage/libexec/$(dirname "$relative")"
  install -m "$data_mode" "$asset" "$stage/libexec/$relative"
done
install -m "$data_mode" "$source_dir/src/release/etc/brama-skarbiec/recipient-public-keys.asc" "$stage/etc/brama-skarbiec/recipient-public-keys.asc"
install -m "$data_mode" "$source_dir/LICENSE" "$stage/LICENSE"

#!/usr/bin/env bash
set -euo pipefail

source_dir=${WISENT_SOURCE_DIR:?WISENT_SOURCE_DIR is required}
output_dir=${WISENT_OUTPUT_DIR:?WISENT_OUTPUT_DIR is required}
platform=${WISENT_PLATFORM:?WISENT_PLATFORM is required}
version=${WISENT_VERSION:?WISENT_VERSION is required}
skarbiec_source=${WISENT_INPUT_SKARBIEC_DIR:?WISENT_INPUT_SKARBIEC_DIR is required}

# Same reason as the quality gate: the worker's PATH is not a login shell's, and
# the Linux builder keeps cargo under `$HOME/.cargo/bin`.
if ! command -v cargo >/dev/null; then
  PATH="$HOME/.cargo/bin:$PATH"
  export PATH
fi
command -v cargo >/dev/null || {
  printf 'cargo is not installed for this builder\n' >&2
  exit 69
}

case "$platform" in
  darwin-arm64)
    expected_os=Darwin
    expected_arch=arm64
    ;;
  linux-amd64)
    expected_os=Linux
    expected_arch=x86_64
    ;;
  *)
    printf 'unsupported release platform: %s\n' "$platform" >&2
    exit 64
    ;;
esac
actual_os=$(uname -s)
actual_arch=$(uname -m)
if [[ "$actual_os" != "$expected_os" || "$actual_arch" != "$expected_arch" ]]; then
  printf 'builder %s/%s cannot produce %s\n' "$actual_os" "$actual_arch" "$platform" >&2
  exit 65
fi

for required in \
  "$source_dir/Cargo.toml" \
  "$skarbiec_source/Cargo.toml" \
  "$source_dir/scripts/start-with-skarbiec.sh"; do
  if [[ ! -f "$required" ]]; then
    printf 'release source is missing %s\n' "$required" >&2
    exit 66
  fi
done

declared=$(sed -n 's/^version = "\([^"]*\)"$/\1/p' "$source_dir/Cargo.toml" | sed -n '1p')
if [[ "$declared" != "$version" ]]; then
  printf 'WISENT_VERSION %s does not match Cargo.toml version %s\n' "$version" "$declared" >&2
  exit 65
fi

build_root="$output_dir/.build"
stage="$output_dir/stage"
rm -rf "$build_root" "$stage"
mkdir -p "$build_root/brama" "$build_root/skarbiec" \
  "$stage/bin" "$stage/libexec" "$stage/etc/brama-skarbiec"

BRAMA_BUILD_PLATFORM="$platform" \
CARGO_TARGET_DIR="$build_root/brama" \
  cargo build --locked --release --bin brama \
    --manifest-path "$source_dir/Cargo.toml"
CARGO_TARGET_DIR="$build_root/skarbiec" \
  cargo build --locked --release --bin skarbiec \
    --manifest-path "$skarbiec_source/Cargo.toml"

python3 -S "$source_dir/scripts/check-router-verbs.py" \
  "$source_dir/scripts/start-with-skarbiec.sh" \
  "$build_root/skarbiec/release/skarbiec"

install -m 0755 "$build_root/brama/release/brama" "$stage/bin/brama"
install -m 0755 "$build_root/skarbiec/release/skarbiec" \
  "$stage/bin/skarbiec-entitlements-router"
install -m 0755 "$source_dir/scripts/start-with-skarbiec.sh" \
  "$stage/bin/start-with-skarbiec"
install -m 0755 "$source_dir/scripts/provision-skarbiec-trust.sh" \
  "$stage/bin/provision-skarbiec-trust"

for asset in \
  generate-skarbiec-config.mjs \
  brama-diagnose.py \
  brama-subscription-report.py \
  brama-register-workload.py \
  brama-route-probe.py \
  brama-clear-stale-broker.py \
  brama-repair-inference-routes.py; do
  install -m 0644 "$source_dir/scripts/$asset" "$stage/libexec/$asset"
done
install -m 0644 "$source_dir/scripts/skarbiec-recipient-public-keys.asc" \
  "$stage/etc/brama-skarbiec/recipient-public-keys.asc"
install -m 0644 "$source_dir/LICENSE" "$stage/LICENSE"

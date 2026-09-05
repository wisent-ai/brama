#!/usr/bin/env bash
set -euo pipefail

source_dir=${WISENT_SOURCE_DIR:?WISENT_SOURCE_DIR is required}
output_dir=${WISENT_OUTPUT_DIR:?WISENT_OUTPUT_DIR is required}
platform=${WISENT_PLATFORM:?WISENT_PLATFORM is required}
version=${WISENT_VERSION:?WISENT_VERSION is required}
# The release worker materialises the manifest's `skarbiec` input and exports
# WISENT_INPUT_SKARBIEC_DIR; when it is set this build is byte-identical to CI.
# Off a worker nothing sets it, so `:?` made the release build unrunnable on a
# developer machine — the same shape skarbiec-desktop hit with SKARBIEC_ROOT.
# The fallback is the canonical Skarbiec checkout beside this one, and it is not
# a weakening: the `$skarbiec_source/Cargo.toml` check below still refuses (66)
# when neither the input nor the sibling checkout is there.
skarbiec_source=${WISENT_INPUT_SKARBIEC_DIR:-"$source_dir/../skarbiec"}

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

cargo_overrides=()
if [[ -n "${WISENT_INPUTS_DIR:-}" ]]; then
  echo_web_source=${WISENT_INPUT_ECHO_WEB_DIR:?release manifest must supply echo_web}
  wisent_errors_source=${WISENT_INPUT_WISENT_ERRORS_DIR:?release manifest must supply wisent_errors}
  for required in \
    "$source_dir/Cargo.release.lock" \
    "$echo_web_source/crates/onboarding-client/Cargo.toml" \
    "$wisent_errors_source/rust/Cargo.toml"; do
    if [[ ! -f "$required" ]]; then
      printf 'release input is missing %s\n' "$required" >&2
      exit 66
    fi
  done
  cargo_config="$build_root/brama-inputs.toml"
  cat >"$cargo_config" <<EOF
[patch."https://github.com/wisent-ai/echo-web"]
wisent-onboarding-client = { path = "$echo_web_source/crates/onboarding-client" }

[patch."https://github.com/wisent-ai/wisent-errors"]
wisent-errors = { path = "$wisent_errors_source/rust" }
EOF
  cargo_overrides=(--config "$cargo_config")
  normal_lock="$source_dir/Cargo.lock"
  lock_backup="$build_root/Cargo.lock.normal"
  cp "$normal_lock" "$lock_backup"
  restore_normal_lock() {
    cp "$lock_backup" "$normal_lock"
  }
  trap restore_normal_lock EXIT
  cp "$source_dir/Cargo.release.lock" "$normal_lock"
fi

BRAMA_BUILD_PLATFORM="$platform" \
CARGO_TARGET_DIR="$build_root/brama" \
  cargo build "${cargo_overrides[@]}" --locked --release --bin brama \
    --manifest-path "$source_dir/Cargo.toml"
if [[ -n "${WISENT_INPUTS_DIR:-}" ]]; then
  restore_normal_lock
  trap - EXIT
fi
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

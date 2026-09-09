#!/usr/bin/env bash
set -euo pipefail
source_dir=${WISENT_SOURCE_DIR:?WISENT_SOURCE_DIR is required}
output_dir=${WISENT_OUTPUT_DIR:?WISENT_OUTPUT_DIR is required}
platform=${WISENT_PLATFORM:?WISENT_PLATFORM is required}
version=${WISENT_VERSION:?WISENT_VERSION is required}
skarbiec_source=${WISENT_INPUT_SKARBIEC_DIR:-"$source_dir/../skarbiec"}

if ! command -v cargo >/dev/null; then
  PATH="$HOME/.cargo/bin:$PATH"
  export PATH
fi
command -v cargo >/dev/null || { printf 'cargo is not installed for this builder\n' >&2; exit 69; }
case "$platform" in
  darwin-arm64) expected_os=Darwin; expected_arch=arm64 ;;
  linux-amd64) expected_os=Linux; expected_arch=x86_64 ;;
  *) printf 'unsupported release platform: %s\n' "$platform" >&2; exit 64 ;;
esac
actual_os=$(uname -s)
actual_arch=$(uname -m)
if [[ "$actual_os" != "$expected_os" || "$actual_arch" != "$expected_arch" ]]; then
  printf 'builder %s/%s cannot produce %s\n' "$actual_os" "$actual_arch" "$platform" >&2
  exit 65
fi
for required in "$source_dir/Cargo.toml" "$source_dir/Cargo.lock" \
  "$skarbiec_source/Cargo.toml" "$source_dir/src/release/bin/start-with-skarbiec"; do
  [[ -f "$required" ]] || { printf 'release source is missing %s\n' "$required" >&2; exit 66; }
done
declared=$(sed -n 's/^version = "\([^"]*\)"$/\1/p' "$source_dir/Cargo.toml" | sed -n '1p')
if [[ "$declared" != "$version" ]]; then
  printf 'WISENT_VERSION %s does not match Cargo.toml version %s\n' "$version" "$declared" >&2
  exit 65
fi
locked=$(awk '/^name = "brama"$/ { found = 1; next } found && /^version = "/ { gsub(/^version = "|"$/, ""); print; exit }' "$source_dir/Cargo.lock")
if [[ "$locked" != "$version" ]]; then
  printf 'Cargo.lock pins brama %s but this release is %s; update Cargo.toml and run cargo check\n' "$locked" "$version" >&2
  exit 65
fi

build_root="$output_dir/.build"
stage="$output_dir/stage"
rm -rf "$build_root" "$stage"
mkdir -p "$build_root/brama" "$build_root/skarbiec" "$stage/bin" "$stage/libexec" "$stage/etc/brama-skarbiec"
cargo_overrides=()
build_source="$source_dir"
if [[ -n "${WISENT_INPUTS_DIR:-}" ]]; then
  echo_web_source=${WISENT_INPUT_ECHO_WEB_DIR:?release manifest must supply echo_web}
  wisent_errors_source=${WISENT_INPUT_WISENT_ERRORS_DIR:?release manifest must supply wisent_errors}
  build_source="$build_root/source"
  python3 -S "$source_dir/src/release/prepare_inputs.py" "$source_dir" "$build_source" \
    "wisent-onboarding-client=$echo_web_source/crates/onboarding-client" \
    "wisent-errors=$wisent_errors_source/rust"
  cargo_overrides=(--config "$build_source/release-inputs.toml")
fi

BRAMA_BUILD_PLATFORM="$platform" CARGO_TARGET_DIR="$build_root/brama" \
  cargo build "${cargo_overrides[@]}" --locked --release --bin brama --manifest-path "$build_source/Cargo.toml"
CARGO_TARGET_DIR="$build_root/skarbiec" \
  cargo build --locked --release --bin skarbiec --manifest-path "$skarbiec_source/Cargo.toml"
python3 -S "$source_dir/tests/release/check_router_verbs.py" \
  "$build_root/skarbiec/release/skarbiec" "$source_dir/src/release/bin/start-with-skarbiec"

install -m 0755 "$build_root/brama/release/brama" "$stage/bin/brama"
install -m 0755 "$build_root/skarbiec/release/skarbiec" "$stage/bin/skarbiec-entitlements-router"
install -m 0755 "$source_dir/src/release/bin/start-with-skarbiec" "$stage/bin/start-with-skarbiec"
install -m 0755 "$source_dir/src/release/bin/provision-skarbiec-trust" "$stage/bin/provision-skarbiec-trust"
launcher_root="$source_dir/src/release/bin/launcher"
find "$launcher_root" -type f -print0 | while IFS= read -r -d '' stage_file; do
  relative=${stage_file#"$launcher_root/"}
  install -d "$stage/bin/launcher/$(dirname "$relative")"
  install -m 0755 "$stage_file" "$stage/bin/launcher/$relative"
done
libexec_root="$source_dir/src/release/libexec"
find "$libexec_root" -type f -not -path '*/__pycache__/*' -print0 | while IFS= read -r -d '' asset; do
  relative=${asset#"$libexec_root/"}
  install -d "$stage/libexec/$(dirname "$relative")"
  install -m 0644 "$asset" "$stage/libexec/$relative"
done
install -m 0644 "$source_dir/src/release/etc/brama-skarbiec/recipient-public-keys.asc" "$stage/etc/brama-skarbiec/recipient-public-keys.asc"
install -m 0644 "$source_dir/LICENSE" "$stage/LICENSE"

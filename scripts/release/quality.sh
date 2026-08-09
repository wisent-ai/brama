#!/usr/bin/env bash
set -euo pipefail

source_dir=${WISENT_SOURCE_DIR:?WISENT_SOURCE_DIR is required}
: "${WISENT_OUTPUT_DIR:?WISENT_OUTPUT_DIR is required}"
platform=${WISENT_PLATFORM:?WISENT_PLATFORM is required}
version=${WISENT_VERSION:?WISENT_VERSION is required}

case "$platform" in
  darwin-arm64|linux-amd64) ;;
  *) printf 'unsupported release platform: %s\n' "$platform" >&2; exit 64 ;;
esac

manifest="$source_dir/Cargo.toml"
declared=$(sed -n 's/^version = "\([^"]*\)"$/\1/p' "$manifest" | sed -n '1p')
if [[ "$declared" != "$version" ]]; then
  printf 'WISENT_VERSION %s does not match Cargo.toml version %s\n' "$version" "$declared" >&2
  exit 65
fi

cargo fmt --manifest-path "$manifest" -- --check
cargo clippy --locked --all-targets \
  --manifest-path "$manifest" -- -D warnings
python3 "$source_dir/scripts/surface.py" >/dev/null
sh -n "$source_dir/scripts/start-with-skarbiec.sh"
sh -n "$source_dir/scripts/provision-skarbiec-trust.sh"
python3 "$source_dir/scripts/check-launcher-blocks.py" \
  "$source_dir/scripts/start-with-skarbiec.sh"

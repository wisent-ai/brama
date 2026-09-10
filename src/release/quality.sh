#!/usr/bin/env bash
set -euo pipefail

source_dir=${WISENT_SOURCE_DIR:?WISENT_SOURCE_DIR is required}
: "${WISENT_OUTPUT_DIR:?WISENT_OUTPUT_DIR is required}"
platform=${WISENT_PLATFORM:?WISENT_PLATFORM is required}
version=${WISENT_VERSION:?WISENT_VERSION is required}

if ! command -v cargo >/dev/null; then
  PATH="$HOME/.cargo/bin:$PATH"
  export PATH
fi
command -v cargo >/dev/null || {
  printf 'cargo is not installed for this builder\n' >&2
  exit 69
}

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
sh -n "$source_dir/src/release/bin/start-with-skarbiec"
sh -n "$source_dir/src/release/bin/provision-skarbiec-trust"
python3 -S "$source_dir/tests/release/check_launcher_blocks.py" \
  "$source_dir/src/release/bin/start-with-skarbiec"

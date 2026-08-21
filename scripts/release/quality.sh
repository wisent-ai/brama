#!/usr/bin/env bash
set -euo pipefail

source_dir=${WISENT_SOURCE_DIR:?WISENT_SOURCE_DIR is required}
: "${WISENT_OUTPUT_DIR:?WISENT_OUTPUT_DIR is required}"
platform=${WISENT_PLATFORM:?WISENT_PLATFORM is required}
version=${WISENT_VERSION:?WISENT_VERSION is required}

# A release worker is not a login shell: on the fleet's Linux builder `cargo`
# lives in `$HOME/.cargo/bin` and the unit's PATH does not carry it, so this gate
# failed with `cargo: command not found` on a host that has the toolchain. The
# same PATH-shaped gap made a diagnostic probe report the toolchain missing
# entirely, so the fix belongs in the scripts rather than in each host's
# environment.
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
sh -n "$source_dir/scripts/start-with-skarbiec.sh"
sh -n "$source_dir/scripts/provision-skarbiec-trust.sh"
python3 -S "$source_dir/scripts/check-launcher-blocks.py" \
  "$source_dir/scripts/start-with-skarbiec.sh"

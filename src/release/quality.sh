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

# The launcher is an entry point plus the stages it sources; every file of
# that family must parse.
for launcher_file in "$source_dir/src/release/bin/start-with-skarbiec" \
  "$source_dir/src/release/bin/provision-skarbiec-trust" \
  "$source_dir"/src/release/bin/launcher/*.sh \
  "$source_dir"/src/release/bin/launcher/policy/*.sh; do
  sh -n "$launcher_file"
done

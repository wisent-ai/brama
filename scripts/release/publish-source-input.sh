#!/usr/bin/env bash
# Publish one exact Git revision as a deterministic immutable Stado source input.
set -euo pipefail

product="${1:?usage: publish-source-input.sh PRODUCT REPOSITORY REVISION [SUBPATH]}"
repository="${2:?usage: publish-source-input.sh PRODUCT REPOSITORY REVISION}"
revision="${3:?usage: publish-source-input.sh PRODUCT REPOSITORY REVISION}"
subpath=${4:-}
stado_bin="${STADO_BIN:-$HOME/.stado/bin/stado}"
case "$product" in *[!a-z0-9-]*|"") echo "invalid product: $product" >&2; exit 64;; esac
[ -x "$stado_bin" ] || { echo "Stado binary is not executable: $stado_bin" >&2; exit 69; }
git -C "$repository" cat-file -e "$revision^{commit}"
commit="$(git -C "$repository" rev-parse "$revision^{commit}")"
if [[ "$subpath" == /* || "$subpath" == ".." || "$subpath" == ../* || "$subpath" == */.. || "$subpath" == */../* ]]; then
  echo "invalid source subpath: $subpath" >&2
  exit 64
fi
if [[ -n "$subpath" ]]; then
  git -C "$repository" cat-file -e "$commit:$subpath"
fi

work_key=${subpath//\//-}
work="$PWD/.release-inputs/$product-$commit${work_key:+-$work_key}"
mkdir -p "$work"
archive="$work/source.tar.gz"
if [[ -n "$subpath" ]]; then
  git -C "$repository" archive --format=tar "$commit" -- "$subpath" | gzip -9 -n > "$archive"
else
  git -C "$repository" archive --format=tar "$commit" | gzip -9 -n > "$archive"
fi
digest="$(openssl dgst -sha256 "$archive")"
digest="${digest##* }"
uri="stado://sources/$product/$digest/source.tar.gz"

if ! "$stado_bin" storage put "$uri" "$archive" --if-absent; then
  existing="$work/existing.tar.gz"
  rm -f "$existing"
  "$stado_bin" storage get "$uri" "$existing"
  cmp "$archive" "$existing" || {
    echo "immutable source collision: $uri" >&2
    exit 1
  }
fi
printf '%s\n' "$uri"

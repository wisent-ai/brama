#!/usr/bin/env bash
# Project Cargo.lock onto the immutable path dependencies supplied by Stado.
#
# Usage: prepare_inputs.sh SOURCE DESTINATION PACKAGE=DIRECTORY...
#
# Only the selected packages' Git source lines change. Package versions,
# registry checksums and dependency edges stay byte for byte the canonical
# lock's data, so `cargo build --locked` still accepts the resulting graph.
# DESTINATION receives a copy of SOURCE (without .git, target, node_modules,
# .wisent-output and .build), the projected Cargo.lock, and
# release-inputs.toml with one `[patch."<git origin>"]` per package.
set -euo pipefail

source_dir=$(cd -P "${1:?source directory}" && pwd -P)
destination=${2:?destination directory}
shift 2
[ "$#" -gt 0 ] || { echo "at least one PACKAGE=DIRECTORY binding is required" >&2; exit 2; }

bindings=$(mktemp)
trap 'rm -f "$bindings"' EXIT
for binding in "$@"; do
  name=${binding%%=*}
  directory=${binding#*=}
  if [ "$name" = "$binding" ] || [ -z "$name" ] || cut -f1 "$bindings" | grep -qxF -- "$name"; then
    echo "invalid or duplicate Cargo input: $binding" >&2
    exit 1
  fi
  path=$(cd -P "$directory" 2>/dev/null && pwd -P) || { echo "Cargo input $name does not exist: $directory" >&2; exit 1; }
  [ -f "$path/Cargo.toml" ] || { echo "Cargo input $name has no manifest: $path" >&2; exit 1; }
  printf '%s\t%s\n' "$name" "$path" >> "$bindings"
done

mkdir -p "$destination"
(cd "$source_dir" && tar -cf - --exclude .git --exclude target --exclude node_modules \
  --exclude .wisent-output --exclude .build .) | (cd "$destination" && tar -xf -)

awk -F'\t' -v patches="$destination/release-inputs.toml" '
  NR == FNR { wanted[$1] = $2; next }
  function fail(message) { print message > "/dev/stderr"; failed = 1; exit 1 }
  function flush(   i, name, origin, source_at) {
    name = ""; source_at = 0
    for (i = 1; i <= count; i++) {
      if (name == "" && block[i] ~ /^name = "[^"]+"$/) { name = block[i]; sub(/^name = "/, "", name); sub(/"$/, "", name) }
    }
    if (name in wanted) {
      if (name in found) fail("Cargo.lock has several packages named " name)
      for (i = 1; i <= count; i++) if (block[i] ~ /^source = "git\+[^"]+"$/) { source_at = i; break }
      if (!source_at) fail("Cargo.lock does not pin a Git source for " name)
      origin = block[source_at]
      sub(/^source = "git\+/, "", origin); sub(/"$/, "", origin); sub(/[?#].*$/, "", origin)
      printf "%s[patch.\"%s\"]\n%s = { path = \"%s\" }\n", (patched++ ? "\n" : ""), origin, name, wanted[name] > patches
      found[name] = 1
    }
    for (i = 1; i <= count; i++) if (i != source_at) print block[i]
    count = 0
  }
  $0 == "[[package]]" { flush(); print; next }
  { block[++count] = $0 }
  END {
    if (failed) exit 1
    flush()
    for (name in wanted) if (!(name in found)) missing = missing (missing ? ", " : "") name
    if (missing) { print "Cargo.lock has no input packages: " missing > "/dev/stderr"; exit 1 }
  }
' "$bindings" FS='\n' "$source_dir/Cargo.lock" > "$destination/Cargo.lock"

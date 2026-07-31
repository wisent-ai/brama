#!/bin/sh
set -eu
umask u=rwx,go=

: "${STADO_BIN:?STADO_BIN is required}"
: "${STADO_API_URL:?STADO_API_URL is required for publisher authentication}"
: "${STADO_API_TOKEN:?STADO_API_TOKEN is required for publisher authentication}"
: "${STADO_SERVICE_HOST:?STADO_SERVICE_HOST is required}"
: "${STADO_SERVICE_RELEASE_ROOT:?STADO_SERVICE_RELEASE_ROOT is required}"
: "${STADO_RELEASE_VERSION:?STADO_RELEASE_VERSION is required}"
: "${STADO_RELEASE_PLATFORM:?STADO_RELEASE_PLATFORM is required}"
: "${STADO_RELEASE_ARCHIVE:?STADO_RELEASE_ARCHIVE is required}"
: "${STADO_RELEASE_SHA256:?STADO_RELEASE_SHA256 is required}"
: "${STADO_RELEASE_PROVENANCE:?STADO_RELEASE_PROVENANCE is required}"
for path_component in "$STADO_RELEASE_VERSION" "$STADO_RELEASE_PLATFORM"; do
  case "$path_component" in
    .|..|*[![:alnum:]._-]*)
      printf '%s\n' 'release version and platform must be path-safe' >/dev/stderr
      false
      ;;
  esac
done

if [ ! -x "$STADO_BIN" ]; then
  printf '%s\n' "STADO_BIN is not executable: $STADO_BIN" >/dev/stderr
  false
fi
if [ ! -f "$STADO_RELEASE_ARCHIVE" ]; then
  printf '%s\n' "STADO_RELEASE_ARCHIVE is not a regular file: $STADO_RELEASE_ARCHIVE" >/dev/stderr
  false
fi
if [ ! -f "$STADO_RELEASE_SHA256" ]; then
  printf '%s\n' "STADO_RELEASE_SHA256 is not a regular file: $STADO_RELEASE_SHA256" >/dev/stderr
  false
fi
if [ ! -f "$STADO_RELEASE_PROVENANCE" ]; then
  printf '%s\n' "STADO_RELEASE_PROVENANCE is not a regular file: $STADO_RELEASE_PROVENANCE" >/dev/stderr
  false
fi
(CDPATH= cd -- "$(dirname -- "$STADO_RELEASE_ARCHIVE")" && \
  sha256sum --check "$(basename -- "$STADO_RELEASE_SHA256")")
case "$STADO_SERVICE_RELEASE_ROOT" in
  /*) ;;
  *)
    printf '%s\n' 'STADO_SERVICE_RELEASE_ROOT must be an absolute target-host path' >/dev/stderr
    false
    ;;
esac

# This script intentionally stages only on its local filesystem. The workflow
# runner must therefore be the registered STADO_SERVICE_HOST. Operators using a
# different runner must first materialize this immutable object on that host
# through Stado's approved object/machine path, then invoke `stado service
# deploy` there. Ad-hoc shell and provider CLI fallbacks are not supported.
release_uri="stado://releases/brama/$STADO_RELEASE_VERSION/$STADO_RELEASE_PLATFORM/brama-runtime.tar.gz"
checksum_uri="stado://releases/brama/$STADO_RELEASE_VERSION/$STADO_RELEASE_PLATFORM/brama-runtime.tar.gz.sha256"
provenance_uri="stado://releases/brama/$STADO_RELEASE_VERSION/$STADO_RELEASE_PLATFORM/provenance.json"
target_root="${STADO_SERVICE_RELEASE_ROOT%/}/$STADO_RELEASE_VERSION/$STADO_RELEASE_PLATFORM"
target_program="$target_root/bin/start-with-skarbiec"
staging_root="${STADO_SERVICE_RELEASE_ROOT%/}/.$STADO_RELEASE_VERSION.staging.$$"

"$STADO_BIN" storage put \
  "$release_uri" \
  "$STADO_RELEASE_ARCHIVE" \
  --if-absent \
  --content-type application/gzip
"$STADO_BIN" storage put \
  "$checksum_uri" \
  "$STADO_RELEASE_SHA256" \
  --if-absent \
  --content-type text/plain
"$STADO_BIN" storage put \
  "$provenance_uri" \
  "$STADO_RELEASE_PROVENANCE" \
  --if-absent \
  --content-type application/json

if [ -e "$target_root" ]; then
  printf '%s\n' "refusing to replace immutable staged release: $target_root" >/dev/stderr
  false
fi
mkdir -p "$(dirname -- "$target_root")"
rm -rf "$staging_root"
mkdir "$staging_root"
trap 'rm -rf "$staging_root"' EXIT HUP INT TERM
tar -C "$staging_root" -xzf "$STADO_RELEASE_ARCHIVE"
for executable in \
  "$staging_root/bin/brama" \
  "$staging_root/bin/skarbiec-entitlements-router" \
  "$staging_root/bin/stado" \
  "$staging_root/bin/start-with-skarbiec"
do
  if [ ! -x "$executable" ]; then
    printf '%s\n' "release is missing an executable: $executable" >/dev/stderr
    false
  fi
done
mv "$staging_root" "$target_root"
trap - EXIT HUP INT TERM

"$STADO_BIN" service deploy brama \
  --host "$STADO_SERVICE_HOST" \
  --from "$target_program"
printf '%s\n' "deployed brama from $release_uri on $STADO_SERVICE_HOST"

#!/bin/sh
set -eu
umask u=rwx,go=

: "${BRAMA_BINARY:?BRAMA_BINARY is required}"
: "${SKARBIEC_ROUTER_BINARY:?SKARBIEC_ROUTER_BINARY is required}"
: "${STADO_BIN:?STADO_BIN is required}"
: "${STADO_RELEASE_VERSION:?STADO_RELEASE_VERSION is required}"
: "${STADO_RELEASE_PLATFORM:?STADO_RELEASE_PLATFORM is required}"
: "${STADO_RELEASE_ARCHIVE:?STADO_RELEASE_ARCHIVE is required}"
: "${STADO_SERVICE_RELEASE_ROOT:?STADO_SERVICE_RELEASE_ROOT is required}"
: "${BRAMA_PRODUCT_VERSION:?BRAMA_PRODUCT_VERSION is required}"
: "${BRAMA_SOURCE_REVISION:?BRAMA_SOURCE_REVISION is required}"
: "${BRAMA_BUILD_TIMESTAMP:?BRAMA_BUILD_TIMESTAMP is required}"
: "${BRAMA_BUILDER_IDENTITY:?BRAMA_BUILDER_IDENTITY is required}"

case "$STADO_RELEASE_VERSION" in
  .|..|*[![:alnum:]._-]*)
    printf '%s\n' 'STADO_RELEASE_VERSION must be path-safe' >/dev/stderr
    false
    ;;
esac
case "$STADO_RELEASE_PLATFORM" in
  .|..|*[![:alnum:]._-]*)
    printf '%s\n' 'STADO_RELEASE_PLATFORM must be path-safe' >/dev/stderr
    false
    ;;
esac
case "$STADO_SERVICE_RELEASE_ROOT" in
  /*) ;;
  *)
    printf '%s\n' 'STADO_SERVICE_RELEASE_ROOT must be an absolute target-host path' >/dev/stderr
    false
    ;;
esac

for executable in "$BRAMA_BINARY" "$SKARBIEC_ROUTER_BINARY" "$STADO_BIN"; do
  if [ ! -x "$executable" ]; then
    printf '%s\n' "release input is not executable: $executable" >/dev/stderr
    false
  fi
done
if [ -e "$STADO_RELEASE_ARCHIVE" ]; then
  printf '%s\n' "refusing to replace release archive: $STADO_RELEASE_ARCHIVE" >/dev/stderr
  false
fi

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
target_root="${STADO_SERVICE_RELEASE_ROOT%/}/$STADO_RELEASE_VERSION/$STADO_RELEASE_PLATFORM"
staging_dir=$(mktemp -d "${TMPDIR:-/tmp}/brama-stado-release.XXXXXX")
trap 'rm -rf "$staging_dir"' EXIT HUP INT TERM

mkdir -p "$staging_dir/bin" "$staging_dir/etc/brama-skarbiec"
install -m u=rwx,go= "$BRAMA_BINARY" "$staging_dir/bin/brama"
install -m u=rwx,go= "$SKARBIEC_ROUTER_BINARY" "$staging_dir/bin/skarbiec-entitlements-router"
install -m u=rwx,go= "$STADO_BIN" "$staging_dir/bin/stado"
install -m u=rwx,go= "$repo_root/scripts/start-with-skarbiec.sh" "$staging_dir/bin/start-with-skarbiec"
install -m u=rw,go= "$repo_root/scripts/skarbiec-subscriptions.json" "$staging_dir/etc/brama-skarbiec/subscriptions.json"
install -m u=rw,go= "$repo_root/scripts/skarbiec-recipient-public-keys.asc" "$staging_dir/etc/brama-skarbiec/recipient-public-keys.asc"

set -- $(sha256sum "$repo_root/Cargo.lock")
dependency_lock_digest=$1
archive_name=$(basename -- "$STADO_RELEASE_ARCHIVE")
mkdir -p "$staging_dir/share/brama"
node - \
  "$staging_dir/share/brama/provenance.json" \
  "$BRAMA_PRODUCT_VERSION" \
  "$BRAMA_SOURCE_REVISION" \
  "$STADO_RELEASE_PLATFORM" \
  "$BRAMA_BUILD_TIMESTAMP" \
  "$archive_name" \
  "$BRAMA_BUILDER_IDENTITY" \
  "$dependency_lock_digest" \
  "" <<'NODE'
const [, , outputPath, productVersion, sourceRevision, platform, builtAt, archiveFilename, builderIdentity, dependencyLockDigest, artifactDigest] = process.argv;
const provenance = {
  product: "brama",
  product_version: productVersion,
  source_revision: sourceRevision,
  platform,
  built_at: builtAt,
  archive_filename: archiveFilename,
  builder_identity: builderIdentity,
  dependency_lock_sha256: dependencyLockDigest,
  artifact_sha256: artifactDigest || null,
};
require("node:fs").writeFileSync(outputPath, JSON.stringify(provenance) + "\n", { mode: Number.parseInt("600", "8") });
NODE

service_uid=${BRAMA_SERVICE_UID:-$(id -u)}
service_gid=${BRAMA_SERVICE_GID:-$(id -g)}
node "$repo_root/scripts/generate-skarbiec-config.mjs" \
  "$staging_dir/bin/brama" \
  "$staging_dir/etc/brama-skarbiec" \
  "$repo_root/scripts/skarbiec-subscriptions.json" \
  "$target_root/bin/brama" \
  "$service_uid" \
  "$service_gid"

mkdir -p "$(dirname -- "$STADO_RELEASE_ARCHIVE")"
tar -C "$staging_dir" -czf "$STADO_RELEASE_ARCHIVE" .
printf '%s\n' "$STADO_RELEASE_ARCHIVE"
set -- $(sha256sum "$STADO_RELEASE_ARCHIVE")
artifact_digest=$1
checksum_file="$STADO_RELEASE_ARCHIVE.sha256"
provenance_file="$STADO_RELEASE_ARCHIVE.provenance.json"
printf '%s  %s\n' "$artifact_digest" "$archive_name" >"$checksum_file"
node - \
  "$provenance_file" \
  "$BRAMA_PRODUCT_VERSION" \
  "$BRAMA_SOURCE_REVISION" \
  "$STADO_RELEASE_PLATFORM" \
  "$BRAMA_BUILD_TIMESTAMP" \
  "$archive_name" \
  "$BRAMA_BUILDER_IDENTITY" \
  "$dependency_lock_digest" \
  "$artifact_digest" <<'NODE'
const [, , outputPath, productVersion, sourceRevision, platform, builtAt, archiveFilename, builderIdentity, dependencyLockDigest, artifactDigest] = process.argv;
const provenance = {
  product: "brama",
  product_version: productVersion,
  source_revision: sourceRevision,
  platform,
  built_at: builtAt,
  archive_filename: archiveFilename,
  builder_identity: builderIdentity,
  dependency_lock_sha256: dependencyLockDigest,
  artifact_sha256: artifactDigest,
};
require("node:fs").writeFileSync(outputPath, JSON.stringify(provenance) + "\n", { mode: Number.parseInt("600", "8") });
NODE
printf '%s\n' "$checksum_file"
printf '%s\n' "$provenance_file"

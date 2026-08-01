#!/bin/sh
set -eu
umask u=rwx,go=

release_marker=${BRAMA_RELEASE_MARKER:-"$HOME/.stado/brama-release-version"}
[ -f "$release_marker" ] || { printf '%s\n' "missing Brama release marker: $release_marker" >/dev/stderr; false; }
IFS= read -r release_version <"$release_marker"
case "$release_version" in
  .|..|*[![:alnum:]._-]*) printf '%s\n' 'invalid Brama release marker' >/dev/stderr; false ;;
esac
platform=${BRAMA_RELEASE_PLATFORM:-linux-x86_64}
release_archive="$HOME/.stado/releases/brama/$release_version/$platform/brama.tar.gz"
vault_archive="$HOME/.stado/releases/brama-vault/$release_version/$platform/brama-vault.tar.gz"
release_root="$HOME/.stado/services/brama/releases/$release_version/$platform"
vault_root="$HOME/.stado/services/brama/vaults/$release_version"

[ -f "$release_archive" ] || { printf '%s\n' "missing Brama release: $release_archive" >/dev/stderr; false; }
[ -f "$vault_archive" ] || { printf '%s\n' "missing Brama vault release: $vault_archive" >/dev/stderr; false; }

if [ ! -d "$release_root" ]; then
  release_staging="$release_root.staging.$$"
  mkdir -p "$(dirname "$release_root")" "$release_staging"
  trap 'rm -rf "$release_staging"' EXIT HUP INT TERM
  tar -C "$release_staging" -xzf "$release_archive"
  for executable in brama skarbiec-entitlements-router stado start-with-skarbiec; do
    [ -x "$release_staging/bin/$executable" ] || {
      printf '%s\n' "release is missing executable: $executable" >/dev/stderr
      false
    }
  done
  mv "$release_staging" "$release_root"
  trap - EXIT HUP INT TERM
fi

if [ ! -d "$vault_root" ]; then
  vault_staging="$vault_root.staging.$$"
  mkdir -p "$(dirname "$vault_root")" "$vault_staging"
  trap 'rm -rf "$vault_staging"' EXIT HUP INT TERM
  tar -C "$vault_staging" -xzf "$vault_archive"
  [ -f "$vault_staging/skarbiec.vault.json" ] || {
    printf '%s\n' 'vault release is missing skarbiec.vault.json' >/dev/stderr
    false
  }
  chmod u=rw,go= "$vault_staging/skarbiec.vault.json"
  mv "$vault_staging" "$vault_root"
  trap - EXIT HUP INT TERM
fi

config_dir="$HOME/.config/brama"
mkdir -p "$config_dir" "$HOME/.stado/run/brama"
chmod u=rwx,go= "$config_dir" "$HOME/.stado/run/brama"
cat >"$config_dir/service.env" <<EOF
BRAMA_SECRET_SOURCE=local-vault
BRAMA_GNUPG_HOME=$HOME/.stado/brama-gnupg
SKARBIEC_VAULT_FILE=$vault_root/skarbiec.vault.json
BRAMA_CONTROL_CONFIG=$HOME/.stado/brama-control-config.json
BRAMA_RUNTIME_DIR=$HOME/.stado/run/brama
EOF
chmod u=rw,go= "$config_dir/service.env"
printf '%s\n' "$release_root/bin/start-with-skarbiec"

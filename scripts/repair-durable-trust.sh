#!/bin/sh
# Put this host's Skarbiec trust material where a release upgrade cannot take it
# away, and make the running release use it.
#
# Trust material used to be provisioned inside the release directory, so every
# `stado service update` produced a gateway with an empty `etc/brama-skarbiec`:
# no workload registry, therefore no proof the broker recognises, therefore
# every capability redemption refused while `/health` kept answering `ok`. The
# launcher already prefers a durable directory outside the release and already
# knows how to provision into it; it skipped doing so because the inputs it
# checks for ship inside the release.
#
# This performs the same repair for a host whose running release predates that
# fix: seed the durable directory from the release, provision the current digest
# into it, and register the workload key with the vault. It is idempotent and
# never mints a second identity: the durable key is passed in explicitly.
set -eu

service_dir="$HOME/.stado/services/brama"
current=$(cd -- "$service_dir/current" && pwd -P)
architecture="$current/darwin-arm"
[ -d "$architecture" ] || architecture="$current"
config_dir="$HOME/.config/brama/trust"
stable_proof_key="$HOME/.stado/brama-proof.key"

printf 'running release: %s\n' "$(basename -- "$current")"
printf 'durable trust:   %s\n' "$config_dir"

mkdir -p "$config_dir"
chmod u=rwx,go= "$config_dir"
for seed in recipient-public-keys.asc; do
  source_file="$architecture/etc/brama-skarbiec/$seed"
  if [ -f "$source_file" ] && [ ! -f "$config_dir/$seed" ]; then
    cp "$source_file" "$config_dir/$seed"
    printf 'seeded: %s\n' "$seed"
  fi
done
# No subscriptions manifest is seeded or required: which subscriptions exist is
# read off the vault items themselves, by the same tags the gateway reads.

# Provisioning signs the policy and registry with Node, and a helper runs with a
# minimal PATH. Resolve it the way the launcher does -- from the service
# environment first, because that is the interpreter the service itself uses.
if [ -z "${NODE_BIN:-}" ] && [ -f "$HOME/.config/brama/service.env" ]; then
  NODE_BIN=$(sed -n 's/^NODE_BIN=//p' "$HOME/.config/brama/service.env" \
    | tr -d '"' | tr -d "'" | tail -1)
fi
if [ -z "${NODE_BIN:-}" ] || ! [ -x "$NODE_BIN" ]; then
  for candidate in /opt/homebrew/bin/node /usr/local/bin/node "$HOME"/.nvm/versions/node/*/bin/node; do
    if [ -x "$candidate" ]; then
      NODE_BIN="$candidate"
      break
    fi
  done
fi
if [ -z "${NODE_BIN:-}" ] || ! [ -x "$NODE_BIN" ]; then
  printf '%s\n' "no Node interpreter found for signing the policy and registry" >/dev/stderr
  false
fi
export NODE_BIN
printf 'node:            %s\n' "$NODE_BIN"

# --force is right here and only here: the durable directory may hold a registry
# pinned to a previous digest, which is exactly what must be re-pinned. The key
# survives because it is passed in.
BRAMA_SKARBIEC_CONFIG_DIR="$config_dir" \
BRAMA_BIN="$architecture/bin/brama" \
BRAMA_PROOF_KEY_FILE="$stable_proof_key" \
BRAMA_WORKLOAD_UID="$(id -u)" \
BRAMA_WORKLOAD_GID="$(id -g)" \
  "$architecture/bin/provision-skarbiec-trust" --force

if [ ! -f "$stable_proof_key" ] && [ -f "$config_dir/brama-proof.key" ]; then
  cp "$config_dir/brama-proof.key" "$stable_proof_key"
  chmod u=rw,go= "$stable_proof_key"
  printf 'recorded the durable workload key at %s\n' "$stable_proof_key"
fi

printf -- '--- registering the workload with the vault ---\n'
BRAMA_SKARBIEC_CONFIG_DIR="$config_dir" \
  /usr/bin/env python3 "$architecture/libexec/brama-register-workload.py"

printf -- '--- what the durable directory now holds ---\n'
ls "$config_dir"

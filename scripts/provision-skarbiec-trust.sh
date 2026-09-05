#!/bin/sh
# Generate this installation's Skarbiec trust material.
#
# Run once per installation, before the first start. It produces the signed
# policy, the workload registry, the trust root, the WORM receipt command and
# the workload proof key that `start-with-skarbiec` consumes.
#
# Why this is not done when the release is built: the registry pins the exact
# executable path and SHA-256 of the binary that is allowed to redeem a
# capability, and a build machine does not know where the artifact will be
# installed. Generating it here also means the proof key belongs to this
# installation instead of being the same secret in every copy of the archive.
#
# Usage:
#   bin/provision-skarbiec-trust [--force]
#
# Environment:
#   BRAMA_SKARBIEC_CONFIG_DIR  where to write (default: the bundle's etc dir)
#   BRAMA_BIN                  the brama binary the registry should pin
#   NODE_BIN                   node executable (default: node)
#   BRAMA_WORKLOAD_UID/GID     numeric owner the broker will require
#                              (default: the account running this script)
set -eu
umask 077

force=
for argument in "$@"; do
  case "$argument" in
    --force) force=yes ;;
    *)
      printf '%s\n' "unknown argument: $argument" >/dev/stderr
      printf '%s\n' "usage: provision-skarbiec-trust [--force]" >/dev/stderr
      false
      ;;
  esac
done

# `pwd -P`, so the path this pins is the one the kernel will report for the
# running gateway. Invoked through `.../brama/current/...`, a logical `pwd`
# records the alias, and the broker then compares it against the physical
# digest directory and refuses every redemption as a peer mismatch.
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)
bundle_root=$(CDPATH= cd -- "$script_dir/.." && pwd -P)

if [ -f "$bundle_root/libexec/generate-skarbiec-config.mjs" ]; then
  generator="$bundle_root/libexec/generate-skarbiec-config.mjs"
  default_config_dir="$bundle_root/etc/brama-skarbiec"
  default_brama_bin="$bundle_root/bin/brama"
elif [ -f "$bundle_root/scripts/generate-skarbiec-config.mjs" ]; then
  generator="$bundle_root/scripts/generate-skarbiec-config.mjs"
  default_config_dir="$bundle_root/etc/brama-skarbiec"
  default_brama_bin="$bundle_root/target/release/brama"
else
  printf '%s\n' "cannot find generate-skarbiec-config.mjs next to $script_dir" >/dev/stderr
  false
fi

config_dir=${BRAMA_SKARBIEC_CONFIG_DIR:-"$default_config_dir"}
brama_bin=${BRAMA_BIN:-"$default_brama_bin"}
NODE_BIN=${NODE_BIN:-node}

command -v "$NODE_BIN" >/dev/null 2>&1 || {
  printf '%s\n' "NODE_BIN is not executable: $NODE_BIN" >/dev/stderr
  printf '%s\n' "Provisioning needs Node to sign the policy and registry." >/dev/stderr
  false
}
[ -x "$brama_bin" ] || {
  printf '%s\n' "not an executable brama binary: $brama_bin" >/dev/stderr
  printf '%s\n' "Set BRAMA_BIN to the binary this installation will run." >/dev/stderr
  false
}

# No subscriptions manifest is read or required. Which subscriptions exist is a
# fact about the vault, and the generator reads it there: every item
# `provider:<provider>:brama-sub-<agent>-*` is one, which is the same rule the
# gateway itself uses to discover them. A hand-written list beside that rule could
# only ever disagree with it, and did -- a paid Claude account was missing from the
# list and therefore from the policy, so the gateway could not use it.
control_config=${BRAMA_CONTROL_CONFIG:-}
if [ -z "$control_config" ]; then
  for candidate in \
    "${HOME:-/nonexistent}/.config/brama/control.json" \
    "${HOME:-/nonexistent}/.config/stado/config.json"
  do
    if [ -f "$candidate" ]; then
      control_config=$candidate
      break
    fi
  done
fi
if [ -n "$control_config" ] && [ ! -f "$control_config" ]; then
  printf '%s\n' "BRAMA_CONTROL_CONFIG is not a regular file: $control_config" >/dev/stderr
  false
fi

# The proof key is the one file whose presence means this installation already
# has an identity. Overwriting it silently would strand every capability the
# broker has already bound to the old key.
if [ -f "$config_dir/brama-proof.key" ] && [ -z "$force" ]; then
  printf '%s\n' "this installation already has trust material in $config_dir" >/dev/stderr
  printf '%s\n' "Re-provisioning replaces its workload identity. Pass --force if" >/dev/stderr
  printf '%s\n' "that is what you mean, and expect to re-grant its capabilities." >/dev/stderr
  false
fi

mkdir -p "$config_dir"
chmod u=rwx,go= "$config_dir"

# The absolute runtime path is passed explicitly: the registry pins it, and the
# broker compares it against the process that asks to redeem.
runtime_bin=$(CDPATH= cd -- "$(dirname -- "$brama_bin")" && pwd)/$(basename -- "$brama_bin")

# So are the uid and gid, always. The generator's own default is the container
# account the image creates, which exists nowhere else; leaving it in place on
# a host installation pins a workload nobody can be. The broker then refuses
# every redemption with `peer mismatch` while the path and digest it names in
# the same record match perfectly, which is why this cost days to find. An
# installation is provisioned by the account that will run it, so that account
# is the honest default and the variables stay as the override for the cases
# where it is not.
# And so is the durable workload key, by environment rather than argument. The
# key that proves the workload belongs to Brama on this host, not to one
# installation of it: the registry re-pins path, digest and account freely, but a
# new key needs a fresh vault grant, and the service cannot authorise one because
# the vault is encrypted to the owner. Carrying the key across installations is
# what lets the operator's single registration keep working.
# The launcher's location is canonical, because the launcher is what runs on
# every start and exports it. This default disagreed with it, so a provision
# run outside the launcher minted a second identity in a second place and the
# vault kept the public half of a key nothing signed with. Prefer the canonical
# file, keep the older path as a fallback for installations that still hold it.
canonical_proof_key="${HOME:-/nonexistent}/.stado/brama-proof.key"
legacy_proof_key="${HOME:-/nonexistent}/.config/brama/brama-proof.key"
if [ -z "${BRAMA_PROOF_KEY_FILE:-}" ]; then
  if [ -f "$canonical_proof_key" ] || [ ! -f "$legacy_proof_key" ]; then
    BRAMA_PROOF_KEY_FILE="$canonical_proof_key"
  else
    BRAMA_PROOF_KEY_FILE="$legacy_proof_key"
  fi
fi
export BRAMA_PROOF_KEY_FILE
set -- "$brama_bin" "$config_dir" "$runtime_bin" \
  "${BRAMA_WORKLOAD_UID:-$(id -u)}" "${BRAMA_WORKLOAD_GID:-$(id -g)}"
if [ -n "$control_config" ]; then
  set -- "$@" "$control_config"
fi
"$NODE_BIN" "$generator" "$@"

printf '%s\n' "provisioned Skarbiec trust material for $runtime_bin in $config_dir"

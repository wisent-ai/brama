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

subscriptions="$config_dir/subscriptions.json"
[ -f "$subscriptions" ] || {
  printf '%s\n' "missing subscriptions manifest: $subscriptions" >/dev/stderr
  false
}

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
set -- "$brama_bin" "$config_dir" "$subscriptions" "$runtime_bin" \
  "${BRAMA_WORKLOAD_UID:-$(id -u)}" "${BRAMA_WORKLOAD_GID:-$(id -g)}"
"$NODE_BIN" "$generator" "$@"

printf '%s\n' "provisioned Skarbiec trust material for $runtime_bin in $config_dir"

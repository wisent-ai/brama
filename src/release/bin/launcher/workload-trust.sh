provision_and_register_workload() {
# The trust material below is per installation. It pins the absolute path,
# SHA-256, uid and gid of the process allowed to redeem a capability, so
# material generated anywhere but this installation describes somebody else and
# the broker refuses every redemption with `peer mismatch` - while the gateway
# answers /health, which is what made this take days to see.
#
# Two things put a foreign registry here. An archive built with the material
# baked in carries the build machine's stage path and its container account.
# And an installation copied to a new directory - which is what happens every
# time the fleet materialises an artifact under a fresh digest-named path - is
# a new installation as far as the broker is concerned, however faithful the
# copy.
#
# So this provisions rather than refuses. The bundle carries everything the
# generator needs, the account that runs the service is the account the
# registry must name, and doing it here means no operator has to notice that a
# directory moved. Refusing is kept for the case where the bundle cannot
# provision itself.
if [ -x "$script_dir/provision-skarbiec-trust" ]; then
  provision_hint="$script_dir/provision-skarbiec-trust"
else
  provision_hint="$bundle_root/bin/provision-skarbiec-trust"
fi

registry_describes_this_installation() {
  [ -f "$config_dir/registry.json" ] || return
  BRAMA_BIN="$BRAMA_BIN" "$PYTHON_BIN" - "$config_dir/registry.json" <<'PY'
import hashlib
import json
import os
import sys

arguments = iter(sys.argv)
next(arguments)
registry_path = next(arguments)
binary = os.environ["BRAMA_BIN"]
try:
    document = json.load(open(registry_path, encoding="utf-8"))
except (OSError, ValueError) as error:
    raise SystemExit(f"workload registry is unreadable: {error}")
workload = next(iter(document.get("workloads", {}).values()), {})
with open(binary, "rb") as handle:
    digest = hashlib.sha256(handle.read()).hexdigest()
expected = {
    "uid": os.getuid(),
    "gid": os.getgid(),
    "executable_path": os.path.realpath(binary),
    "executable_sha256": digest,
}
for name, value in expected.items():
    if str(workload.get(name)) != str(value):
        raise SystemExit(
            f"workload registry disagrees on {name}: "
            f"pinned={workload.get(name)} actual={value}"
        )
PY
}

# The workload identity belongs to this host's gateway, not to a bundle
# version. The vault holds its public half, and the broker verifies every
# redemption against that, so a key minted fresh in each new digest directory
# is a new stranger every update: capabilities keep being issued and every
# redemption is refused. Keep the private half in one stable place and let the
# generator reuse it; only the very first provision mints one.
stable_proof_key=${BRAMA_PROOF_KEY_FILE:-"${HOME:-/nonexistent}/.stado/brama-proof.key"}
# The self-healing below is conditional on inputs that ship inside the release
# while `config_dir` is deliberately outside it, so on a host whose durable
# directory was never seeded the condition is false, provisioning is skipped
# without a word, and every redemption is refused for as long as nobody looks.
# Seed it from the bundle instead: this file is release content, not identity, and
# copying it is what makes the next update self-heal.
#
# Only the recipient keys are seeded now. A subscriptions manifest used to ship
# beside them and gate the provision below, so a host with no manifest silently
# skipped provisioning and refused every redemption -- while the vault it would
# have read held the subscriptions all along. What exists is a fact about the
# vault, so nothing is copied to declare it.
if [ "$bundled_installation" -eq 1 ]; then
  seed_source="$bundle_root/etc/brama-skarbiec/recipient-public-keys.asc"
else
  seed_source="$bundle_root/etc/brama-skarbiec/recipient-public-keys.asc"
fi
if [ -f "$seed_source" ] && [ ! -f "$config_dir/recipient-public-keys.asc" ]; then
  mkdir -p "$config_dir"
  chmod u=rwx,go= "$config_dir"
  cp "$seed_source" "$config_dir/recipient-public-keys.asc"
  printf '%s\n' "seeded recipient-public-keys.asc into $config_dir" >/dev/stderr
fi
# Authoritative for the provision below, whether or not it exists yet: the
# generator's own default lives elsewhere, so leaving this unset is how a first
# provision mints the identity in a second location and the vault ends up
# holding the public half of a key nothing signs with.
export BRAMA_PROOF_KEY_FILE="$stable_proof_key"
if ! registry_describes_this_installation; then
  if [ -x "$provision_hint" ]; then
    printf '%s\n' "provisioning this installation's Skarbiec identity in $config_dir" >/dev/stderr
    BRAMA_SKARBIEC_CONFIG_DIR="$config_dir" \
    BRAMA_BIN="$BRAMA_BIN" \
    BRAMA_WORKLOAD_UID="${BRAMA_WORKLOAD_UID:-$(id -u)}" \
    BRAMA_WORKLOAD_GID="${BRAMA_WORKLOAD_GID:-$(id -g)}" \
    "$provision_hint" --force >/dev/stderr
    # A first provision mints the identity. Put it where the next version will
    # find it, or the very next update becomes a stranger again.
    if [ ! -f "$stable_proof_key" ] && [ -f "$config_dir/brama-proof.key" ]; then
      mkdir -p "$(dirname -- "$stable_proof_key")"
      cp "$config_dir/brama-proof.key" "$stable_proof_key"
      chmod u=rw,go= "$stable_proof_key"
      printf '%s\n' "recorded this gateway's workload identity at $stable_proof_key" >/dev/stderr
    fi
  fi
fi

# Every start, not only the starts that provisioned. The vault is where the
# broker looks for the public half of the key this installation proves with, and
# the entry it finds may belong to another generation entirely - a copy that was
# provisioned after this one, on the same host, leaves exactly that. The
# registration is idempotent and re-dates the grant, so doing it unconditionally
# costs one vault write and removes a whole class of `capability redemption
# denied` that no message anywhere explains.
#
# Registration is a startup requirement. Continuing used to publish `/health`
# from a gateway that could not redeem any provider credential, so release
# control committed it and every model request failed later.
register="$bundle_root/libexec/brama-register-workload.py"
[ -f "$register" ] || {
  printf '%s\n' "workload registrar is absent: $register" >/dev/stderr
  false
}
BRAMA_SKARBIEC_CONFIG_DIR="$config_dir" \
ENTITLEMENTS_ROUTER_BIN="$ENTITLEMENTS_ROUTER_BIN" \
SKARBIEC_VAULT_FILE="$SKARBIEC_VAULT_FILE" \
"$PYTHON_BIN" "$register" >/dev/stderr
}

: "${SKARBIEC_VAULT_FILE:?SKARBIEC_VAULT_FILE is required}"
source_vault_file=$SKARBIEC_VAULT_FILE
[ -f "$source_vault_file" ] || {
  printf '%s\n' "SKARBIEC_VAULT_FILE is not a regular file" >/dev/stderr
  false
}
# The vault is the fleet's, and Brama reads it where it lives.
#
# This launcher used to copy it into the per-installation runtime directory and
# serve the copy. That made Brama a second vault: a capability issued against
# the durable file was redeemed against a copy that had never heard of it, which
# is what "redemption denied: no such capability" says, and any write a refresh
# made landed in a file discarded at the next boot. One Skarbiec holds every
# level of credential -- separating them is what recipients and grants are for,
# not a second instance.
provision_and_register_workload
# Public recipient keys keep donations recoverable without exposing provider
# credentials to service configuration.
gpg --batch --quiet --import "$config_dir/recipient-public-keys.asc"

[ -r "$source_vault_file" ] || {
  printf '%s\n' "SKARBIEC_VAULT_FILE is not readable: $source_vault_file" >/dev/stderr
  false
}
export SKARBIEC_VAULT_FILE="$source_vault_file"
# The routes table says which vault coordinate a purpose stands for, and the
# authority looks for it beside the vault it was given. Both name the same
# directory as the fleet's own, so the table the operator maintains is the table
# in force.
#
# Named unconditionally, not only when the file already exists. The gateway's own
# read-grant path resolves a coordinate through this variable, so leaving it
# unset on a host whose table has yet to be written leaves that grant resolving to
# nothing while the authority still resolves against its default -- two readers
# disagreeing about which table is in force, which reads from the outside as a
# credential that is simply "unavailable".
SKARBIEC_CAPABILITY_ROUTES_FILE=${SKARBIEC_CAPABILITY_ROUTES_FILE:-"${source_vault_file%/*}/capability-routes.json"}
export SKARBIEC_CAPABILITY_ROUTES_FILE

# Skarbiec owns the mapping from capability resources to vault coordinates.
# Provider and agent resources are item ids, so its reconcile command can add
# identity mappings without Brama reading or writing the routes table. Existing
# mappings are never repointed; ambiguous items are reported and skipped.
SKARBIEC_VAULT_FILE="$SKARBIEC_VAULT_FILE" \
SKARBIEC_CAPABILITY_ROUTES_FILE="$SKARBIEC_CAPABILITY_ROUTES_FILE" \
"$ENTITLEMENTS_ROUTER_BIN" routes reconcile >/dev/stderr || \
  printf '%s\n' "Skarbiec could not reconcile capability routes; newly banked credentials may remain unavailable" >/dev/stderr
unset source_vault_file

missing=
for required in trust.json policy.json policy.sig registry.json registry.sig \
  brama-proof.key worm-receipt; do
  [ -f "$config_dir/$required" ] || missing="$missing $required"
done
if [ -n "$missing" ]; then
  printf '%s\n' "missing Skarbiec trust material in $config_dir:$missing" >/dev/stderr
  printf '%s\n' "Provision this installation once before starting it:" >/dev/stderr
  printf '%s\n' "  $provision_hint" >/dev/stderr
  printf '%s\n' "Point BRAMA_SKARBIEC_CONFIG_DIR at that directory first if the" >/dev/stderr
  printf '%s\n' "material belongs somewhere other than $config_dir." >/dev/stderr
  false
fi

export SKARBIEC_CAP_TRUST_ROOT="$config_dir/trust.json"
export SKARBIEC_CAP_POLICY="$config_dir/policy.json"
export SKARBIEC_CAP_POLICY_SIG="$config_dir/policy.sig"
export SKARBIEC_WORKLOAD_REGISTRY="$config_dir/registry.json"
export SKARBIEC_WORKLOAD_REGISTRY_SIG="$config_dir/registry.sig"
export SKARBIEC_CAP_STATE="$runtime_dir/capability.sqlite"
# A blue-green generation owns its broker socket with its capability state and
# workload registry. Capabilities are issued immediately before redemption, so
# no durable id needs a machine-wide socket; sharing one instead lets a candidate
# replace the active release's broker before traffic has cut over.
SKARBIEC_CAP_SOCKET=${BRAMA_CAP_SOCKET:-"$socket_dir/capability.sock"}
export SKARBIEC_CAP_SOCKET
mkdir -p "$(dirname -- "$SKARBIEC_CAP_SOCKET")"
chmod u=rwx,g=rx,o= "$(dirname -- "$SKARBIEC_CAP_SOCKET")"
SKARBIEC_CAP_SOCKET_GID=$(id -g)
export SKARBIEC_CAP_SOCKET_GID
export SKARBIEC_WORM_RECEIPT_DIR="$worm_dir"
export SKARBIEC_WORM_RECEIPT_COMMAND="$config_dir/worm-receipt"
export SKARBIEC_WORM_CHECKPOINT="$runtime_dir/checkpoint.json"
export SKARBIEC_WORKLOAD_ID=brama-service
export SKARBIEC_WORKLOAD_SIGNING_KEY_FILE="$config_dir/brama-proof.key"
# `read_owner_key` opens this file itself and refuses any mode carrying group or
# other bits, so a key left world-readable by whatever wrote it disables the
# capability client — and a disabled client reports as "no configured matching
# provider capability" against an arbitrary alias, never as a permission fault.
# Narrow it here, where the path is already known, rather than trusting every
# provisioning route to have done it.
chmod go-rwx "$config_dir/brama-proof.key"
export SKARBIEC_DONATION_RECIPIENT=brama-service
export ENTITLEMENTS_ROUTER_BIN

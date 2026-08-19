#!/bin/sh
set -eu
umask 077

# `pwd -P`, not `pwd`. The unit runs this through `.../brama/current/...`, and
# a logical `pwd` keeps that link in the path. The trust material then pins the
# gateway as the alias while the kernel reports the physical digest directory
# for the running process, so the broker sees a workload that is not the one
# redeeming and refuses -- and the only symptom is a credential that is
# "unavailable" long after start, with /health and /v1/models still answering.
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)
bundle_root=$(CDPATH= cd -- "$script_dir/.." && pwd -P)
if [ -x "$bundle_root/bin/brama" ] && [ -x "$bundle_root/bin/skarbiec-entitlements-router" ]; then
  default_brama_bin="$bundle_root/bin/brama"
  default_router_bin="$bundle_root/bin/skarbiec-entitlements-router"
  default_config_dir="${HOME:-/nonexistent}/.config/brama/trust"
  bundled_installation=1
else
  default_brama_bin=/usr/local/bin/brama
  default_router_bin=/usr/local/bin/skarbiec-entitlements-router
  default_config_dir=/etc/brama-skarbiec
  bundled_installation=0
fi

service_env_file=${BRAMA_SERVICE_ENV_FILE:-${HOME:-/nonexistent}/.config/brama/service.env}
if [ -f "$service_env_file" ]; then
  set -a
  . "$service_env_file"
  set +a
elif [ -n "${BRAMA_SERVICE_ENV_FILE:-}" ]; then
  printf '%s\n' "BRAMA_SERVICE_ENV_FILE is not a regular file: $service_env_file" >/dev/stderr
  false
fi

# A versioned bundle carries its own executables and trust material; these must
# move together because registry.json binds capabilities to that exact binary.
# service.env may retain paths from an older digest after a Stado update.
if [ "$bundled_installation" -eq 1 ]; then
  BRAMA_BIN="$default_brama_bin"
  ENTITLEMENTS_ROUTER_BIN="$default_router_bin"
  BRAMA_SKARBIEC_CONFIG_DIR="$default_config_dir"
else
  if [ -x "$default_brama_bin" ]; then BRAMA_BIN="$default_brama_bin"; fi
  if [ -x "$default_router_bin" ]; then ENTITLEMENTS_ROUTER_BIN="$default_router_bin"; fi
  if [ -d "$default_config_dir" ]; then BRAMA_SKARBIEC_CONFIG_DIR="$default_config_dir"; fi
fi
BRAMA_BIN=${BRAMA_BIN:-"$default_brama_bin"}
ENTITLEMENTS_ROUTER_BIN=${ENTITLEMENTS_ROUTER_BIN:-"$default_router_bin"}
config_dir=${BRAMA_SKARBIEC_CONFIG_DIR:-"$default_config_dir"}
PYTHON_BIN=${PYTHON_BIN:-python3}
command -v "$PYTHON_BIN" >/dev/null 2>&1 || {
  printf '%s\n' "PYTHON_BIN is not executable: $PYTHON_BIN" >/dev/stderr
  false
}
# One runtime directory per installation, not one for the machine. The broker
# socket lives here, and the broker answers with the trust material of the
# bundle that started it. A single shared path meant a gateway from a new
# bundle redeemed against a broker left running by an older one, whose registry
# describes a different workload -- so the authority issued the capability and
# the broker then denied redeeming it, which is a hard failure to read because
# both halves are working exactly as told.
installation=$(basename "$(dirname -- "$bundle_root")")
runtime_dir=${BRAMA_RUNTIME_DIR:-/tmp/brama-skarbiec-$installation}
socket_dir="$runtime_dir/socket"
gnupg_dir="$runtime_dir/gnupg"
worm_dir="$runtime_dir/worm"
mkdir -p "$runtime_dir" "$socket_dir" "$gnupg_dir" "$worm_dir"
chmod u=rwx,go= "$runtime_dir" "$gnupg_dir" "$worm_dir"
chmod u=rwx,g=rx,o= "$socket_dir"

: "${BRAMA_GNUPG_HOME:?BRAMA_GNUPG_HOME is required}"
[ -d "$BRAMA_GNUPG_HOME" ] || {
  printf '%s\n' "BRAMA_GNUPG_HOME is not a directory" >/dev/stderr
  false
}
export GNUPGHOME="$BRAMA_GNUPG_HOME"

# This service has its own identity, and the fleet already keeps it: the vault
# item `brama-service` carries the private half for exactly this purpose. Import
# it here, because everything below decrypts with it, and without it the
# entitlements router fails with "No secret key" and no indication that the key
# was ever meant to be somewhere.
if [ -x "$HOME/.stado/bin/stado" ]; then
  stado_bin="$HOME/.stado/bin/stado"
else
  stado_bin="$(command -v stado || true)"
fi
# Imported unconditionally: a home that already holds some secret key is not a
# home that holds this one, and skipping on that basis is why the router still
# reported "No secret key" after the import was added. Importing a key that is
# already present costs nothing.
if [ -n "$stado_bin" ]; then
  service_key="$gnupg_dir/brama-service.key"
  rm -f "$service_key"
  ( umask 077
    STADO_CONFIG=${BRAMA_SKARBIEC_STADO_CONFIG:-"${HOME:-/nonexistent}/.config/stado/brama-service.json"} \
      "$stado_bin" secrets get brama-service --field gpg_private_key > "$service_key" ) || {
    printf '%s\n' 'cannot read this service identity from Skarbiec (brama-service.gpg_private_key)' >/dev/stderr
    false
  }
  gpg --batch --quiet --import "$service_key" || {
    rm -f "$service_key"
    printf '%s\n' 'the service identity from Skarbiec did not import' >/dev/stderr
    false
  }
  rm -f "$service_key"
fi

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
  provision_hint="$bundle_root/scripts/provision-skarbiec-trust.sh"
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
  for seed in recipient-public-keys.asc; do
    if [ -f "$bundle_root/etc/brama-skarbiec/$seed" ] && [ ! -f "$config_dir/$seed" ]; then
      mkdir -p "$config_dir"
      chmod u=rwx,go= "$config_dir"
      cp "$bundle_root/etc/brama-skarbiec/$seed" "$config_dir/$seed"
      printf '%s\n' "seeded $seed into $config_dir from this release" >/dev/stderr
    fi
  done
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
register="$bundle_root/libexec/brama-register-workload.py"
if [ -f "$register" ]; then
  BRAMA_SKARBIEC_CONFIG_DIR="$config_dir" \
  ENTITLEMENTS_ROUTER_BIN="$ENTITLEMENTS_ROUTER_BIN" \
  SKARBIEC_VAULT_FILE="$SKARBIEC_VAULT_FILE" \
  "$PYTHON_BIN" "$register" >/dev/stderr || \
    printf '%s\n' "workload registration failed; redemption will be denied" >/dev/stderr
fi
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
# unset on a host whose table has yet to be written makes the grant fall back to
# nothing while the authority still resolves against its default -- two readers
# disagreeing about which table is in force, which reads from the outside as a
# credential that is simply "unavailable".
SKARBIEC_CAPABILITY_ROUTES_FILE=${SKARBIEC_CAPABILITY_ROUTES_FILE:-"${source_vault_file%/*}/capability-routes.json"}
export SKARBIEC_CAPABILITY_ROUTES_FILE

# A banked credential is spendable only when four things agree: the vault holds
# the item, the signed policy allows its resource, the routes table maps that
# resource to an item and field, and issuance succeeds. Three of those are
# derived from the vault every time this script runs. The third was not: the
# table was written by a helper somebody had to remember to run, so a
# subscription banked after the last run had no coordinate, `capability-issue`
# refused it with "no capability route maps <resource> to a vault field", the
# grant read had nothing to resolve either, and the request path answered "no
# '<provider>' credential could be redeemed for agent" while the credential
# itself was perfectly good. One ChatGPT seat sat in exactly that state for a
# day.
#
# So the table is asserted here, at every start, from the host's own contents.
# Additive only: an existing entry is never repointed and never removed, the
# coordinate is the item's own id -- the launcher builds resources from item ids
# below, so the two are the same string -- and the field is taken only when the
# item carries exactly one, which is a fact rather than a choice. Nothing about
# this widens what may be redeemed: the table maps names to coordinates, and
# redemption is still authorised by the workload key the vault registers and the
# recipients the item itself carries.
#
# Non-fatal on purpose. A host that can serve nine subscriptions must not refuse
# to start because the tenth item leaves a field ambiguous.
routes_provisioner="$bundle_root/libexec/provision-capability-routes.py"
if [ -f "$routes_provisioner" ]; then
  ENTITLEMENTS_ROUTER_BIN="$ENTITLEMENTS_ROUTER_BIN" \
  SKARBIEC_VAULT_FILE="$SKARBIEC_VAULT_FILE" \
  SKARBIEC_CAPABILITY_ROUTES_FILE="$SKARBIEC_CAPABILITY_ROUTES_FILE" \
  "$PYTHON_BIN" "$routes_provisioner" >/dev/stderr || \
    printf '%s\n' "capability routes were not provisioned: a subscription banked since the last start cannot be redeemed until $SKARBIEC_CAPABILITY_ROUTES_FILE names it" >/dev/stderr
fi
unset source_vault_file routes_provisioner

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
# One socket for this host, not one per installed release. The path used to
# carry the installation's own hash, so every upgrade introduced a broker that
# no previously issued capability could be redeemed against, and the old ones
# stayed behind holding their sockets. The guard below ends a leftover broker of
# this kind before binding, which is what makes a single stable path safe.
SKARBIEC_CAP_SOCKET=${BRAMA_CAP_SOCKET:-$HOME/.stado/run/brama-capability.sock}
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

# Brama resolves a bearer it does not recognise by asking Skarbiec what it is,
# instead of refusing everything absent from the table this launcher builds at
# start. That call needs an identity of its own: one grant carrying `introspect`
# on `tokens`, provisioned like every other consumer here.
#
# Missing grant is not fatal. Without it the gateway behaves exactly as it did
# before -- a bearer outside the table is refused -- and says so once at start
# rather than leaving every such refusal to look like a bad key.
BRAMA_SKARBIEC_CONSUMER=${BRAMA_SKARBIEC_CONSUMER:-brama-token-introspector}
BRAMA_SKARBIEC_TOKEN_FILE=${BRAMA_SKARBIEC_TOKEN_FILE:-"${HOME:-/nonexistent}/.stado/brama-token-introspector-skarbiec-token"}
if [ -r "$BRAMA_SKARBIEC_TOKEN_FILE" ]; then
  export BRAMA_SKARBIEC_CONSUMER
  export BRAMA_SKARBIEC_TOKEN_FILE
else
  printf '%s\n' "no introspection grant at $BRAMA_SKARBIEC_TOKEN_FILE; a bearer this start did not read will be refused" >/dev/stderr
  unset BRAMA_SKARBIEC_CONSUMER
  unset BRAMA_SKARBIEC_TOKEN_FILE
fi

# The service directory is host-relative: Stado adapters on this host route to
# Brama's loopback listener, while public ingress terminates before forwarding
# locally. Ignore stale deployment variables from the superseded fleet-wide
# plaintext listener.
unset BRAMA_BIND_ADDRESS
unset BRAMA_ENCRYPTED_PEER_IPS
if [ -e "$SKARBIEC_CAP_SOCKET" ] || [ -L "$SKARBIEC_CAP_SOCKET" ]; then
  [ -S "$SKARBIEC_CAP_SOCKET" ] && [ ! -L "$SKARBIEC_CAP_SOCKET" ] || {
    printf '%s\n' "unsafe stale capability socket: $SKARBIEC_CAP_SOCKET" >/dev/stderr
    false
  }
  # An owner here used to end the start, and nothing ever cleared it: the
  # launcher is the only thing that creates this broker, so a live owner is a
  # leftover from an earlier start -- and until the trap above was fixed, every
  # failed start produced one. Refusing made the first failure permanent.
  #
  # End it, but only when it really is this installation's broker. Anything
  # else holding the path is a situation this script must not resolve by
  # killing a process it cannot identify, so that still refuses.
  lsof_bin=$(command -v lsof || true)
  if [ -n "$lsof_bin" ]; then
    for owner in $("$lsof_bin" -t -- "$SKARBIEC_CAP_SOCKET" || true); do
      owner_command=$(ps -p "$owner" -o comm= || true)
      case "$owner_command" in
        *skarbiec-entitlements-router|*/skarbiec)
          printf '%s\n' "ending a leftover capability broker: $owner" >/dev/stderr
          kill "$owner" || true
          ;;
        "")
          ;;
        *)
          printf '%s\n' "capability socket is held by $owner_command ($owner), which this launcher did not start" >/dev/stderr
          false
          ;;
      esac
    done
  fi
  rm -f -- "$SKARBIEC_CAP_SOCKET"
fi

# The product-owned control document is the sole source of Brama's nonsecret
# ingress and provider policy. Service env files may select the document but may
# not override individual policy values.
# A release candidate is launched by the Stado release agent, which passes only
# `runtime.environment` from this product's manifest -- and the manifest declares
# none, so `BRAMA_CONTROL_CONFIG` arrives unset. The old fallback was
# `~/.config/brama/control.json`, a path that does not exist on the host that runs
# this service: the running process is configured through `service.env`, which
# names `~/.stado/brama-28b-control.json`. So every candidate started with no
# policy at all, failed the alias-set check the stable process passes, never
# became ready, and had its digest quarantined -- twice, twelve days apart, with
# the agent reporting only "candidate did not become ready before deadline".
#
# Reading the same `service.env` the service is configured from makes candidate
# and stable share one declaration instead of two that silently disagree.
control_config=${BRAMA_CONTROL_CONFIG:-}
# `BRAMA_SERVICE_ENV_FILE` is the fleet's name for this path: four other scripts
# here read it, and the registry's product policy passes it to every candidate.
# My first version of this fallback invented `BRAMA_SERVICE_ENV`, which would have
# been a second name for one thing in the middle of a change whose whole subject is
# that such pairs stop agreeing.
brama_service_env=${BRAMA_SERVICE_ENV_FILE:-${HOME:-/nonexistent}/.config/brama/service.env}
if [ -z "$control_config" ] && [ -f "$brama_service_env" ]; then
  control_config=$(
    sed -n 's/^[[:space:]]*BRAMA_CONTROL_CONFIG[[:space:]]*=[[:space:]]*//p' "$brama_service_env" \
      | tail -1 | tr -d "\"'"
  )
fi
control_config=${control_config:-${HOME:-/nonexistent}/.config/brama/control.json}
[ -f "$control_config" ] || {
  printf '%s\n' "BRAMA_CONTROL_CONFIG is not a regular file: $control_config" >/dev/stderr
  false
}
printf '%s\n' "control:  $control_config" >/dev/stderr
policy_dir=$(mktemp -d "$runtime_dir/policy.XXXXXX")
trap 'rm -rf "$policy_dir"' EXIT HUP INT TERM
"$PYTHON_BIN" - "$control_config" "$policy_dir/allowed-models" "$policy_dir/model-aliases" <<'PY'
import json
import os
import stat
import sys

arguments = iter(sys.argv)
next(arguments)
config_path, allowed_path, aliases_path = arguments
with open(config_path, "r", encoding="utf-8") as source:
    document = json.load(source)
try:
    policy = document["services"]["brama"]
    allowed_models = policy["allowed_models"]
    aliases = policy["model_aliases"]
    required_providers = policy["required_provider_capabilities"]
except (KeyError, TypeError) as error:
    raise SystemExit(f"services.brama policy is incomplete: {error}") from error

expected_alias_routes = {
    "best": "codex/gpt-5.3-codex-spark",
    "wisent-backend/chat/primary": "featherless/TheDrummer/Cydonia-24B-v4.3",
    "wisent-backend/chat/fallback": "featherless/TheDrummer/Cydonia-24B-v4.3",
    "wisent-backend/evaluation": "openai/default",
    "wisent-backend/embeddings": "openai/embeddings",
    "wisent-backend/moderation": "openai/moderation",
    "weles/agent/primary": "local-openai/chat-primary",
}
expected_aliases = set(expected_alias_routes)
if (
    not isinstance(allowed_models, list)
    or any(not isinstance(value, str) or not value or value.strip() != value for value in allowed_models)
    or len(allowed_models) != len(set(allowed_models))
    or set(allowed_models) != expected_aliases
):
    # Four conditions share one sentence, and the sentence named none of them. A
    # candidate failed this check on a host whose configured file held exactly the
    # expected set, and the only way to tell which condition fired was to add this.
    # Aliases are not secrets.
    if not isinstance(allowed_models, list):
        raise SystemExit(
            f"services.brama.allowed_models must be a list, found {type(allowed_models).__name__}"
            f" in {config_path}"
        )
    malformed = [
        value
        for value in allowed_models
        if not isinstance(value, str) or not value or value.strip() != value
    ]
    duplicates = sorted({value for value in allowed_models if allowed_models.count(value) > 1})
    raise SystemExit(
        "services.brama.allowed_models must contain the exact closed Brama alias set"
        f"; file={config_path}"
        f"; missing={sorted(expected_aliases - set(allowed_models))}"
        f"; unexpected={sorted(set(allowed_models) - expected_aliases)}"
        f"; malformed={malformed}"
        f"; duplicated={duplicates}"
    )


if (
    not isinstance(aliases, dict)
    or aliases != expected_alias_routes
):
    raise SystemExit("services.brama.model_aliases must map every exact alias to one provider/model route")
expected_providers = {"featherless", "openai"}
if (
    not isinstance(required_providers, list)
    or set(required_providers) != expected_providers
    or len(required_providers) != len(expected_providers)
):
    raise SystemExit(
        "services.brama.required_provider_capabilities must contain the exact direct provider set"
    )

def write_policy(path, value):
    mode = stat.S_IRUSR | stat.S_IWUSR
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, mode)
    with os.fdopen(descriptor, "w", encoding="utf-8") as target:
        target.write(value)

write_policy(allowed_path, ",".join(allowed_models))
write_policy(aliases_path, json.dumps(aliases, separators=(",", ":"), sort_keys=True))
PY
BRAMA_ALLOWED_MODELS=$(cat "$policy_dir/allowed-models")
BRAMA_MODEL_ALIASES=$(cat "$policy_dir/model-aliases")
export BRAMA_ALLOWED_MODELS BRAMA_MODEL_ALIASES
rm -rf "$policy_dir"
trap - EXIT HUP INT TERM
unset policy_dir control_config BRAMA_CONTROL_CONFIG

# Persist operator route changes outside immutable releases. The initial
# registry mirrors the exact validated launch aliases; later writes are atomic
# and owner-only in Brama's runtime.
BRAMA_INFERENCE_ROUTES_FILE=${BRAMA_INFERENCE_ROUTES_FILE:-"$HOME/.config/brama/inference-routes.json"}
routes_dir=$(dirname "$BRAMA_INFERENCE_ROUTES_FILE")
mkdir -p "$routes_dir"
chmod 0700 "$routes_dir"
if [ ! -e "$BRAMA_INFERENCE_ROUTES_FILE" ]; then
  BRAMA_MODEL_ALIASES="$BRAMA_MODEL_ALIASES" "$PYTHON_BIN" - "$BRAMA_INFERENCE_ROUTES_FILE" <<'PY'
import json
import os
import stat
import sys

path = sys.argv[1]
document = {
    "schema_version": 1,
    "routes": json.loads(os.environ["BRAMA_MODEL_ALIASES"]),
    "fallbacks": {},
    "deployments": [],
}
descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, stat.S_IRUSR | stat.S_IWUSR)
with os.fdopen(descriptor, "w", encoding="utf-8") as target:
    json.dump(document, target, indent=2, sort_keys=True)
    target.write("\n")
PY
fi
export BRAMA_INFERENCE_ROUTES_FILE
unset routes_dir

# Read every accepted bearer from its dedicated Skarbiec item through the local
# entitlement router and its exact recipient grant.
: "${BRAMA_ALLOWED_MODELS:?set exact closed Brama model allowlist}"
identities_file="$runtime_dir/model-router-client-identities.json"
"$PYTHON_BIN" - "$ENTITLEMENTS_ROUTER_BIN" >"$identities_file" <<'PY'
import json
import os
import subprocess
import sys

arguments = iter(sys.argv)
next(arguments)
router = next(arguments)
all_models = os.environ["BRAMA_ALLOWED_MODELS"].split(",")
backend_models = [model for model in all_models if model.startswith("wisent-backend/")]
# `requires_exact_aliases("weles", &[BEST_ALIAS])` in src/core/server.rs: the
# worker drafts browser trajectories, so its identity is granted the
# subscription alias and nothing else. A second alias was tried and withdrawn:
# browser work belongs on the subscription model, not on whichever local
# deployment happens to answer. Granting `weles/agent/primary` here refuses
# startup with "must give `weles` its exact required alias set".
weles_models = ["best"]
tama_models = ["best"]
# Lem's figure pipeline asks for a capability, not a vendor: `any-vision-capable`
# for judging a rendered figure and `any` for drafting one. Pinning the client to
# a model name dated the allowlist to whatever was current the day it was written
# and made every model change a Brama edit. Literature reads remain capped to the
# two chat aliases.
lem_models = [
    "wisent-backend/chat/primary",
    "wisent-backend/chat/fallback",
    "any",
    "any-vision-capable",
]
sources = [
    ("content-platform-production", "content-platform-production-model-router", "content-platform", None),
    ("echo", "echo-model-router", "echo", None),
    ("oko", "oko-model-router", "oko", None),
    ("weles", "weles-model-router", "weles", weles_models),
    ("weles-keyword-planner", "weles-keyword-planner-model-router", "wisent-app", None),
    ("jeden", "jeden-model-router", None, None),
    ("probierz", "probierz-model-router", "probierz", None),
    ("wisent-backend-api", "wisent-backend-api-model-router", None, None),
    ("wisent-app", "wisent-app-model-router", "wisent-app", None),
    ("growth-tactics", "growth-tactics-model-router", None, None),
    ("singularity", "singularity-model-router", None, None),
    ("trading-tools", "trading-tools-model-router", None, None),
    ("openenv", "openenv-model-router", None, None),
    ("trading-autonomy", "trading-autonomy-model-router", None, None),
    ("wisent-trade-agent", "wisent-trade-agent-model-router", None, None),
    ("wisent-backend", "wisent-backend-model-router", None, backend_models),
    ("tama-objective-authority", "tama-objective-authority-model-router", "wisent-app", tama_models),
    ("brama-operations", "brama-operations-model-router", "wisent-app", None),
    ("brama-desktop", "brama-desktop-model-router", None, None),
    ("lem", "lem-model-router", None, lem_models),
]

def field(item, name, required=True):
    # check=True hides the reason: the traceback names the command and drops
    # everything the router said about why it refused, which turns a one-line
    # cause into an afternoon of guessing from the outside.
    result = subprocess.run(
        [router, "get", item],
        check=False,
        capture_output=True,
        text=True,
        env=os.environ,
    )
    if result.returncode != 0:
        detail = (result.stderr or result.stdout or "").strip()
        message = f"reading {item} through the entitlements router failed: {detail}"
        if required:
            raise SystemExit(message)
        # A host holds the credentials for the products it serves, not for
        # every product in the fleet. Refusing to boot because the item for
        # one unrelated client is encrypted to a key this machine was never
        # given takes down every client whose item is readable, which is the
        # whole gateway. That client is left out instead; nothing it could
        # have done becomes possible, because Brama only accepts the bearers
        # it was handed here.
        sys.stderr.write(f"{message}\nskipping client identity {item}\n")
        return None
    payload = json.loads(result.stdout)
    if payload.get("schema") != "skarbiec.item.v2":
        raise RuntimeError(f"{item} did not return a Skarbiec v2 item")
    fields = payload.get("fields")
    if not isinstance(fields, dict):
        raise RuntimeError(f"{item} did not return a fields object")
    value = fields.get(name)
    if not isinstance(value, str) or not value or value.strip() != value:
        raise RuntimeError(f"{item}/{name} is not a single non-empty value")
    return value

# The two clients the server itself verifies at startup — it exits unless
# `wisent-backend` and `weles` carry their exact alias sets — must be present
# or the failure belongs at the top, not in a warning nobody reads.
REQUIRED_CLIENTS = {"wisent-backend", "weles"}

identities = []
for client_id, item, agent_id, allowed_models in sources:
    token = field(item, "token", required=client_id in REQUIRED_CLIENTS)
    if token is None:
        continue
    identity = {"client_id": client_id, "token": token}
    if agent_id is not None:
        identity["agent_id"] = agent_id
    if allowed_models is not None:
        identity["allowed_models"] = allowed_models
    identities.append(identity)
sys.stdout.write(json.dumps(identities, separators=(",", ":")))
PY
BRAMA_MODEL_ROUTER_CLIENT_IDENTITIES=$(cat "$identities_file")
rm -f "$identities_file"
# Empty is allowed now. The gateway resolves a bearer it does not recognise
# against Skarbiec, so this list is a warm start rather than a precondition --
# and building it requires reading every client's secret here, which is what
# stopped this launcher on a host that was not provisioned to decrypt them.
[ -n "$BRAMA_MODEL_ROUTER_CLIENT_IDENTITIES" ] || {
  printf '%s\n' "no client identities read at start; bearers will be resolved through Skarbiec" >/dev/stderr
  BRAMA_MODEL_ROUTER_CLIENT_IDENTITIES=""
}
export BRAMA_MODEL_ROUTER_CLIENT_IDENTITIES
unset BRAMA_ALLOWED_MODELS

# Product request-sign identities are projected from their exact Skarbiec items.
# `wisent-app` is Jeden's public runtime identity and uses the dedicated
# `agent:wisent-app` item rather than a product-specific `agent_auth_secret`.
BRAMA_REQUEST_SIGN_IDENTITIES="$(
  "$PYTHON_BIN" - "$ENTITLEMENTS_ROUTER_BIN" <<'PY'
import json
import os
import subprocess
import sys

arguments = iter(sys.argv)
next(arguments)
router = next(arguments)
sources = {
    "echo": "echo-agent-auth",
    "content-platform": "content-platform-agent-auth",
    "oko": "oko-model-agent-auth",
    "weles": "weles-model-agent-auth",
    "lem": "lem-agent-auth",
    "probierz": "probierz-agent-auth",
    "wisent-app": "agent:wisent-app",
}

def field(item, name):
    # check=True hides the reason: the traceback names the command and drops
    # everything the router said about why it refused, which turns a one-line
    # cause into an afternoon of guessing from the outside.
    result = subprocess.run(
        [router, "get", item],
        check=False,
        capture_output=True,
        text=True,
        env=os.environ,
    )
    if result.returncode != 0:
        detail = (result.stderr or result.stdout or "").strip()
        raise SystemExit(f"reading {item} through the entitlements router failed: {detail}")
    payload = json.loads(result.stdout)
    if payload.get("schema") != "skarbiec.item.v2":
        raise RuntimeError(f"{item} did not return a Skarbiec v2 item")
    fields = payload.get("fields")
    if not isinstance(fields, dict):
        raise RuntimeError(f"{item} did not return a fields object")
    value = fields.get(name)
    if not isinstance(value, str) or not value:
        raise RuntimeError(f"{item}/{name} is empty")
    return value

identities = {}
for expected_id, item in sources.items():
    if expected_id == "wisent-app":
        identities[expected_id] = field(item, "value")
        continue
    actual_id = field(item, "id")
    if actual_id != expected_id:
        raise RuntimeError(f"{item}/id does not match its product identity")
    identities[actual_id] = field(item, "agent_auth_secret")
print(json.dumps(identities, separators=(",", ":")))
PY
)"
[ -n "$BRAMA_REQUEST_SIGN_IDENTITIES" ] || {
  printf '%s\n' "central request-sign identities are empty" >/dev/stderr
  false
}
export BRAMA_REQUEST_SIGN_IDENTITIES


# Weles reauth is not wired here any more, and this is the note that says why so
# the next person does not restore it by reflex.
#
# This block redeemed `brama-weles-reauth` at every start and exported
# WELES_URL and BRAMA_WELES_REAUTH_TOKEN. The gateway reads neither: the only
# occurrences of "weles" in the crate are the `weles/agent/primary` model alias.
# So every start spent a capability on a token nothing presented, and a refused
# subscription credential had no path back -- the documented self-healing was an
# export.
#
# The reauth surface that does exist is Weles's own worker API on the host that
# runs it: `POST /reauth {"provider":"codex"|"claude"|"kimi"}` in
# `weles/scripts/worker/weles-api-server.mjs`, guarded by WELES_API_TOKEN. That
# token is not `brama-weles-reauth` -- presenting this one returns 401 -- and no
# vault scope in `weles/scripts/worker/deploy/skarbiec-acquisition-scopes.conf`
# provides it. Wiring a client here needs that scope to exist first; until then
# a credential the provider refuses with a token it just issued is retired by
# `subscription_dispatch`, which says re-authorization is what unblocks it.

subscriptions_file="$runtime_dir/subscriptions.json"
capabilities_file="$runtime_dir/provider-capabilities.json"
request_capabilities_file="$runtime_dir/request-sign-capabilities.json"
catalog_file="$runtime_dir/subscription-catalog.json"
"$ENTITLEMENTS_ROUTER_BIN" list >"$subscriptions_file"
"$PYTHON_BIN" - \
  "$ENTITLEMENTS_ROUTER_BIN" \
  "$subscriptions_file" \
  "$config_dir/policy.json" \
  "$capabilities_file" \
  "$request_capabilities_file" \
  "$catalog_file" <<'PY'
import json
import os
import subprocess
import sys

(
    _program,
    router,
    available_path,
    policy_path,
    capabilities_path,
    request_capabilities_path,
    catalog_path,
) = sys.argv
with open(available_path, encoding="utf-8") as source:
    available_items = json.load(source)
with open(policy_path, encoding="utf-8") as source:
    policy = json.load(source)

# Which agents may use a subscription is read off the item, not out of a manifest.
#
# The item carries it: `brama:subscription` marks one and one `brama:agent:<id>`
# tag names each agent. A JSON list beside that was a second answer to the same
# question, and it disagreed -- the list on this host declared twenty-four
# subscriptions over twenty providers while the vault held six, and a paid Claude
# account was in the vault and missing from the list.
with open(os.environ["SKARBIEC_VAULT_FILE"], encoding="utf-8") as source:
    vault_items = (json.load(source).get("items") or {}).values()
manifest_agents = {}
for item in vault_items:
    tags = item.get("tags") or []
    if "brama:subscription" not in tags:
        continue
    subscription_id = next(
        (tag[len("brama:id:"):] for tag in tags if tag.startswith("brama:id:")),
        None,
    )
    if subscription_id is None:
        continue
    agents = [tag[len("brama:agent:"):] for tag in tags if tag.startswith("brama:agent:")]
    if agents:
        manifest_agents[subscription_id] = agents

rules = policy.get("roles", {}).get("brama-runtime", [])
allowed = {
    (rule.get("purpose"), rule.get("resource"))
    for rule in rules
    if isinstance(rule, dict)
}
request_sign_agents = sorted(
    resource.removeprefix("agent:")
    for purpose, resource in allowed
    if purpose == "brama.request.sign"
    and isinstance(resource, str)
    and resource.startswith("agent:")
)
subscription_agents = sorted({
    *request_sign_agents,
    "echo",
    "content-platform",
    "oko",
    "weles",
    "lem",
    *(agent for agents in manifest_agents.values() for agent in agents),
})
normalize = lambda value: value.strip().lower().replace("_", "-")

def issue(purpose, resource):
    # Lifetime and use count are the authority's to set, not this launcher's.
    # It used to name a thirty-day, million-use capability, which the authority
    # now refuses outright: capabilities there are short and countable by
    # design, and their nonce retention is derived from that ceiling. Asking for
    # the defaults keeps the two in step, and moving the ceiling to fit an old
    # ask would have widened a security bound to spare this line a change.
    issued = subprocess.run(
        [
            router,
            "capability-issue",
            # The authority verifies a redemption against the key registered
            # for this agent, and allows a workload key only on a consumer that
            # carries `acquire`. `brama-service` carries one `read` for this
            # gateway's GPG key and so can never be that consumer, whatever
            # else is fixed; the runtime needs its own acquisition consumer.
            "--agent", "brama-runtime",
            "--purpose", purpose,
            "--resource", resource,
            "--target", "brama",
        ],
        capture_output=True,
        text=True,
    )
    if issued.returncode:
        detail = issued.stderr.strip() or issued.stdout.strip() or "no detail"
        # One subscription that cannot be issued is not a reason to serve
        # nobody. The same judgement is already made when an item cannot be
        # read: the client that depends on it is left out, and every other
        # client keeps its gateway. A raise here takes the whole thing down.
        sys.stderr.write(f"capability issue failed for {resource}: {detail}\n")
        sys.stderr.write(f"skipping subscription {resource}\n")
        return None
    return json.loads(issued.stdout)["capability_id"]

capabilities = {}
catalog = []
for item in available_items:
    if not isinstance(item, dict) or item.get("deleted", False):
        continue
    resource_id = item.get("id")
    if not isinstance(resource_id, str):
        continue
    parts = resource_id.split(":")
    if len(parts) == 2 and parts[0] == "provider":
        provider = normalize(parts[1])
        resource = f"provider:{provider}"
        if ("brama.provider.authenticate", resource) not in allowed:
            continue
        granted = issue("brama.provider.authenticate", resource)
        if granted is not None:
            capabilities[provider] = granted
    elif len(parts) == 3 and parts[0] == "provider":
        provider, item_id = normalize(parts[1]), parts[2]
        agent_ids = manifest_agents.get(item_id)
        if agent_ids is None:
            inferred = next(
                (
                    agent
                    for agent in subscription_agents
                    if item_id.startswith(f"brama-sub-{agent}-")
                ),
                None,
            )
            agent_ids = [] if inferred is None else [inferred]
        if not agent_ids:
            continue
        resource = f"provider:{provider}:{item_id}"
        if ("brama.provider.authenticate", resource) not in allowed:
            continue
        granted = issue("brama.provider.authenticate", resource)
        if granted is None:
            continue
        capabilities[item_id] = granted
        for agent_id in agent_ids:
            catalog.append({
                "id": item_id,
                "provider": provider,
                "agent_id": agent_id,
                "status": "active",
            })

for purpose, resource in sorted(allowed):
    if purpose != "brama.provider.authenticate":
        continue
    parts = resource.split(":")
    if len(parts) == 2 and parts[0] == "provider":
        provider = normalize(parts[1])
        if provider not in capabilities:
            granted = issue(purpose, resource)
            if granted is not None:
                capabilities[provider] = granted

request_capabilities = {}
for agent_id in request_sign_agents:
    granted = issue("brama.request.sign", f"agent:{agent_id}")
    if granted is not None:
        request_capabilities[agent_id] = granted

# Every capability refused, and none issued, has one overwhelmingly likely
# cause worth stating once instead of leaving it to be inferred from a column
# of identical refusals followed by an alias error that mentions none of this.
# A resource names a purpose; only the issuing operator says which vault entry
# it stands for, and that mapping lives in one file beside the vault. Without
# it nothing resolves, the gateway starts with no provider it may authenticate
# to, and the first alias that needs one ends the process.
if not capabilities and not request_capabilities:
    routes_file = os.environ.get("SKARBIEC_CAPABILITY_ROUTES_FILE", "")
    where = routes_file or "capability-routes.json beside the Skarbiec vault"
    sys.stderr.write(
        "no capability was issued for any provider on this host: the routes "
        f"table is missing or maps nothing.\nExpected at: {where}\n"
        'Each entry maps one resource to one vault coordinate, for example '
        '{"provider:openai": {"item": "provider-openai", "field": "api_key"}}.\n'
        "The issuing operator writes it -- a workload that chose its own "
        "mapping would be choosing which credential its purpose stands for.\n"
    )
with open(capabilities_path, "w", encoding="utf-8") as target:
    json.dump(capabilities, target, separators=(",", ":"))
with open(request_capabilities_path, "w", encoding="utf-8") as target:
    json.dump(request_capabilities, target, separators=(",", ":"))
with open(catalog_path, "w", encoding="utf-8") as target:
    json.dump({"items": catalog}, target, separators=(",", ":"))
PY
export BRAMA_PROVIDER_CAPABILITY_IDS="$(cat "$capabilities_file")"
export BRAMA_REQUEST_SIGN_CAPABILITY_IDS="$(cat "$request_capabilities_file")"
export BRAMA_SUBSCRIPTION_CATALOG="$(cat "$catalog_file")"

# Keep the Darwin runtime aligned with the canonical control-plane aliases.
# `MODEL_ALIASES` in src/core/server.rs requires the exact seven-alias set, so
# omitting one fails startup with "must contain the exact named alias set".
#
# `weles/agent/primary` is no longer routed to a client: `weles` holds `best`
# only. The alias name stays because the validated set is exactly these seven,
# and its route points at the deployment the backend chat aliases use rather
# than at the roleplay model it named before.
if [ "$(uname -s)" = Darwin ]; then
  export BRAMA_MODEL_ALIASES='{"best":"codex/gpt-5.3-codex-spark","weles/agent/primary":"local-openai/chat-primary","wisent-backend/chat/fallback":"local-openai/chat-primary","wisent-backend/chat/primary":"local-openai/chat-primary","wisent-backend/embeddings":"openai/embeddings","wisent-backend/evaluation":"local-openai/chat-primary","wisent-backend/moderation":"openai/moderation"}'
fi


$ENTITLEMENTS_ROUTER_BIN capability-serve &
broker_pid=$!
trap 'kill "$broker_pid" 2>/dev/null || true' EXIT INT TERM
attempt=0
while [ ! -S "$SKARBIEC_CAP_SOCKET" ]; do
  attempt=$((attempt + 1))
  if [ "$attempt" -ge 100 ]; then
    echo 'Skarbiec capability broker did not create its socket' >&2
    exit 1
  fi
  sleep 0.05
done


# Renewal runs here because nothing else runs it. A subscription credential that
# the provider rejects is repaired by a real sign-in through Weles, and until now
# that repair waited for a person: the fleet carried five days of refused
# subscriptions that only surfaced when an operator looked at a screen. A
# separate launchd unit would be the tidier home, but a new unit cannot be
# bootstrapped through the fleet channel on this host, so the loop lives beside
# the capability broker and shares the gateway's lifecycle. It sweeps, sleeps and
# spends nothing when there is nothing refused; its own cooldown, not the sweep
# interval, decides how often one account is signed in again.
RENEWAL_LOOP=${BRAMA_RENEWAL_LOOP_BIN:-"$bundle_root/bin/renewal-loop-service"}
if [ "${BRAMA_RENEWAL_ENABLED:-1}" != 0 ] && [ -x "$RENEWAL_LOOP" ]; then
  BRAMA_RENEWAL_SWEEP_COMMAND=${BRAMA_RENEWAL_SWEEP_COMMAND:-"$bundle_root/bin/renew-refused-subscriptions"}
  BRAMA_RENEWAL_ROUTER_BIN=${BRAMA_RENEWAL_ROUTER_BIN:-"$ENTITLEMENTS_ROUTER_BIN"}
  export BRAMA_RENEWAL_SWEEP_COMMAND BRAMA_RENEWAL_ROUTER_BIN
  "$RENEWAL_LOOP" &
  renewal_pid=$!
  trap 'kill "$broker_pid" "$renewal_pid" 2>/dev/null || true' EXIT INT TERM
  printf '%s\n' "renewal loop started as pid $renewal_pid"
else
  printf '%s\n' "renewal loop not started (enabled=${BRAMA_RENEWAL_ENABLED:-1}, path $RENEWAL_LOOP)"
fi

# `exec` on purpose. Supervising the gateway from this shell instead looked
# tidier -- a trap could then stop the capability broker -- but it put a shell
# between the supervisor and the process that matters. The supervisor stops the
# job by signalling what it launched, that signal is not one a shell trap gets
# to answer, and the gateway it had started outlived the stop as a disowned
# process still holding port 8080. Every later start then failed on an address
# already in use, and the service showed inactive while a gateway it no longer
# controlled kept serving.
#
# The broker no longer needs the trap: a leftover one is ended at startup by
# the socket guard above, which is the same repair without the shell.
exec "$BRAMA_BIN" serve --port "${BRAMA_PORT_OVERRIDE:-${PORT:-8080}}"

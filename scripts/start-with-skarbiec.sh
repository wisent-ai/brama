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

requested_runtime_dir=${BRAMA_RUNTIME_DIR:-}
requested_config_dir=${BRAMA_SKARBIEC_CONFIG_DIR:-}
service_env_file=${BRAMA_SERVICE_ENV_FILE:-${HOME:-/nonexistent}/.config/brama/service.env}
if [ -f "$service_env_file" ]; then
  set -a
  . "$service_env_file"
  set +a
elif [ -n "${BRAMA_SERVICE_ENV_FILE:-}" ]; then
  printf '%s\n' "BRAMA_SERVICE_ENV_FILE is not a regular file: $service_env_file" >/dev/stderr
  false
fi
configured_config_dir=$requested_config_dir

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
if [ "$bundled_installation" -eq 0 ] && [ ! -x "$ENTITLEMENTS_ROUTER_BIN" ]; then
  if [ -x "${HOME:-/nonexistent}/.stado/bin/skarbiec" ]; then
    ENTITLEMENTS_ROUTER_BIN="${HOME:-/nonexistent}/.stado/bin/skarbiec"
  else
    discovered_router=$(command -v skarbiec || true)
    if [ -n "$discovered_router" ] && [ -x "$discovered_router" ]; then
      ENTITLEMENTS_ROUTER_BIN="$discovered_router"
    fi
  fi
fi
if [ -n "${BRAMA_BIN_OVERRIDE:-}" ]; then
  [ -x "$BRAMA_BIN_OVERRIDE" ] || {
    printf '%s\n' "BRAMA_BIN_OVERRIDE is not executable: $BRAMA_BIN_OVERRIDE" >/dev/stderr
    false
  }
  BRAMA_BIN="$BRAMA_BIN_OVERRIDE"
  # A source-tree binary is not a system installation. Provision its generated
  # trust beside the user's service state unless the caller named another
  # directory; /etc is both shared with production and unwritable in a normal
  # development or Probierz journey.
  if [ -z "$requested_runtime_dir" ]; then
    unset BRAMA_RUNTIME_DIR
  fi
  if [ -z "$configured_config_dir" ]; then
    BRAMA_SKARBIEC_CONFIG_DIR="${HOME:-/nonexistent}/.config/brama/trust"
    config_dir="$BRAMA_SKARBIEC_CONFIG_DIR"
  fi
fi
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
runtime_dir=${BRAMA_RUNTIME_DIR:-"${HOME:-/nonexistent}/.stado/run/brama-skarbiec-$installation"}
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
# The service key must match a recipient of the encrypted Brama items. An
# unrelated private key is not enough: operator keyrings can contain other
# identities while still being unable to decrypt the model-router records.
service_identity_present=
for recipient_key in 7E0441E08C5CEAAC 6C6746F4AB546CB4 C30E7BF28DDE114E; do
  if gpg --batch --list-secret-keys "$recipient_key" >/dev/null 2>&1; then
    service_identity_present=1
    break
  fi
done
if [ -n "$stado_bin" ] && [ -z "$service_identity_present" ]; then
  # An explicitly configured endpoint belongs to this Brama installation and
  # wins over the fleet default. The fleet can run another Skarbiec instance
  # for host management; using that endpoint here couples Brama startup to an
  # unrelated keyring and can leave a healthy listener unable to decrypt the
  # Brama service identity.
  if [ -n "${WC_AGENT_SKARBIEC_URL:-}" ]; then
    agent_skarbiec_url="$WC_AGENT_SKARBIEC_URL"
  else
    fleet_stado_config=${BRAMA_FLEET_STADO_CONFIG:-"${HOME:-/nonexistent}/.config/stado/config.json"}
    agent_skarbiec_url="$(
      STADO_CONFIG="$fleet_stado_config" "$stado_bin" config show \
        | "$PYTHON_BIN" -c '
import json
import sys
value = json.load(sys.stdin).get("resolved", {}).get("agent_skarbiec_url")
if not isinstance(value, str) or not value:
    raise SystemExit("fleet Stado config has no agent_skarbiec_url")
sys.stdout.write(value)
'
    )"
  fi
  service_key="$gnupg_dir/brama-service.key"
  rm -f "$service_key"
  read_attempt=1
  read_attempts=${BRAMA_SKARBIEC_READ_ATTEMPTS:-3}
  while ! ( umask 077
    WC_AGENT_SKARBIEC_URL="$agent_skarbiec_url" \
      STADO_CONFIG=${BRAMA_SKARBIEC_STADO_CONFIG:-"${HOME:-/nonexistent}/.config/stado/brama-service.json"} \
      "$stado_bin" secrets get brama-service --field gpg_private_key > "$service_key" )
  do
    rm -f "$service_key"
    if [ "$read_attempt" -ge "$read_attempts" ]; then
      printf '%s\n' 'cannot read this service identity from Skarbiec (brama-service.gpg_private_key)' >/dev/stderr
      false
    fi
    printf '%s\n' "Skarbiec could not return the Brama identity; retrying ($read_attempt/$read_attempts)" >/dev/stderr
    read_attempt=$((read_attempt + 1))
    sleep 2
  done
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
  seed_source="$bundle_root/etc/brama-skarbiec/recipient-public-keys.asc"
else
  seed_source="$bundle_root/scripts/skarbiec-recipient-public-keys.asc"
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
# unset on a host whose table has yet to be written makes the grant fall back to
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

required_aliases = {
    "best",
    "wisent-backend/chat/primary",
    "wisent-backend/chat/fallback",
    "wisent-backend/evaluation",
    "wisent-backend/embeddings",
    "wisent-backend/moderation",
    "weles/agent/primary",
}
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
alias_names = set(aliases) if isinstance(aliases, dict) else set()
allowed_names = {value for value in allowed_models if isinstance(value, str)}
if (
    malformed
    or duplicates
    or allowed_names != alias_names
    or not required_aliases.issubset(allowed_names)
):
    raise SystemExit(
        "services.brama.allowed_models must match model_aliases and include the required aliases"
        f"; file={config_path}"
        f"; missing_required={sorted(required_aliases - allowed_names)}"
        f"; missing_from_allowed={sorted(alias_names - allowed_names)}"
        f"; missing_from_aliases={sorted(allowed_names - alias_names)}"
        f"; malformed={malformed}"
        f"; duplicated={duplicates}"
    )

if not isinstance(aliases, dict) or not required_aliases.issubset(set(aliases)):
    raise SystemExit(
        "services.brama.model_aliases must contain every required Brama alias"
        f"; file={config_path}"
        f"; missing={sorted(required_aliases - set(aliases) if isinstance(aliases, dict) else required_aliases)}"
    )
malformed_routes = {
    alias: route
    for alias, route in aliases.items()
    if not isinstance(route, str)
    or not route
    or route.strip() != route
    or "/" not in route
}
if malformed_routes:
    raise SystemExit(
        "services.brama.model_aliases contains malformed provider/model routes"
        f"; file={config_path}; malformed={malformed_routes}"
    )
if (
    not isinstance(required_providers, list)
    or not required_providers
    or any(
        not isinstance(provider, str) or not provider or provider.strip() != provider
        for provider in required_providers
    )
    or len(required_providers) != len(set(required_providers))
):
    raise SystemExit(
        "services.brama.required_provider_capabilities must be a non-empty unique provider list"
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

# Preload only the two bearers whose exact alias sets Brama validates at
# startup. Every other bearer is resolved through Skarbiec introspection on its
# first request. Each router invocation loads the large vault, so bounded
# concurrent reads keep required startup work inside Stado's candidate deadline.
: "${BRAMA_ALLOWED_MODELS:?set exact closed Brama model allowlist}"
identities_file="$runtime_dir/model-router-client-identities.json"
printf '%s\n' "reading required model-router identities" >/dev/stderr
"$PYTHON_BIN" - "$ENTITLEMENTS_ROUTER_BIN" >"$identities_file" <<'PY'
from concurrent.futures import ThreadPoolExecutor
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
sources = [
    ("weles", "weles-model-router", "weles", weles_models),
    ("wisent-backend", "wisent-backend-model-router", "wisent-app", backend_models),
]

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
    if not isinstance(value, str) or not value or value.strip() != value:
        raise RuntimeError(f"{item}/{name} is not a single non-empty value")
    return value

def identity(source):
    client_id, item, agent_id, allowed_models = source
    result = {"client_id": client_id, "token": field(item, "token"), "agent_id": agent_id}
    if allowed_models is not None:
        result["allowed_models"] = allowed_models
    return result

with ThreadPoolExecutor(max_workers=len(sources)) as executor:
    identities = list(executor.map(identity, sources))
sys.stdout.write(json.dumps(identities, separators=(",", ":")))
PY
printf '%s\n' "read required model-router identities" >/dev/stderr
BRAMA_MODEL_ROUTER_CLIENT_IDENTITIES=$(cat "$identities_file")
rm -f "$identities_file"
# Unknown bearers are intentionally absent here and resolved by the
# introspection grant configured above.
export BRAMA_MODEL_ROUTER_CLIENT_IDENTITIES
unset BRAMA_ALLOWED_MODELS

# Product request-sign identities are projected from their exact Skarbiec items.
# `wisent-app` is Jeden's public runtime identity and uses the dedicated
# `agent:wisent-app` item rather than a product-specific `agent_auth_secret`.
BRAMA_REQUEST_SIGN_IDENTITIES="$(
printf '%s\n' "reading request-sign identities" >/dev/stderr
  "$PYTHON_BIN" - "$ENTITLEMENTS_ROUTER_BIN" <<'PY'
from concurrent.futures import ThreadPoolExecutor
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

def item_fields(item):
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
    return fields

def field(fields, item, name):
    value = fields.get(name)
    if not isinstance(value, str) or not value:
        raise RuntimeError(f"{item}/{name} is empty")
    return value

def identity(source):
    expected_id, item = source
    fields = item_fields(item)
    if expected_id == "wisent-app":
        return expected_id, field(fields, item, "value")
    actual_id = field(fields, item, "id")
    if actual_id != expected_id:
        raise RuntimeError(f"{item}/id does not match its product identity")
    return actual_id, field(fields, item, "agent_auth_secret")

with ThreadPoolExecutor(max_workers=4) as executor:
    identities = dict(executor.map(identity, sources.items()))
print(json.dumps(identities, separators=(",", ":")))
PY
)"
printf '%s\n' "read request-sign identities" >/dev/stderr
[ -n "$BRAMA_REQUEST_SIGN_IDENTITIES" ] || {
  printf '%s\n' "central request-sign identities are empty" >/dev/stderr
  false
}
export BRAMA_REQUEST_SIGN_IDENTITIES


# Brama and Weles authenticate this one route with the same Skarbiec-owned
# bearer. Brama acquires its copy through its own identity; Weles acquires its
# copy through its own identity. Neither service reads the other's files.
printf '%s\n' "reading Brama-Weles reauthentication identity" >/dev/stderr
BRAMA_WELES_REAUTH_TOKEN="$(
  "$ENTITLEMENTS_ROUTER_BIN" get brama-weles-reauth \
    | "$PYTHON_BIN" -c '
import json
import sys
payload = json.load(sys.stdin)
if payload.get("schema") != "skarbiec.item.v2":
    raise SystemExit("brama-weles-reauth did not return a Skarbiec v2 item")
value = payload.get("fields", {}).get("token")
if not isinstance(value, str) or not value:
    raise SystemExit("brama-weles-reauth/token is empty")
sys.stdout.write(value)
'
)"
printf '%s\n' "read Brama-Weles reauthentication identity" >/dev/stderr
[ -n "$BRAMA_WELES_REAUTH_TOKEN" ] || {
  printf '%s\n' "Brama-Weles reauthentication token is empty" >/dev/stderr
  false
}
export BRAMA_WELES_REAUTH_TOKEN

subscriptions_file="$runtime_dir/subscriptions.json"
catalog_file="$runtime_dir/subscription-catalog.json"
# Releases before 0.2.52 persisted boot-time capability ids here. They are
# single-use authority records, not durable service state.
rm -f "$runtime_dir/provider-capabilities.json" "$runtime_dir/request-sign-capabilities.json"
printf '%s\n' "reading subscription catalog" >/dev/stderr
"$ENTITLEMENTS_ROUTER_BIN" list >"$subscriptions_file"
printf '%s\n' "read subscription catalog; building runtime catalog" >/dev/stderr
"$PYTHON_BIN" - \
  "$subscriptions_file" \
  "$config_dir/policy.json" \
  "$catalog_file" <<'PY'
import json
import sys

(
    _program,
    available_path,
    policy_path,
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
#
# These four namespaces are the whole vocabulary, and the listing already read
# above carries them, so this file opens the vault once through the router and
# never reads the vault file beside it.
SUBSCRIPTION_TAG = "brama:subscription"
PROVIDER_TAG = "brama:provider:"
SUBSCRIPTION_ID_TAG = "brama:id:"
AGENT_TAG = "brama:agent:"
LOGIN_TAG = "brama:login:"


def tag_values(tags, prefix):
    return [tag[len(prefix):] for tag in tags if tag.startswith(prefix) and tag != prefix]


def tag_value(tags, prefix):
    declared = tag_values(tags, prefix)
    return declared[0] if declared else None

rules = policy.get("roles", {}).get("brama-runtime", [])
allowed = {
    (rule.get("purpose"), rule.get("resource"))
    for rule in rules
    if isinstance(rule, dict)
}
normalize = lambda value: value.strip().lower().replace("_", "-")


catalog = []
# Subscription metadata comes from the item's declared tags, never from its id.
# A capability is intentionally not issued here. Skarbiec capabilities are
# short-lived and single-use, while the gateway already obtains one immediately
# before each credential redemption. Seeding every allowed resource delayed
# startup, rewrote the capability state once per resource, and produced ids that
# model discovery spent before the first request.
#
# `brama:subscription` marks a subscription, `brama:provider:<provider>` names
# its provider, `brama:id:<subscription-id>` names the subscription,
# `brama:login:<vault-item>` names the Weles account that can renew it, and each
# `brama:agent:<agent>` names an agent allowed to spend it. The policy still has
# to allow the exact provider resource; the catalog only exposes metadata and
# every use still requires a fresh capability from the authority.
for item in available_items:
    if not isinstance(item, dict) or item.get("deleted", False):
        continue
    item_name = item.get("id")
    if not isinstance(item_name, str):
        continue
    tags = item.get("tags") or []
    if SUBSCRIPTION_TAG not in tags:
        continue
    provider = tag_value(tags, PROVIDER_TAG)
    subscription_id = tag_value(tags, SUBSCRIPTION_ID_TAG)
    agent_ids = tag_values(tags, AGENT_TAG)
    login_item = tag_value(tags, LOGIN_TAG)
    missing = [
        f"{prefix}<value>"
        for prefix, value in (
            (PROVIDER_TAG, provider),
            (SUBSCRIPTION_ID_TAG, subscription_id),
            (AGENT_TAG, agent_ids),
        )
        if not value
    ]
    if missing:
        sys.stderr.write(
            f"skipping {item_name}: carries {SUBSCRIPTION_TAG} but no "
            f"{' and no '.join(missing)} tag, so nothing declares what it serves; "
            "tag it and it is served again\n"
        )
        continue
    provider = normalize(provider)
    resource = f"provider:{provider}:{subscription_id}"
    if ("brama.provider.authenticate", resource) not in allowed:
        continue
    for agent_id in agent_ids:
        catalog.append({
            "id": subscription_id,
            "provider": provider,
            "agent_id": agent_id,
            "status": "active",
            "login_item": login_item,
        })

with open(catalog_path, "w", encoding="utf-8") as target:
    json.dump({"items": catalog}, target, separators=(",", ":"))
PY
printf '%s\n' "built runtime catalog; capabilities issue on demand" >/dev/stderr
export BRAMA_SUBSCRIPTION_CATALOG="$(cat "$catalog_file")"
# Boot-time ids are single-use and cannot be refreshed inside a running process.
# Clear inherited values so every credential use asks the authority at its final
# use boundary instead of trying a stale seed first.
unset BRAMA_PROVIDER_CAPABILITY_IDS BRAMA_REQUEST_SIGN_CAPABILITY_IDS



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

# The same trust, routes and short-lived capabilities serve administrative CLI
# journeys and real product tests. Keeping that setup here prevents `brama
# onboard` from silently falling back to an unconfigured in-process router while
# the service itself is healthy. `--exec` runs a named program inside the exact
# launcher environment while trust remains pinned to BRAMA_BIN_OVERRIDE; this is
# how Cargo's real-binary journeys run without teaching the service launcher
# about Cargo. A command exits through the trap above, so its temporary broker is
# removed; the long-running service path below still uses `exec`.
if [ "${1:-}" = "--exec" ]; then
  shift
  [ "$#" -gt 0 ] || {
    printf '%s\n' 'start-with-skarbiec.sh --exec requires a command' >/dev/stderr
    exit 2
  }
  "$@"
  exit $?
fi
if [ "$#" -gt 0 ]; then
  "$BRAMA_BIN" "$@"
  exit $?
fi


# Releases before this launcher handed the gateway to a child shell. Stopping
# the launchd job therefore left that child alive on port 8080, and every newer
# release was quarantined even though its own process model was correct. Retire
# only an exact stale managed executable: never kill an arbitrary listener.
brama_port=${BRAMA_PORT_OVERRIDE:-${PORT:-8080}}
if [ -x /usr/sbin/lsof ]; then
  for stale_pid in $(/usr/sbin/lsof -nP -tiTCP:"$brama_port" -sTCP:LISTEN 2>/dev/null || true); do
    stale_bin=$(ps -p "$stale_pid" -o comm= 2>/dev/null || true)
    case "$stale_bin" in
      "${HOME:-/nonexistent}/.stado/services/brama/sha256-"*/darwin-arm/bin/brama)
        if [ "$(realpath "$stale_bin")" != "$(realpath "$BRAMA_BIN")" ]; then
          printf '%s\n' "retiring stale managed Brama process $stale_pid from $stale_bin" >/dev/stderr
          kill "$stale_pid"
          attempt=0
          while kill -0 "$stale_pid" 2>/dev/null; do
            attempt=$((attempt + 1))
            if [ "$attempt" -ge 100 ]; then
              printf '%s\n' "stale managed Brama process $stale_pid did not stop" >/dev/stderr
              exit 1
            fi
            sleep 0.05
          done
        fi
        ;;
    esac
  done
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

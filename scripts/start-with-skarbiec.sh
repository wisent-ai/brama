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

# A versioned bundle carries its own executables, and those always win. Trust
# configuration is host-owned and stable across bundle digests, so an explicit
# BRAMA_SKARBIEC_CONFIG_DIR remains authoritative.
if [ "$bundled_installation" -eq 1 ]; then
  BRAMA_BIN="$default_brama_bin"
  ENTITLEMENTS_ROUTER_BIN="$default_router_bin"
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
if [ -f "$stable_proof_key" ]; then
  BRAMA_PROOF_KEY_FILE="$stable_proof_key"
  export BRAMA_PROOF_KEY_FILE
fi
if ! registry_describes_this_installation; then
  if [ -x "$provision_hint" ] && [ -f "$config_dir/subscriptions.json" ]; then
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
# The workload registration belongs to the durable vault, not to the copy this
# launcher is about to make: the copy exists for the length of one boot, and a
# grant written into it is gone by the next one — while `token-mint` refuses to
# write it there at all, because the copy carries none of the owner material
# that authorises a change. So provisioning and registration happen against the
# real vault, above the copy, and the copy then carries the result.
provision_and_register_workload
# Public recipient keys keep donations recoverable without exposing provider
# credentials to service configuration.
gpg --batch --quiet --import "$config_dir/recipient-public-keys.asc"

vault_file="$runtime_dir/vault.json"
if [ "$source_vault_file" != "$vault_file" ]; then
  cp "$source_vault_file" "$vault_file"
fi
chmod u=rw,go= "$vault_file"
export SKARBIEC_VAULT_FILE="$vault_file"
# The routes table says which vault coordinate a purpose stands for, and the
# authority looks for it beside the vault it was given. That is now the copy
# above, in a runtime directory this launcher just made, where the operator's
# table has never been -- so every resource resolved to nothing and the gateway
# started with no provider it could authenticate to. Name the real one instead
# of copying it: one table, beside the vault it belongs to, never a stale
# duplicate. Absent, the authority keeps its own default.
if [ -f "${source_vault_file%/*}/capability-routes.json" ]; then
  SKARBIEC_CAPABILITY_ROUTES_FILE="${source_vault_file%/*}/capability-routes.json"
  export SKARBIEC_CAPABILITY_ROUTES_FILE
fi
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
export SKARBIEC_CAP_SOCKET="$socket_dir/broker.sock"
SKARBIEC_CAP_SOCKET_GID=$(id -g)
export SKARBIEC_CAP_SOCKET_GID
chgrp "$SKARBIEC_CAP_SOCKET_GID" "$socket_dir"
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
control_config=${BRAMA_CONTROL_CONFIG:-${HOME:-/nonexistent}/.config/brama/control.json}
[ -f "$control_config" ] || {
  printf '%s\n' "BRAMA_CONTROL_CONFIG is not a regular file: $control_config" >/dev/stderr
  false
}
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
    "-best": "claude-code/claude-opus-4-6",
    "wisent-backend/chat/primary": "qwen/default",
    "wisent-backend/chat/fallback": "qwen/default",
    "wisent-backend/evaluation": "openai/default",
    "wisent-backend/embeddings": "openai/embeddings",
    "wisent-backend/moderation": "openai/moderation",
    "weles/agent/primary": "qwen/default",
}
expected_aliases = set(expected_alias_routes)
if (
    not isinstance(allowed_models, list)
    or any(not isinstance(value, str) or not value or value.strip() != value for value in allowed_models)
    or len(allowed_models) != len(set(allowed_models))
    or set(allowed_models) != expected_aliases
):
    raise SystemExit("services.brama.allowed_models must contain the exact closed Brama alias set")


if (
    not isinstance(aliases, dict)
    or aliases != expected_alias_routes
):
    raise SystemExit("services.brama.model_aliases must map every exact alias to one provider/model route")
expected_providers = {"openai", "qwen"}
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
BRAMA_MODEL_ROUTER_CLIENT_IDENTITIES="$(
  "$PYTHON_BIN" - "$ENTITLEMENTS_ROUTER_BIN" <<'PY'
import json
import os
import subprocess
import sys

arguments = iter(sys.argv)
next(arguments)
router = next(arguments)
all_models = os.environ["BRAMA_ALLOWED_MODELS"].split(",")
backend_models = [model for model in all_models if model.startswith("wisent-backend/")]
weles_models = ["weles/agent/primary"]
tama_models = ["-best"]
# Lem reads literature one paper per call, so it is capped to the chat
# aliases rather than the whole catalogue: a harvest that could reach an
# image or embedding route is a harvest that can spend on one by mistake.
lem_models = ["wisent-backend/chat/primary", "wisent-backend/chat/fallback"]
sources = [
    ("content-platform-production", "content-platform-production-model-router", "content-platform", None),
    ("echo", "echo-model-router", "echo", None),
    ("oko", "oko-model-router", "oko", None),
    ("weles", "weles-model-router", "weles", weles_models),
    ("weles-keyword-planner", "weles-keyword-planner-model-router", "wisent-app", None),
    ("jeden", "jeden-model-router", None, None),
    ("probierz", "probierz-model-router", None, None),
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
)"
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

# Echo, legacy Content Platform, Oko, and Weles identities are read from their
# exact Skarbiec items.
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


# Weles reauth uses one dedicated Skarbiec item and one accepted runtime name.
# It is not the Weles console token and has no general Weles API scope.
: "${WELES_URL:=https://weles.wisent.ai}"

BRAMA_WELES_REAUTH_TOKEN="$(
  "$PYTHON_BIN" - "$ENTITLEMENTS_ROUTER_BIN" <<'PY'
import json
import os
import subprocess
import sys

arguments = iter(sys.argv)
next(arguments)
router = next(arguments)
result = subprocess.run(
    [router, "get", "brama-weles-reauth"],
    check=True,
    capture_output=True,
    text=True,
    env=os.environ,
)
payload = json.loads(result.stdout)
if payload.get("schema") != "skarbiec.item.v2":
    raise RuntimeError("brama-weles-reauth did not return a Skarbiec v2 item")
fields = payload.get("fields")
if not isinstance(fields, dict):
    raise RuntimeError("brama-weles-reauth did not return a fields object")
value = fields.get("token")
if not isinstance(value, str) or not value or value.strip() != value:
    raise RuntimeError("brama-weles-reauth/token is empty or malformed")
sys.stdout.write(value)
PY
)"
export WELES_URL BRAMA_WELES_REAUTH_TOKEN

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
subscription_agents = sorted([*request_sign_agents, "echo", "content-platform", "oko"])
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
        agent_id = next(
            (
                agent
                for agent in subscription_agents
                if item_id.startswith(f"brama-sub-{agent}-")
            ),
            None,
        )
        if agent_id is None:
            continue
        resource = f"provider:{provider}:{item_id}"
        if ("brama.provider.authenticate", resource) not in allowed:
            continue
        granted = issue("brama.provider.authenticate", resource)
        if granted is None:
            continue
        capabilities[item_id] = granted
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

# The Mac worker exposes only its deployment-owned local chat provider, plus the
# `-best` subscription route. `-best` must be present: `MODEL_ALIASES` in
# src/core/server.rs requires the exact seven-alias set, so omitting it here
# fails startup with "must contain the exact named alias set". It carries no
# direct provider capability by design; the caller's HMAC identity selects the
# subscription that pays.
if [ "$(uname -s)" = Darwin ]; then
  export BRAMA_MODEL_ALIASES='{"-best":"claude-code/claude-opus-4-6","weles/agent/primary":"local-openai/chat-primary","wisent-backend/chat/fallback":"local-openai/chat-primary","wisent-backend/chat/primary":"local-openai/chat-primary","wisent-backend/embeddings":"openai/embeddings","wisent-backend/evaluation":"local-openai/chat-primary","wisent-backend/moderation":"openai/moderation"}'
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

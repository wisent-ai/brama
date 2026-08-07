#!/bin/sh
set -eu
umask 077

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
bundle_root=$(CDPATH= cd -- "$script_dir/.." && pwd)
if [ -x "$bundle_root/bin/brama" ] && [ -d "$bundle_root/etc/brama-skarbiec" ]; then
  default_brama_bin="$bundle_root/bin/brama"
  default_router_bin="$bundle_root/bin/skarbiec-entitlements-router"
  default_config_dir="$bundle_root/etc/brama-skarbiec"
else
  default_brama_bin=/usr/local/bin/brama
  default_router_bin=/usr/local/bin/skarbiec-entitlements-router
  default_config_dir=/etc/brama-skarbiec
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

# A versioned bundle carries its own components, and its own win. The service
# env file is sourced with `set -a`, so a path written there once pins every
# later version to the directory that happened to be current that day: the
# launcher from the new bundle starts, runs a binary from the old one, and the
# gateway never comes up. The override still works for a bundle that does not
# carry the component.
if [ -x "$default_brama_bin" ]; then BRAMA_BIN="$default_brama_bin"; fi
if [ -x "$default_router_bin" ]; then ENTITLEMENTS_ROUTER_BIN="$default_router_bin"; fi
if [ -d "$default_config_dir" ]; then BRAMA_SKARBIEC_CONFIG_DIR="$default_config_dir"; fi
BRAMA_BIN=${BRAMA_BIN:-"$default_brama_bin"}
ENTITLEMENTS_ROUTER_BIN=${ENTITLEMENTS_ROUTER_BIN:-"$default_router_bin"}
config_dir=${BRAMA_SKARBIEC_CONFIG_DIR:-"$default_config_dir"}
PYTHON_BIN=${PYTHON_BIN:-python3}
command -v "$PYTHON_BIN" >/dev/null 2>&1 || {
  printf '%s\n' "PYTHON_BIN is not executable: $PYTHON_BIN" >/dev/stderr
  false
}
runtime_dir=${BRAMA_RUNTIME_DIR:-/tmp/brama-skarbiec}
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
# Public recipient keys keep donations recoverable without exposing provider
# credentials to service configuration.
gpg --batch --quiet --import "$config_dir/recipient-public-keys.asc"

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

: "${SKARBIEC_VAULT_FILE:?SKARBIEC_VAULT_FILE is required}"
source_vault_file=$SKARBIEC_VAULT_FILE
[ -f "$source_vault_file" ] || {
  printf '%s\n' "SKARBIEC_VAULT_FILE is not a regular file" >/dev/stderr
  false
}
vault_file="$runtime_dir/vault.json"
if [ "$source_vault_file" != "$vault_file" ]; then
  cp "$source_vault_file" "$vault_file"
fi
chmod u=rw,go= "$vault_file"
export SKARBIEC_VAULT_FILE="$vault_file"
unset source_vault_file

# The trust material below is per installation and is deliberately absent from
# the release archive. It pins the absolute path and SHA-256 of the binary
# allowed to redeem a capability, so one shared copy would both misidentify the
# workload and put the same proof key in every download. Refuse to start rather
# than run half-provisioned: a partially written directory is how a superseded
# key outlives the re-provision that was supposed to replace it.
if [ -x "$script_dir/provision-skarbiec-trust" ]; then
  provision_hint="$script_dir/provision-skarbiec-trust"
else
  provision_hint="$bundle_root/scripts/provision-skarbiec-trust.sh"
fi
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
export SKARBIEC_DONATION_RECIPIENT=brama-service
export ENTITLEMENTS_ROUTER_BIN
if [ -e "$SKARBIEC_CAP_SOCKET" ] || [ -L "$SKARBIEC_CAP_SOCKET" ]; then
  [ -S "$SKARBIEC_CAP_SOCKET" ] && [ ! -L "$SKARBIEC_CAP_SOCKET" ] || {
    printf '%s\n' "unsafe stale capability socket: $SKARBIEC_CAP_SOCKET" >/dev/stderr
    false
  }
  lsof_bin=$(command -v lsof || true)
  if [ -n "$lsof_bin" ] && "$lsof_bin" -t -- "$SKARBIEC_CAP_SOCKET" >/dev/null 2>&1; then
    printf '%s\n' "Skarbiec capability socket is owned by another process" >/dev/stderr
    false
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

identities = []
for client_id, item, agent_id, allowed_models in sources:
    identity = {"client_id": client_id, "token": field(item, "token")}
    if agent_id is not None:
        identity["agent_id"] = agent_id
    if allowed_models is not None:
        identity["allowed_models"] = allowed_models
    identities.append(identity)
sys.stdout.write(json.dumps(identities, separators=(",", ":")))
PY
)"
[ -n "$BRAMA_MODEL_ROUTER_CLIENT_IDENTITIES" ] || {
  printf '%s\n' "model-router verifier identities are empty" >/dev/stderr
  false
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
    issued = subprocess.run(
        [
            router,
            "capability-issue",
            "--agent", "brama-runtime",
            "--purpose", purpose,
            "--resource", resource,
            "--target", "brama",
            "--ttl", "2592000",
            "--max-uses", "1000000",
        ],
        capture_output=True,
        text=True,
    )
    if issued.returncode:
        detail = issued.stderr.strip() or issued.stdout.strip() or "no detail"
        raise RuntimeError(f"capability issue failed for {resource}: {detail}")
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
        capabilities[provider] = issue("brama.provider.authenticate", resource)
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
        capabilities[item_id] = issue("brama.provider.authenticate", resource)
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
            capabilities[provider] = issue(purpose, resource)

request_capabilities = {
    agent_id: issue("brama.request.sign", f"agent:{agent_id}")
    for agent_id in request_sign_agents
}
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

# Where to listen, and whose hops are already encrypted, both worked out by
# Stado from the placement and the host records rather than written here. A
# host the gateway is not placed on gets a refusal, and the variables stay
# unset: it then binds loopback, which is what an instance nobody placed should
# be reachable as. Moving the gateway needs no edit in this file.
if [ -z "${BRAMA_BIND_ADDRESS:-}" ]; then
  if [ -x "$HOME/.stado/bin/stado" ]; then
    stado_bin="$HOME/.stado/bin/stado"
  else
    stado_bin="$(command -v stado || true)"
  fi
  if [ -n "$stado_bin" ]; then
    serving="$("$stado_bin" service directory bind brama 2>/dev/null || true)"
    for line in $serving; do
      case "$line" in
        bind_address=*) BRAMA_BIND_ADDRESS="${line#bind_address=}" ;;
        encrypted_peers=*) BRAMA_ENCRYPTED_PEER_IPS="${line#encrypted_peers=}" ;;
      esac
    done
    if [ -n "${BRAMA_BIND_ADDRESS:-}" ]; then
      export BRAMA_BIND_ADDRESS
      printf 'serving on %s for the fleet\n' "$BRAMA_BIND_ADDRESS"
    fi
    if [ -n "${BRAMA_ENCRYPTED_PEER_IPS:-}" ]; then
      export BRAMA_ENCRYPTED_PEER_IPS
    fi
  fi
fi

exec "$BRAMA_BIN" serve --port "${BRAMA_PORT_OVERRIDE:-${PORT:-8080}}"

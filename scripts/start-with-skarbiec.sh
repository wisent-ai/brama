#!/bin/sh
set -eu
umask 077

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
bundle_root=$(CDPATH= cd -- "$script_dir/.." && pwd)
if [ -x "$bundle_root/bin/brama" ] && [ -d "$bundle_root/etc/brama-skarbiec" ]; then
  default_brama_bin="$bundle_root/bin/brama"
  default_router_bin="$bundle_root/bin/skarbiec-entitlements-router"
  default_stado_bin="$bundle_root/bin/stado"
  default_config_dir="$bundle_root/etc/brama-skarbiec"
else
  default_brama_bin=/usr/local/bin/brama
  default_router_bin=/usr/local/bin/skarbiec-entitlements-router
  default_stado_bin=/usr/local/bin/stado
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

BRAMA_BIN=${BRAMA_BIN:-"$default_brama_bin"}
ENTITLEMENTS_ROUTER_BIN=${ENTITLEMENTS_ROUTER_BIN:-"$default_router_bin"}
STADO_BIN=${STADO_BIN:-"$default_stado_bin"}
config_dir=${BRAMA_SKARBIEC_CONFIG_DIR:-"$default_config_dir"}
runtime_dir=${BRAMA_RUNTIME_DIR:-/tmp/brama-skarbiec}
socket_dir="$runtime_dir/socket"
gnupg_dir="$runtime_dir/gnupg"
worm_dir="$runtime_dir/worm"
mkdir -p "$runtime_dir" "$socket_dir" "$gnupg_dir" "$worm_dir"
chmod u=rwx,go= "$runtime_dir" "$gnupg_dir" "$worm_dir"
chmod u=rwx,g=rx,o= "$socket_dir"

secret_source=${BRAMA_SECRET_SOURCE:-stado}
case "$secret_source" in
  local-vault)
    : "${BRAMA_GNUPG_HOME:?BRAMA_GNUPG_HOME is required for local-vault secrets}"
    [ -d "$BRAMA_GNUPG_HOME" ] || {
      printf '%s\n' "BRAMA_GNUPG_HOME is not a directory" >/dev/stderr
      false
    }
    export GNUPGHOME="$BRAMA_GNUPG_HOME"
    ;;
  stado)
    private_key_file="$runtime_dir/vault-private-key.asc"
    trap 'rm -f "$private_key_file"' EXIT HUP INT TERM
    [ -x "$STADO_BIN" ] || { printf '%s\n' "STADO_BIN is not executable: $STADO_BIN" >/dev/stderr; false; }
    STADO_CONFIG=${BRAMA_SKARBIEC_STADO_CONFIG:-${HOME:-/nonexistent}/.config/stado/brama-service.json} "$STADO_BIN" secrets get brama-service --field gpg_private_key >"$private_key_file"
    chmod u=rw,go= "$private_key_file"
    export GNUPGHOME="$gnupg_dir"
    gpg --batch --quiet --import "$private_key_file"
    rm -f "$private_key_file"
    trap - EXIT HUP INT TERM
    unset private_key_file BRAMA_SKARBIEC_STADO_CONFIG STADO_CONFIG
    ;;
  *)
    printf '%s\n' "BRAMA_SECRET_SOURCE must be stado or local-vault" >/dev/stderr
    false
    ;;
esac
# Public recipient keys (owner + recovery) keep donations recoverable without
# exposing any provider credential to Stado or the service configuration.
gpg --batch --quiet --import "$config_dir/recipient-public-keys.asc"

vault_file="$runtime_dir/vault.json"
if [ -n "${SKARBIEC_VAULT_FILE:-}" ]; then
  source_vault_file=$SKARBIEC_VAULT_FILE
  if [ ! -f "$source_vault_file" ]; then
    printf '%s\n' "SKARBIEC_VAULT_FILE is not a regular file" >/dev/stderr
    false
  fi
  if [ "$source_vault_file" != "$vault_file" ]; then
    cp "$source_vault_file" "$vault_file"
  fi
else
  : "${SKARBIEC_VAULT_URI:?set SKARBIEC_VAULT_FILE or SKARBIEC_VAULT_URI}"
  case "$SKARBIEC_VAULT_URI" in
    stado://entitlements-rotator/[!/]*) ;;
    *)
      printf '%s\n' "SKARBIEC_VAULT_URI must use stado://entitlements-rotator/<key>" >/dev/stderr
      false
      ;;
  esac
  if [ ! -x "$STADO_BIN" ]; then
    printf '%s\n' "STADO_BIN is not executable: $STADO_BIN" >/dev/stderr
    false
  fi
  if [ -n "${STADO_API_TOKEN_FILE:-}" ]; then [ -f "$STADO_API_TOKEN_FILE" ] || { printf '%s\n' "STADO_API_TOKEN_FILE is not a regular file" >/dev/stderr; false; }; STADO_API_TOKEN=$(cat "$STADO_API_TOKEN_FILE"); [ -n "$STADO_API_TOKEN" ] || { printf '%s\n' "STADO_API_TOKEN_FILE is empty" >/dev/stderr; false; }; export STADO_API_TOKEN; fi; "$STADO_BIN" storage get "$SKARBIEC_VAULT_URI" "$vault_file"
fi
chmod u=rw,go= "$vault_file"
export SKARBIEC_VAULT_FILE="$vault_file"
unset source_vault_file STADO_API_TOKEN

export SKARBIEC_CAP_TRUST_ROOT="$config_dir/trust.json"
export SKARBIEC_CAP_POLICY="$config_dir/policy.json"
export SKARBIEC_CAP_POLICY_SIG="$config_dir/policy.sig"
export SKARBIEC_WORKLOAD_REGISTRY="$config_dir/registry.json"
export SKARBIEC_WORKLOAD_REGISTRY_SIG="$config_dir/registry.sig"
export SKARBIEC_CAP_STATE="$runtime_dir/capability.sqlite"
export SKARBIEC_CAP_SOCKET="$socket_dir/broker.sock"
SKARBIEC_CAP_SOCKET_GID=$(id -g)
export SKARBIEC_CAP_SOCKET_GID
export SKARBIEC_WORM_RECEIPT_DIR="$worm_dir"
export SKARBIEC_WORM_RECEIPT_COMMAND="$config_dir/worm-receipt"
export SKARBIEC_WORM_CHECKPOINT="$runtime_dir/checkpoint.json"
export SKARBIEC_WORKLOAD_ID=brama-service
export SKARBIEC_WORKLOAD_SIGNING_KEY_FILE="$config_dir/brama-proof.key"
export SKARBIEC_DONATION_RECIPIENT=brama-service
export ENTITLEMENTS_ROUTER_BIN

# The central Stado service document is the sole source of Brama's nonsecret
# Wisent-backend ingress and provider policy. Service env files may select the
# document but may not override individual policy values.
control_config=${BRAMA_CONTROL_CONFIG:-${HOME:-/nonexistent}/.config/stado/config.json}
[ -f "$control_config" ] || {
  printf '%s\n' "BRAMA_CONTROL_CONFIG is not a regular file: $control_config" >/dev/stderr
  false
}
policy_dir=$(mktemp -d "$runtime_dir/policy.XXXXXX")
trap 'rm -rf "$policy_dir"' EXIT HUP INT TERM
python3 - "$control_config" "$policy_dir/allowed-models" "$policy_dir/model-aliases" <<'PY'
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
expected_providers = {"local-openai", "openai", "qwen"}
if (
    not isinstance(required_providers, list)
    or set(required_providers) != expected_providers
    or len(required_providers) != len(expected_providers)
):
    raise SystemExit(
        "services.brama.required_provider_capabilities must contain the exact alias providers"
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

# Read every accepted bearer from its dedicated Skarbiec item. Hosted
# deployments use the verifier-only Stado consumer; a host-local deployment
# uses a GPG recipient that was granted only these exact items.
model_reader_config=${BRAMA_MODEL_ROUTER_VERIFIER_STADO_CONFIG:-}
if [ "$secret_source" = stado ] && [ -z "$model_reader_config" ]; then
  printf '%s\n' "BRAMA_MODEL_ROUTER_VERIFIER_STADO_CONFIG is required" >/dev/stderr
  false
fi
: "${BRAMA_ALLOWED_MODELS:?set exact closed Brama model allowlist}"
BRAMA_MODEL_ROUTER_CLIENT_IDENTITIES="$(
  python3 - "$STADO_BIN" "$ENTITLEMENTS_ROUTER_BIN" "$secret_source" "$model_reader_config" <<'PY'
import json
import os
import subprocess
import sys

stado, router, source, stado_config = sys.argv[1:]
all_models = os.environ["BRAMA_ALLOWED_MODELS"].split(",")
backend_models = [model for model in all_models if model.startswith("wisent-backend/")]
weles_models = ["weles/agent/primary"]
sources = [
    ("content-platform-production", "content-platform-production-model-router", "content-platform", None),
    ("echo", "echo-model-router", None, None),
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
    ("brama-operations", "brama-operations-model-router", "wisent-app", None),
]

def field(item, name):
    if source == "local-vault":
        result = subprocess.run(
            [router, "get", item],
            check=True,
            capture_output=True,
            text=True,
            env=os.environ,
        )
        value = json.loads(result.stdout).get(name)
    else:
        environment = os.environ.copy()
        environment["STADO_CONFIG"] = stado_config
        result = subprocess.run(
            [stado, "secrets", "get", item, "--field", name],
            check=True,
            capture_output=True,
            text=True,
            env=environment,
        )
        value = result.stdout.rstrip("\r\n")
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
unset model_reader_config BRAMA_MODEL_ROUTER_VERIFIER_STADO_CONFIG BRAMA_ALLOWED_MODELS

# Content Platform, Oko, and Weles identities are read from their exact
# centrally managed items.
request_reader_config=${BRAMA_REQUEST_SIGN_STADO_CONFIG:-}
if [ "$secret_source" = stado ] && [ -z "$request_reader_config" ]; then
  printf '%s\n' "BRAMA_REQUEST_SIGN_STADO_CONFIG is required" >/dev/stderr
  false
fi
BRAMA_REQUEST_SIGN_IDENTITIES="$(
  python3 - "$STADO_BIN" "$ENTITLEMENTS_ROUTER_BIN" "$secret_source" "$request_reader_config" <<'PY'
import json
import os
import subprocess
import sys

stado, router, source, stado_config = sys.argv[1:]
sources = {
    "content-platform": "content-platform-agent-auth",
    "oko": "oko-model-agent-auth",
    "weles": "weles-model-agent-auth",
}

def field(item, name):
    if source == "local-vault":
        result = subprocess.run(
            [router, "get", item],
            check=True,
            capture_output=True,
            text=True,
            env=os.environ,
        )
        value = json.loads(result.stdout).get(name)
    else:
        environment = os.environ.copy()
        environment["STADO_CONFIG"] = stado_config
        result = subprocess.run(
            [stado, "secrets", "get", item, "--field", name],
            check=True,
            capture_output=True,
            text=True,
            env=environment,
        )
        value = result.stdout.rstrip("\r\n")
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
unset request_reader_config BRAMA_REQUEST_SIGN_STADO_CONFIG

# Weles reauth uses one dedicated Skarbiec item and one accepted runtime name.
# It is not the Weles console token and has no general Weles API scope.
: "${WELES_URL:=https://weles.wisent.ai}"
weles_reader_config=${BRAMA_WELES_REAUTH_STADO_CONFIG:-}
if [ "$secret_source" = stado ] && [ -z "$weles_reader_config" ]; then
  printf '%s\n' "BRAMA_WELES_REAUTH_STADO_CONFIG is required" >/dev/stderr
  false
fi
BRAMA_WELES_REAUTH_TOKEN="$(
  python3 - "$STADO_BIN" "$ENTITLEMENTS_ROUTER_BIN" "$secret_source" "$weles_reader_config" <<'PY'
import json
import os
import subprocess
import sys

stado, router, source, stado_config = sys.argv[1:]
if source == "local-vault":
    result = subprocess.run(
        [router, "get", "brama-weles-reauth"],
        check=True,
        capture_output=True,
        text=True,
        env=os.environ,
    )
    value = json.loads(result.stdout).get("token")
else:
    environment = os.environ.copy()
    environment["STADO_CONFIG"] = stado_config
    result = subprocess.run(
        [stado, "secrets", "get", "brama-weles-reauth", "--field", "token"],
        check=True,
        capture_output=True,
        text=True,
        env=environment,
    )
    value = result.stdout.rstrip("\r\n")
if not isinstance(value, str) or not value or value.strip() != value:
    raise RuntimeError("brama-weles-reauth/token is empty or malformed")
sys.stdout.write(value)
PY
)"
export WELES_URL BRAMA_WELES_REAUTH_TOKEN
unset weles_reader_config BRAMA_WELES_REAUTH_STADO_CONFIG secret_source

subscriptions_file="$runtime_dir/subscriptions.json"
capabilities_file="$runtime_dir/provider-capabilities.json"
request_capabilities_file="$runtime_dir/request-sign-capabilities.json"
catalog_file="$runtime_dir/subscription-catalog.json"
"$ENTITLEMENTS_ROUTER_BIN" list >"$subscriptions_file"
python3 - \
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
subscription_agents = sorted([*request_sign_agents, "content-platform", "oko"])
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
    match resource_id.split(":"):
        case ["provider", provider_name]:
            provider = normalize(provider_name)
            resource = f"provider:{provider}"
            if ("brama.provider.authenticate", resource) not in allowed:
                continue
            capabilities[provider] = issue("brama.provider.authenticate", resource)
        case ["provider", provider_name, item_id]:
            provider = normalize(provider_name)
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
        case _:
            continue

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

exec "$BRAMA_BIN" serve --port "${PORT:-8080}"

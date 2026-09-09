# The product-owned control document is the sole source of Brama's nonsecret
# ingress and provider policy. Service env files may select the document but may
# not override individual policy values.
# A release candidate is launched by the Stado release agent, which passes only
# `runtime.environment` from this product's manifest -- and the manifest declares
# none, so `BRAMA_CONTROL_CONFIG` arrives unset. The path chosen when it is
# unset used to be `~/.config/brama/control.json`, which does not exist on the host that runs
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
# My first version of this resolution invented `BRAMA_SERVICE_ENV`, which would have
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
"$PYTHON_BIN" - "$control_config" "$policy_dir/allowed-models" "$policy_dir/model-aliases" "$policy_dir/backend-models" <<'PY'
import json
import os
import stat
import sys

arguments = iter(sys.argv)
next(arguments)
config_path, allowed_path, aliases_path, backend_path = arguments
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
    "wisent-backend",
    "wisent-backend/evaluation",
    "wisent-backend/embeddings",
    "wisent-backend/moderation",
    "weles",
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
    or ("/" not in route and route != "best")
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
write_policy(
    backend_path,
    json.dumps(sorted(alias for alias in required_aliases if alias == "wisent-backend" or alias.startswith("wisent-backend/"))),
)
PY
BRAMA_ALLOWED_MODELS=$(cat "$policy_dir/allowed-models")
BRAMA_MODEL_ALIASES=$(cat "$policy_dir/model-aliases")
backend_models=$(cat "$policy_dir/backend-models")
export BRAMA_ALLOWED_MODELS BRAMA_MODEL_ALIASES
rm -rf "$policy_dir"
trap - EXIT HUP INT TERM
unset policy_dir control_config BRAMA_CONTROL_CONFIG

# Persist operator route changes outside immutable releases. The initial
# registry mirrors the exact validated launch aliases; later writes are atomic
# and owner-only in Brama's runtime. The document carries exactly the three
# fields the registry reader accepts -- `schema_version`, `deployments` and
# `routes` -- because that reader refuses an unknown field by name, so any
# fourth key written here would make the gateway reject its own file at start.
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

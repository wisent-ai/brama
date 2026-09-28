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
"$BRAMA_BIN" launcher policy --config "$control_config" --allowed "$policy_dir/allowed-models" \
  --aliases "$policy_dir/model-aliases" --backend "$policy_dir/backend-models"
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
  BRAMA_MODEL_ALIASES="$BRAMA_MODEL_ALIASES" "$BRAMA_BIN" launcher seed-routes "$BRAMA_INFERENCE_ROUTES_FILE"
fi
export BRAMA_INFERENCE_ROUTES_FILE
unset routes_dir

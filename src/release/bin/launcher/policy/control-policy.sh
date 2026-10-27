# The product-owned control document is the sole source of Brama's nonsecret
# ingress and provider policy. Service env files may select the document but may
# not override individual policy values.
# Release candidates and the stable service read the same product-owned control
# document selected by the service environment. A missing declaration is a
# launch refusal, not a value chosen by the launcher.
: "${BRAMA_CONTROL_CONFIG:?BRAMA_CONTROL_CONFIG must name the service policy document}"
control_config=$BRAMA_CONTROL_CONFIG
[ -f "$control_config" ] || {
  printf '%s\n' "BRAMA_CONTROL_CONFIG is not a regular file: $control_config" >/dev/stderr
  false
}
printf '%s\n' "control:  $control_config" >/dev/stderr
policy_dir=$(mktemp -d "$runtime_dir/policy.XXXXXX")
trap 'rm -rf "$policy_dir"' EXIT HUP INT TERM
"$BRAMA_BIN" launcher policy --config "$control_config" --allowed "$policy_dir/allowed-models" \
  --aliases "$policy_dir/model-aliases" --backend "$policy_dir/backend-models" \
  --lifetime "$policy_dir/capability-lifetime"
BRAMA_ALLOWED_MODELS=$(cat "$policy_dir/allowed-models")
BRAMA_MODEL_ALIASES=$(cat "$policy_dir/model-aliases")
BRAMA_CAPABILITY_LIFETIME_SECONDS=$(cat "$policy_dir/capability-lifetime")
backend_models=$(cat "$policy_dir/backend-models")
export BRAMA_ALLOWED_MODELS BRAMA_MODEL_ALIASES BRAMA_CAPABILITY_LIFETIME_SECONDS
rm -rf "$policy_dir"
trap - EXIT HUP INT TERM
unset policy_dir control_config BRAMA_CONTROL_CONFIG

# Persist operator route changes outside immutable releases. The initial
# registry mirrors the exact validated launch aliases; later writes are atomic
# and owner-only in Brama's runtime. The document carries exactly the three
# fields the registry reader accepts -- `schema_version`, `deployments` and
# `routes` -- because that reader refuses an unknown field by name, so any
# fourth key written here would make the gateway reject its own file at start.
: "${BRAMA_INFERENCE_ROUTES_FILE:?BRAMA_INFERENCE_ROUTES_FILE must name the owner-only route registry}"
routes_dir=$(dirname "$BRAMA_INFERENCE_ROUTES_FILE")
mkdir -p "$routes_dir"
chmod u=rwx,go= "$routes_dir"
if [ ! -e "$BRAMA_INFERENCE_ROUTES_FILE" ]; then
  BRAMA_MODEL_ALIASES="$BRAMA_MODEL_ALIASES" "$BRAMA_BIN" launcher seed "$BRAMA_INFERENCE_ROUTES_FILE"
fi
export BRAMA_INFERENCE_ROUTES_FILE
unset routes_dir

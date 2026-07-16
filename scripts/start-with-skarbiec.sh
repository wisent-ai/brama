#!/bin/sh
set -eu
umask 077

runtime_dir=/tmp/brama-skarbiec
socket_dir="$runtime_dir/socket"
gnupg_dir="$runtime_dir/gnupg"
worm_dir="$runtime_dir/worm"
mkdir -p "$runtime_dir" "$socket_dir" "$gnupg_dir" "$worm_dir"
chmod 700 "$runtime_dir" "$gnupg_dir" "$worm_dir"
chmod 750 "$socket_dir"

: "${SKARBIEC_GPG_PRIVATE_KEY_FILE:?SKARBIEC_GPG_PRIVATE_KEY_FILE is required}"
: "${SKARBIEC_VAULT_GCS_BUCKET:?SKARBIEC_VAULT_GCS_BUCKET is required}"
: "${SKARBIEC_VAULT_GCS_OBJECT:=skarbiec.vault.json}"

export GNUPGHOME="$gnupg_dir"
gpg --batch --quiet --import "$SKARBIEC_GPG_PRIVATE_KEY_FILE"

metadata_token_json="$(curl --fail --silent --show-error \
  -H 'Metadata-Flavor: Google' \
  'http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token')"
access_token="$(printf '%s' "$metadata_token_json" | python3 -c 'import json,sys; print(json.load(sys.stdin)["access_token"])')"
encoded_object="$(printf '%s' "$SKARBIEC_VAULT_GCS_OBJECT" | python3 -c 'import sys,urllib.parse; print(urllib.parse.quote(sys.stdin.read(), safe=""))')"
vault_file="$runtime_dir/vault.json"
curl --fail --silent --show-error \
  -H "Authorization: Bearer $access_token" \
  "https://storage.googleapis.com/download/storage/v1/b/$SKARBIEC_VAULT_GCS_BUCKET/o/$encoded_object?alt=media" \
  -o "$vault_file"
chmod 600 "$vault_file"
unset access_token metadata_token_json

config_dir=/etc/brama-skarbiec
export SKARBIEC_VAULT_FILE="$vault_file"
export SKARBIEC_CAP_TRUST_ROOT="$config_dir/trust.json"
export SKARBIEC_CAP_POLICY="$config_dir/policy.json"
export SKARBIEC_CAP_POLICY_SIG="$config_dir/policy.sig"
export SKARBIEC_WORKLOAD_REGISTRY="$config_dir/registry.json"
export SKARBIEC_WORKLOAD_REGISTRY_SIG="$config_dir/registry.sig"
export SKARBIEC_CAP_STATE="$runtime_dir/capability.sqlite"
export SKARBIEC_CAP_SOCKET="$socket_dir/broker.sock"
export SKARBIEC_CAP_SOCKET_GID=10001
export SKARBIEC_WORM_RECEIPT_DIR="$worm_dir"
export SKARBIEC_WORM_RECEIPT_COMMAND="$config_dir/worm-receipt"
export SKARBIEC_WORM_CHECKPOINT="$runtime_dir/checkpoint.json"
export SKARBIEC_WORKLOAD_ID=brama-cloudrun
export SKARBIEC_WORKLOAD_SIGNING_KEY_FILE="$config_dir/brama-proof.key"
export ENTITLEMENTS_ROUTER_BIN=/usr/local/bin/skarbiec-entitlements-router

subscription_issue="$($ENTITLEMENTS_ROUTER_BIN capability-issue \
  --agent brama-runtime \
  --purpose brama.provider.authenticate \
  --resource provider:claude-code:brama-sub-wisent-app-claude-primary \
  --target brama \
  --ttl 2592000 \
  --max-uses 1000000)"
subscription_capability="$(printf '%s' "$subscription_issue" | python3 -c 'import json,sys; print(json.load(sys.stdin)["capability_id"])')"
request_issue="$($ENTITLEMENTS_ROUTER_BIN capability-issue \
  --agent brama-runtime \
  --purpose brama.request.sign \
  --resource agent:wisent-app \
  --target brama \
  --ttl 2592000 \
  --max-uses 1000000)"
request_capability="$(printf '%s' "$request_issue" | python3 -c 'import json,sys; print(json.load(sys.stdin)["capability_id"])')"
export BRAMA_PROVIDER_CAPABILITY_IDS="{\"brama-sub-wisent-app-claude-primary\":\"$subscription_capability\"}"
export BRAMA_REQUEST_SIGN_CAPABILITY_IDS="{\"wisent-app\":\"$request_capability\"}"
export BRAMA_SUBSCRIPTION_CATALOG='{"items":[{"id":"brama-sub-wisent-app-claude-primary","provider":"claude_code","agent_id":"wisent-app","status":"active"}]}'
unset subscription_issue subscription_capability request_issue request_capability

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

exec /usr/local/bin/brama serve --port "${PORT:-8080}"

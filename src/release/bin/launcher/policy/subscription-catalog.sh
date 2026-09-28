subscriptions_file="$runtime_dir/subscriptions.json"
catalog_file="$runtime_dir/subscription-catalog.json"
# Releases before 0.2.52 persisted boot-time capability ids here. They are
# single-use authority records, not durable service state.
rm -f "$runtime_dir/provider-capabilities.json" "$runtime_dir/request-sign-capabilities.json"
printf '%s\n' "reading subscription catalog" >/dev/stderr
"$ENTITLEMENTS_ROUTER_BIN" list >"$subscriptions_file"
printf '%s\n' "read subscription catalog; building runtime catalog" >/dev/stderr
"$BRAMA_BIN" launcher catalog --available "$subscriptions_file" \
  --policy "$config_dir/policy.json" --output "$catalog_file"
printf '%s\n' "built runtime catalog; capabilities issue on demand" >/dev/stderr
export BRAMA_SUBSCRIPTION_CATALOG="$(cat "$catalog_file")"
# Boot-time ids are single-use and cannot be refreshed inside a running process.
# Clear inherited values so every credential use asks the authority at its final
# use boundary instead of trying a stale seed first.
unset BRAMA_PROVIDER_CAPABILITY_IDS BRAMA_REQUEST_SIGN_CAPABILITY_IDS

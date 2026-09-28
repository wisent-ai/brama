# Preload only the two bearers whose exact alias sets Brama validates at
# startup. Every other bearer is resolved through Skarbiec introspection on its
# first request. Each router invocation loads the large vault, so bounded
# concurrent reads keep required startup work inside Stado's candidate deadline.
: "${BRAMA_ALLOWED_MODELS:?set exact closed Brama model allowlist}"
identities_file="$runtime_dir/model-router-client-identities.json"
printf '%s\n' "reading required model-router identities" >/dev/stderr
"$BRAMA_BIN" launcher model-router-identities --router "$ENTITLEMENTS_ROUTER_BIN" \
  --backend-models "$backend_models" >"$identities_file"
printf '%s\n' "read required model-router identities" >/dev/stderr
BRAMA_MODEL_ROUTER_CLIENT_IDENTITIES=$(cat "$identities_file")
rm -f "$identities_file"
# Unknown bearers are intentionally absent here and resolved by the
# introspection grant configured above.
export BRAMA_MODEL_ROUTER_CLIENT_IDENTITIES
unset BRAMA_ALLOWED_MODELS backend_models

# Product request-sign identities are projected from their exact Skarbiec items.
# `wisent-app` is Jeden's public runtime identity and uses the dedicated
# `agent:wisent-app` item rather than a product-specific `agent_auth_secret`.
BRAMA_REQUEST_SIGN_IDENTITIES="$(
printf '%s\n' "reading request-sign identities" >/dev/stderr
  "$BRAMA_BIN" launcher request-sign-identities --router "$ENTITLEMENTS_ROUTER_BIN"
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
  "$BRAMA_BIN" launcher item-field --router "$ENTITLEMENTS_ROUTER_BIN" brama-weles-reauth token
)"
printf '%s\n' "read Brama-Weles reauthentication identity" >/dev/stderr
[ -n "$BRAMA_WELES_REAUTH_TOKEN" ] || {
  printf '%s\n' "Brama-Weles reauthentication token is empty" >/dev/stderr
  false
}
export BRAMA_WELES_REAUTH_TOKEN

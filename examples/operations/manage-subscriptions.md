# List, donate, and retire subscriptions

**Goal:** operate the agent-scoped subscription lifecycle without exposing provider credentials.

**Status:** written against the `0.1.0` contract and not re-verified against the newest published release.

**Risk:** listing is authenticated read-only; donation and retirement are security mutations. Donation may involve provider OAuth outside Brama. Explicit owner approval is required for every mutation.

**Environment:** approved HTTPS or authenticated loopback.

**Preconditions:** dedicated bearer, matching `AGENT_ID`, current HMAC signature over exact body bytes, Skarbiec capability broker, and owner authorization. Use the signing procedure in [`../core/call-agent-selectors.md`](../core/call-agent-selectors.md); GET and DELETE-without-body use an empty body hash.

**Inputs:** donation provider/label plus a provider credential read from an owner-only capability file; retirement body names the exact subscription. The current donation API necessarily carries the credential in the body over HTTPS, so the temporary body must be owner-only, short-lived, and never committed or logged.

**Artifacts and side effects:** donation creates/updates vault-owned capability state; retirement appends a non-secret local marker and prevents future dispatch. Mutation audit evidence is retained by owning systems.

## List owned subscriptions

```bash
curl --fail --silent --show-error \
  -H "Authorization: Bearer $BRAMA_BEARER" \
  -H "x-agent-id: $AGENT_ID" \
  -H "x-agent-timestamp: $AGENT_TIMESTAMP" \
  -H "x-agent-signature: $AGENT_SIGNATURE" \
  "$BRAMA_URL/v1/subscriptions/$AGENT_ID"
```

Expected result is metadata for that exact agent only; credential bytes never appear.

## Donate Claude Code access

After explicit owner approval, materialize the provider credential through its approved consumer at `PROVIDER_CREDENTIAL_FILE`, then create an owner-only exact body without placing the credential in shell history:

```bash
umask 077
python3 - "$PROVIDER_CREDENTIAL_FILE" >donation.json <<'PY'
import json, pathlib, sys
credential = pathlib.Path(sys.argv[1]).read_text()
json.dump({"provider":"claude_code","label":"approved-delegation","api_key":credential}, sys.stdout, separators=(",", ":"))
PY
```


Sign the exact `donation.json` bytes using the procedure in [`../core/call-agent-selectors.md`](../core/call-agent-selectors.md), then send:

```bash
curl --fail --silent --show-error \
  -X POST \
  -H "Authorization: Bearer $BRAMA_BEARER" \
  -H "x-agent-id: $AGENT_ID" \
  -H "x-agent-timestamp: $AGENT_TIMESTAMP" \
  -H "x-agent-signature: $AGENT_SIGNATURE" \
  -H 'Content-Type: application/json' \
  --data-binary @donation.json \
  "$BRAMA_URL/v1/subscriptions/$AGENT_ID"
```

Success is endpoint JSON identifying metadata/state, not a returned secret.

## Retire exact delegated access

After explicit owner approval, place only the exact subscription identity in `retire.json`, sign it, then:

```bash
curl --fail --silent --show-error \
  -X DELETE \
  -H "Authorization: Bearer $BRAMA_BEARER" \
  -H "x-agent-id: $AGENT_ID" \
  -H "x-agent-timestamp: $AGENT_TIMESTAMP" \
  -H "x-agent-signature: $AGENT_SIGNATURE" \
  -H 'Content-Type: application/json' \
  --data-binary @retire.json \
  "$BRAMA_URL/v1/subscriptions/$AGENT_ID"
```

## Failure and recovery

`401` means signature/bearer repair; `403` means identity ownership mismatch; `404` means the exact item is not visible; `409 conflict` means lifecycle state prevents the mutation. Do not retry mutation blindly. Re-list metadata, reconcile owner intent, and issue a newly signed exact request. Brama retirement is not vault deletion; restore/re-donate only through the owning lifecycle.

## Cleanup
Delete owner-only request files immediately, unset process-local values, and revoke any temporary credential-file grant. Never delete a capability item merely to make an example appear clean.

## Next

For bounded evidence refresh, see [`collect-task-quality.md`](collect-task-quality.md).

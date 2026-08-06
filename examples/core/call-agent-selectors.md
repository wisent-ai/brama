# Use an agent-owned subscription selector

**Goal:** sign one exact request and let Brama use only subscriptions delegated to that agent within bounded attempts.

**Status:** written against the `0.1.0` contract and not re-verified against the published `0.2.5`.

**Risk:** credentialed, provider-facing, and billable. `any` and `task:` may perform up to six provider calls. Explicit human approval must name agent, account, selector/route, prompt class, output limit, and spend.

**Environment:** approved HTTPS or authenticated loopback; Python standard library for local signature construction.

**Preconditions:** bearer loaded into `BRAMA_BEARER`; exact `AGENT_ID`; its request-sign secret materialized by the approved consumer in owner-only `AGENT_SECRET_FILE`; active delegated subscriptions.

**Inputs:** exact JSON bytes in `request.json`. Reformatting after signing invalidates the signature.

**Artifacts and side effects:** provider quota/cost, process telemetry, and possible retirement journal record after permanent credential rejection.

## Create and sign the exact body

```bash
umask 077
cat >request.json <<'JSON'
{"model":"any","messages":[{"role":"user","content":"Reply with the single word ready."}],"max_tokens":16,"temperature":0}
JSON

read -r AGENT_TIMESTAMP AGENT_SIGNATURE <<EOF
$(python3 - "$AGENT_ID" "$AGENT_SECRET_FILE" request.json <<'PY'
import hashlib, hmac, pathlib, sys, time
agent_id, secret_path, body_path = sys.argv[1:]
timestamp = str(int(time.time()))
body = pathlib.Path(body_path).read_bytes()
secret = pathlib.Path(secret_path).read_bytes()
body_hash = hashlib.sha256(body).hexdigest() if body else ""
message = f"{agent_id}:{timestamp}:{body_hash}".encode()
print(timestamp, hmac.new(secret, message, hashlib.sha256).hexdigest())
PY
)
EOF
```

## Execute after explicit cost approval

```bash
curl --fail --silent --show-error \
  -H "Authorization: Bearer $BRAMA_BEARER" \
  -H "x-agent-id: $AGENT_ID" \
  -H "x-agent-timestamp: $AGENT_TIMESTAMP" \
  -H "x-agent-signature: $AGENT_SIGNATURE" \
  -H 'Content-Type: application/json' \
  --data-binary @request.json \
  "$BRAMA_URL/v1/chat/completions"
```

Expected success has the actual canonical route in `model`, normalized choices, and usage. Protected stats/logs report a bounded attempt count without credential IDs or prompt text.

To use another supported decision, create a new body and signature with exactly one of:

- `"model":"any-vision-capable"` and image-compatible content;
- `"model":"task:<documented-task-name>"` with active quality evidence;
- a canonical route plus `billingTarget` naming the matching provider and active subscription ID.

`subscriptionDecisionId` is not a supported field and is rejected as unknown.

## Failure path

`401 unauthenticated` covers missing/expired/bad HMAC. `403 forbidden` covers bearer-agent/path mismatch. `429 subscription_unavailable` or `provider_rate_limited` means bounded candidates are exhausted. Use `attempts` and `retryable`; never generate an unbounded caller retry loop.

## Cleanup

```bash
rm -f request.json
unset AGENT_TIMESTAMP AGENT_SIGNATURE BRAMA_BEARER AGENT_ID AGENT_SECRET_FILE
```

Do not delete or broaden subscription resources as cleanup.

## Next

Manage the lifecycle boundary in [`../operations/manage-subscriptions.md`](../operations/manage-subscriptions.md).

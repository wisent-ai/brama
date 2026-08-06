# Use the bearer-scoped HTTP API

**Goal:** discover authorized models and, with separate approval, execute one direct provider route.

**Status:** written against the `0.1.0` contract and not re-verified against the published `0.2.5`.

**Risk:** catalog is authenticated read-only and non-billable; chat, embeddings, and moderation are credentialed, provider-facing, and potentially billable.

**Environment:** running Brama over approved HTTPS or authenticated loopback.

**Preconditions:** `BRAMA_URL`; dedicated bearer loaded by its approved consumer into `BRAMA_BEARER`; an allowed alias and matching direct provider capability. Provider-facing steps require explicit owner approval of route, input, token limit, account, and spend.

**Inputs:** synthetic non-sensitive text only. The example uses no production prompt or personal data.

**Artifacts and side effects:** catalog access creates only bounded logs/cache reads. Provider-facing calls may consume quota/money and update process telemetry.

## Discover models first

```bash
curl --fail --silent --show-error \
  -H "Authorization: Bearer $BRAMA_BEARER" \
  "$BRAMA_URL/v1/models"
```

Expected OpenAI-compatible shape contains `object: "list"` and `data` entries with `id` and `owned_by`. Bearer-only discovery does not discover agent subscriptions.

## Execute one approved chat

Create the exact body in an owner-only file:

```bash
umask 077
cat >request.json <<'JSON'
{"model":"wisent-backend/chat/primary","messages":[{"role":"user","content":"Reply with the single word ready."}],"max_tokens":16,"temperature":0}
JSON
```

After explicit provider-cost approval:

```bash
curl --fail --silent --show-error \
  -H "Authorization: Bearer $BRAMA_BEARER" \
  -H 'Content-Type: application/json' \
  --data-binary @request.json \
  "$BRAMA_URL/v1/chat/completions"
```

Expected shape contains `id`, `object: "chat.completion"`, the logical alias in `model`, one `choices` entry, and `usage`. Provider text is intentionally not predicted here.

Embeddings and moderation follow the same bearer boundary with their exact configured aliases and the schemas documented by `/v1/embeddings` and `/v1/moderations`; run them only under separate provider approval.

## Failure path

`400 invalid_request` means the body, alias, or limit is unsupported. `403 forbidden` means the bearer is valid but its closed allowlist does not include the logical model. `502 provider_failure`, `503 dependency_unavailable`, and `504 dependency_timeout` must be handled according to `retryable`; do not replay an ambiguous provider result automatically.

## Cleanup

```bash
rm -f request.json
unset BRAMA_BEARER
```

No provider resource is created by the gateway. Quota consumed by an approved call cannot be undone.

## Next

Agent-owned subscription selection is in [`call-agent-selectors.md`](call-agent-selectors.md).

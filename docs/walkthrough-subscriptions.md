# Walkthrough: reading and repairing the subscription pool

What the subscription surfaces answer on a deployment whose pool is empty —
which is exactly the state every deployment starts in, and the state an
operator is diagnosing when these commands matter. Executed against a
development build of 0.2.38 with isolated state
(`BRAMA_STATE_DIR`, `BRAMA_SUBSCRIPTION_USAGE_FILE`, and
`ENTITLEMENTS_ROUTER_BIN` pointed at temp paths); output pasted as captured.
No provider was contacted and no quota spent — listing, refreshing an empty
pool, and signed discovery are all free by construction.

## 1. The pool report

```console
$ brama subscriptions list
0 of 0 subscription credentials are live

$ brama subscriptions list --json
{
  "providers": []
}
```

The count leads because it is the question that gets this command run: how
many credentials can still serve a `best` call. The command joins the
deployment's subscription listing to the usage ledger — it contacts no
provider, redeems no capability, and writes nothing, so it is safe against a
serving gateway. Row states are `live`, `expired`, `burnt`, `unknown`
([concepts/subscription](concepts/subscription.md#states)).

## 2. Refreshing an empty pool tells you it is empty

```console
$ brama subscription refresh codex --reason "docs walkthrough: exercise the empty-pool refresh path"
provider: codex
attempted: 0
result: failed
detail: no usable `codex` subscription is in this deployment's pool, so no
credential source is configured to refresh: one has to be signed in and
stored in the vault before this command has anything to act on
$ echo $?
1
```

Exit status is non-zero unless a credential was obtained — the caller
running this is trying to repair an empty pool and needs to know from the
status whether it is still empty. The same verdict for a provider with no
OAuth refresh path (`openai`) reads identically: nothing is configured to
refresh.

Every refresh attempt is journaled with its reason, verbatim
(`$BRAMA_STATE_DIR/journal.jsonl`):

```json
{"at":"2026-08-24T22:04:06.461181+00:00","attempted":0,
 "detail":"no usable `codex` subscription is in this deployment's pool, so no credential source is configured to refresh: one has to be signed in and stored in the vault before this command has anything to act on",
 "kind":"subscription_refresh","provider":"codex",
 "reason":"docs walkthrough: exercise the empty-pool refresh path","result":"failed"}
```

## 3. Signed agent discovery

`GET /v1/subscriptions/:agent_id` requires the bearer **and** the agent's
HMAC trio, and the signed agent must equal the path agent
([concepts/entitlement](concepts/entitlement.md)). With a request-sign
identity configured for `wisent-app` and a correctly signed GET:

```console
$ curl -s -H "Authorization: Bearer docs-test-token" \
    -H "x-agent-id: wisent-app" -H "x-agent-timestamp: $ts" -H "x-agent-signature: $sig" \
    http://127.0.0.1:18321/v1/subscriptions/wisent-app
{"subscriptions":[]}

$ # same signature, different path agent
$ curl -s ... http://127.0.0.1:18321/v1/subscriptions/oko
{"error":{"attempts":0,"code":"forbidden","message":"forbidden",...}}   # 403
```

The signature is HMAC-SHA256 over `{agent_id}:{timestamp}:{body_sha256_hex}`
with an empty hash string for a bodyless GET;
[examples/signed-agent-listing.sh](examples/signed-agent-listing.sh) computes
it with `openssl`.

## 4. What an empty pool refuses at request time

A selector from a correctly signed agent that owns nothing:

```console
$ # model="any", signed as wisent-app
{"error":{"attempts":0,"code":"subscription_unavailable",
 "message":"no active stateless provider models for signed agent",
 "retryable":true,"type":"capacity_error"}}            # HTTP 429
```

And `brama test` (which routes through the same subscription dispatch)
against a provider the agent holds no credential for:

```console
$ brama test --model openai/stub-ok --allow-provider-cost
Error: no active 'openai' credential for agent
$ echo $?
1
```

The stderr log beside it carries the operator envelope with
`failure_point: brama.dispatch.credential-selection`
([concepts/failure-point](concepts/failure-point.md)).

## 5. The admin view of the same pool

The `/v1/admin/*` family answers only the `brama-desktop` identity
([concepts/client-identity](concepts/client-identity.md)):

```console
$ curl -s -H "Authorization: Bearer $DESKTOP_TOKEN" http://127.0.0.1:18321/v1/admin/subscription-pool
{"providers":[]}

$ curl -s -X POST -H "Authorization: Bearer $DESKTOP_TOKEN" \
    -H 'Content-Type: application/json' \
    -d '{"provider":"codex","reason":"docs: exercise refresh with empty pool"}' \
    http://127.0.0.1:18321/v1/admin/subscription-pool/refresh
{"attempted":0,"detail":"no usable `codex` subscription is in this deployment's pool, ...",
 "provider":"codex","result":"failed"}                 # HTTP 200

$ curl -s -H "Authorization: Bearer $DESKTOP_TOKEN" http://127.0.0.1:18321/v1/admin/subscriptions/wisent-app
{"agentId":"wisent-app","subscriptions":[]}

$ curl -s -X POST -H "Authorization: Bearer $DESKTOP_TOKEN" -H 'Content-Type: application/json' \
    -d '{}' http://127.0.0.1:18321/v1/admin/subscriptions/wisent-app/sub-missing/probe
{"error":{"attempts":0,"code":"subscription_not_found","message":"subscription not found",...}}  # 404
```

The HTTP refresh reports the verdict in the body with status 200 — the
verdict is the answer; only the CLI maps `failed` to a non-zero exit. The
probe is the one endpoint in the product that would deliberately spend plan
quota, which is why a missing target is a clean `404` and a blocked one is
refused with `409` before any provider call ([http-api](http-api.md)).

## 6. Where a real repair goes from here

On a managed deployment the repair for `burnt`/`needs_reauthorization` rows
is a sign-in that replaces the grant (Brama Desktop, or a donation through
`POST /v1/subscriptions/:agent_id`), then `brama subscription refresh
<provider> --reason "<why>"` to verify the pool answers `refreshed`. The
[runbook](runbook.md) maps each `last_redeem_error` sentence to its repair.

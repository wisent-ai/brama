# HTTP API

Brama serves one HTTP listener, loopback by default (`127.0.0.1:8080`, port
from `brama serve --port`, address from `BRAMA_BIND_ADDRESS`). Every route —
including `/health` — sits behind the transport guard; every route except
`/health` and `/readyz` additionally requires a bearer. Errors on all routes
use the envelope in [errors](errors.md).

## Route table

| Method | Path | Caller | Purpose |
|---|---|---|---|
| GET | `/health` | anyone on an accepted transport | liveness only |
| GET | `/readyz` | anyone on an accepted transport | readiness: redeems real credentials |
| POST | `/v1/chat/completions` | bearer | OpenAI chat completions, buffered or streamed |
| POST | `/v1/messages` | bearer | Anthropic Messages, same routing decision |
| POST | `/v1/responses` | bearer | OpenAI Responses, same routing decision |
| POST | `/v1/embeddings` | bearer | typed embeddings via `wisent-backend/embeddings` |
| POST | `/v1/moderations` | bearer | typed moderation via `wisent-backend/moderation` |
| GET | `/v1/models` | bearer | model catalog scoped to the caller's identity |
| GET | `/v1/subscriptions/:agent_id` | bearer + agent HMAC | list one agent's subscriptions |
| POST | `/v1/subscriptions/:agent_id` | bearer + agent HMAC | donate a subscription credential |
| DELETE | `/v1/subscriptions/:agent_id` | bearer + agent HMAC | retire a subscription |
| GET | `/v1/account/subscriptions` | Wisent user session | list the account's stored keys |
| POST | `/v1/account/subscriptions` | Wisent user session | store a provider API key |
| DELETE | `/v1/account/subscriptions/:subscription_id` | Wisent user session | retire an account key |
| GET | `/stats` | bearer | bounded process telemetry |
| GET | `/v1/admin/snapshot` | `brama-desktop` only | routes and subscription snapshot |
| PUT | `/v1/admin/routes` | `brama-desktop` only | update one alias route |
| GET | `/v1/admin/subscriptions/:agent_id` | `brama-desktop` only | list an agent's subscriptions |
| POST | `/v1/admin/subscriptions/:agent_id` | `brama-desktop` only | store a subscription credential |
| DELETE | `/v1/admin/subscriptions/:agent_id/:subscription_id` | `brama-desktop` only | retire a subscription |
| POST | `/v1/admin/subscriptions/:agent_id/:subscription_id/probe` | `brama-desktop` only | spend one completion to test a credential |
| GET | `/v1/admin/subscription-pool` | `brama-desktop` only | the dispatch pool as the serving process sees it |
| POST | `/v1/admin/subscription-pool/refresh` | `brama-desktop` only | refresh one provider's pooled grants now |

## Transport

A request is accepted only when its peer is loopback, the address the gateway
itself bound, a peer named in `BRAMA_ENCRYPTED_PEER_IPS` (a mesh hop that is
already encrypted), or a proxy named in `BRAMA_TRUSTED_PROXY_IPS` whose
`Forwarded`/`X-Forwarded-Proto` headers state `https`. Anything else answers
`426` with `HTTPS is required except for direct loopback requests`. Forwarded
headers from an unlisted peer are never trusted.

## Authentication

Protected routes take exactly one `Authorization: Bearer <token>` value. The
token is checked in constant time against the identities in
`BRAMA_MODEL_ROUTER_CLIENT_IDENTITIES`; a bearer not in that table is
resolved against Skarbiec (the authority that issued it) and, for
account-scoped desktop routes, against Wisent Identity. Both authorities fail
closed, and the verdict is cached for five seconds so revocation stays
effective without a restart. A model-scoped bearer (one carrying
`allowed_models`) may reach only the six inference and discovery paths plus
the account routes; anything else is `403`. Agent-scoped operations
additionally sign the exact raw body with the HMAC trio described in
[subscriptions](subscriptions.md).

## `/health` versus `/readyz`

`/health` is liveness only and says so in its body:

```json
{"status": "ok", "build": {...}, "dependencies": "not_probed"}
```

It never redeems a credential, lists subscriptions, or contacts a provider —
it answers `ok` from a gateway whose every redemption is being refused.

`/readyz` is the only evidence that the product works. Per request it:

1. redeems one capability per configured provider, through the same broker
   call the request path makes;
2. checks that every active subscription contributes at least one
   discoverable model;
3. redeems one credential per active subscription;
4. reports vault subscription accounts that carry no `brama:agent:` tag —
   accounts no listing can see and no agent can route to.

`ready` is true only when all four pass and at least one provider capability
is configured. Otherwise the status is `503` and `reason` names the failing
half; the body carries `providers` (per-provider `credential` verdicts),
`denied`, `routing`, `unroutable`, `subscriptions` (per-subscription
`redeemable` and the refusal in the words of whatever refused),
`unredeemable`, `unroutable_accounts`, `operator_action_required`, and
`build` — never a secret. Deploy checks and uptime monitors read `/readyz`;
`/health` only proves the process is running.

## Inference

The three chat formats are one workflow: identity, allowlist, alias
resolution, selector semantics, billing ownership, attempt bounds, and the
error contract are identical; only the request and answer shapes differ. Each
requires a `model` naming an allowed alias, a canonical `provider/model`
route, or a selector (`best` via alias, `any`, `any-vision-capable`,
`task:<task-name>`); a missing model is `400 missing field `model``. No
format may guess a provider from a bare vendor model name.

`POST /v1/chat/completions` accepts exactly: `model`, `messages`,
`max_tokens` (default 1024, bounded at 32768), `temperature` (default 0.7,
maximum 2), `tools`, `tool_choice`, `billingTarget`
(`providerId`/`accountId`/`subscriptionId`), and `stream`. Unknown fields are
refused. The Anthropic and Responses formats accept their own dialects;
fields the provider-neutral request cannot hold (stop sequences,
cache-control hints, reasoning options, stored-response identifiers,
non-function tool types) are accepted and dropped rather than approximated.

With `"stream": true` the response is `text/event-stream` in the caller's own
dialect: `chat.completion.chunk` frames closed by `data: [DONE]`, Anthropic
`message_start`/`content_block_*`/`message_stop` events, or `response.*`
events closed by `response.completed`. Rotation across models and credentials
happens only before the first byte; a stream that ends without its terminal
event was cut after commit, and Brama has already stopped — it never resumes
a committed generation on another credential. Streaming a model reachable
only through the shared catalogue is refused as `400 invalid_request` before
any provider is contacted.

`POST /v1/embeddings` and `POST /v1/moderations` are typed alias-only
endpoints on the `wisent-backend/embeddings` and `wisent-backend/moderation`
routes.

`GET /v1/models` combines public catalog metadata with what the caller can
execute: account discovery marks models executable by that account's stored
keys, and signed agent discovery includes agent-owned subscriptions.

## Subscription lifecycle

`GET|POST|DELETE /v1/subscriptions/:agent_id` are always bearer- and
HMAC-protected: the bearer-bound agent, the signed agent, and the path agent
must agree. A `GET` returns, per subscription, the provider-reported plan
windows (`limits`), what Brama measured (`measured`), any rate-limit `block`
in force, `observed_at_ms`, `usage_source` and `stale`, the newest `probe`
verdict, and where the credential stands (`credential.state`: `active`,
`needs_reauthorization`, or `disabled`, with `cause` and instants). `POST`
donates a credential — the value crosses only the request body and the local
entitlements-router stdin pipe, and is never returned. `DELETE` retires a
subscription. Field semantics live in [subscriptions](subscriptions.md).

The `/v1/account/subscriptions` family is the same lifecycle for
authenticated Wisent users: the owner is derived from the verified session,
never from a caller-supplied account or agent identifier, and `POST` accepts
an API key for any supported remote provider except `local-openai`.

## Admin surface

The `/v1/admin/*` family answers only a `brama-desktop` identity with no
model allowlist; every other caller gets `403`. Responses carry identifiers,
usage, and status only; subscription credentials remain write-only.
`POST .../probe` is the one endpoint in the product that deliberately spends
plan quota: one minimal completion against one named subscription, recorded
as a `probe` with source `completion`, refused with `409` when the
subscription is inside a recorded block, and never triggered by any timer.
`GET /v1/admin/subscription-pool` serves the same report as
`brama subscriptions list`; `POST /v1/admin/subscription-pool/refresh` runs
the same audited refresh as `brama subscription refresh` (see [cli](cli.md)).

## `/stats`

Bearer-protected, no dependency probing. Returns build identity, cumulative
`total_requests`, `total_failures`, `total_provider_attempts`,
`total_input_tokens`, `total_output_tokens`, `uptimeSeconds`, per-provider
descriptors with a `configured` flag, per-model latency/throughput telemetry,
the request limits, and the dependency policy. Process telemetry is not
durable billing evidence.

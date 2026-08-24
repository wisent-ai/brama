# HTTP API

Brama serves one HTTP listener, loopback by default (`127.0.0.1:8080`, port
from `brama serve --port`, address from `BRAMA_BIND_ADDRESS`). Every route —
including `/health` — sits behind the transport guard; every route except
`/health` and `/readyz` additionally requires a bearer. Errors on all routes
use the envelope in [errors](errors.md); example bodies below were captured
from a running 0.2.38 gateway.

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
| GET | `/v1/admin/snapshot` | `brama-desktop` only | routes and provider snapshot |
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
(captured):

```json
{"error":{"attempts":0,"code":"secure_transport_required",
 "message":"HTTPS is required except for direct loopback requests",
 "retryable":false,"type":"transport_error"}}
```

with status `426`. Forwarded headers from an unlisted peer are never trusted.

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
additionally sign the exact raw body with the HMAC trio
([concepts/entitlement](concepts/entitlement.md)); a missing header on a
route that needs one answers, for example,
`401 {"message":"missing x-agent-id header"}` (captured on `model:"best"`).

## `GET /health`

Liveness only, no request fields. Captured:

```json
{"build":{"built_at":"...","platform":"...","product":"brama",
 "source_revision":"...","version":"0.2.38"},
 "dependencies":"not_probed","status":"ok"}
```

It never redeems a credential, lists subscriptions, or contacts a provider —
it answers `ok` from a gateway whose every redemption is being refused
([architecture](architecture.md#health-versus-readyz)).

## `GET /readyz`

The only evidence that the product works. Per request it: (1) redeems one
capability per configured provider through the same broker call the request
path makes; (2) checks every active subscription contributes at least one
discoverable model; (3) redeems one credential per active subscription;
(4) reports vault subscription accounts carrying no `brama:agent:` tag.
Captured healthy body:

```json
{"build":{...},"denied":[],"operator_action_required":false,
 "providers":[{"credential":true,"provider":"openai"}],
 "ready":true,
 "reason":"every configured provider credential was obtained, every active subscription redeemed, and every vault account carries the agent tag that makes it routable",
 "routing":[],"subscriptions":[],"unredeemable":[],"unroutable":[],
 "unroutable_accounts":[]}
```

Otherwise status `503` with `ready: false` and `reason` naming the failing
half — the five sentences and their repairs are in the
[runbook](runbook.md#readyz-answers-503). `subscriptions[]` rows carry
per-subscription `redeemable` and the refusal in the words of whatever
refused; never a secret.

## The three inference dialects

`POST /v1/chat/completions`, `POST /v1/messages`, and `POST /v1/responses`
are one workflow: identity, allowlist, alias resolution, selector semantics,
billing ownership, attempt bounds, and the error contract are identical;
only the request and answer shapes differ. Each requires a `model` naming an
allowed alias, a canonical `provider/model` route, or a selector; a missing
model is `400 missing field `model``, and a bare vendor name is
`400 model must be a canonical provider/model route or a supported selector`
(both captured). Shared validation: `max_tokens` default 1024, refused
outside 1..32768 (`max_tokens must be between one and 32768`); `temperature`
default 0.7, refused unless finite and ≤ 2 (`temperature must be finite and
between zero and 2`); `messages must not be empty`.

### `POST /v1/chat/completions`

Accepts exactly: `model`, `messages`, `max_tokens`, `temperature`, `tools`,
`tool_choice`, `billingTarget` (`providerId`/`accountId`/`subscriptionId`),
`stream`. Unknown fields are refused with the deserializer's own sentence
(captured):

```text
400 invalid JSON: unknown field `frequency_penalty`, expected one of `model`,
`messages`, `max_tokens`, `temperature`, `tools`, `tool_choice`,
`billingTarget`, `stream` at line 1 column 93
```

Success (captured):

```json
{"choices":[{"finish_reason":"stop","index":0,
  "message":{"content":"...","role":"assistant"}}],
 "id":"chatcmpl-...","model":"<route>","object":"chat.completion",
 "usage":{"completion_tokens":7,"prompt_tokens":9,"total_tokens":16}}
```

With `"stream": true`: `text/event-stream` of `chat.completion.chunk`
frames — a role delta first, then content deltas, a `finish_reason` frame, a
usage frame, closed by `data: [DONE]` (captured in
[the walkthrough](walkthrough-standalone-stub.md#4-one-routed-completion)).

### `POST /v1/messages`

The Anthropic Messages dialect over the same routing decision. Accepts the
Anthropic request shape (`model`, `max_tokens`, `messages`, system, tools,
streaming); fields the provider-neutral request cannot hold (stop sequences,
cache-control hints, non-function tool types) are accepted and dropped
rather than approximated. Success (captured):

```json
{"content":[{"text":"...","type":"text"}],"id":"msg_...",
 "model":"<route>","role":"assistant","stop_reason":"end_turn",
 "stop_sequence":null,"type":"message",
 "usage":{"input_tokens":9,"output_tokens":7}}
```

Streaming emits `message_start` / `content_block_*` / `message_delta` /
`message_stop` events, no `[DONE]`.

### `POST /v1/responses`

The OpenAI Responses dialect (`input` in place of `messages`; reasoning
options and stored-response identifiers are dropped). Success (captured):

```json
{"created_at":...,"id":"resp_...","model":"<route>","object":"response",
 "output":[{"content":[{"annotations":[],"text":"...","type":"output_text"}],
   "id":"msg_...","role":"assistant","status":"completed","type":"message"}],
 "status":"completed",
 "usage":{"input_tokens":9,"output_tokens":7,"total_tokens":16}}
```

Streaming emits `response.*` events closed by `response.completed`.

### Streaming commit rule

Rotation across models and credentials happens only before the first byte; a
stream that ends without its terminal event was cut after commit, and Brama
has already stopped — it never resumes a committed generation on another
credential. Streaming a model reachable only through the shared catalogue is
refused as `400 invalid_request` before any provider is contacted.

## `POST /v1/embeddings`

Typed alias-only endpoint on the `wisent-backend/embeddings` route. Accepts
exactly `model`, `input`, `encoding_format` (`float` or `base64`),
`dimensions`, `user`; refusals include `invalid embedding input` and
`invalid embedding encoding format`. On a deployment without the embeddings
alias the captured answer is `500 {"code":"internal_error","message":
"embedding alias missing"}`. Success is the OpenAI embeddings shape
(`object: "list"`, `data[].embedding`, `usage`).

## `POST /v1/moderations`

Typed alias-only endpoint on `wisent-backend/moderation`; requires `model`
and `input` (`invalid moderation input` on shape errors). Success is the
OpenAI moderations shape.

## `GET /v1/models`

Combines public catalog metadata with what the caller can execute: account
discovery marks models executable by that account's stored keys, and signed
agent discovery includes agent-owned subscriptions. Captured shape:

```json
{"data":[{"id":"<provider>/<model>","object":"model","owned_by":"<provider>"}, ...]}
```

## Subscription lifecycle (agent-signed)

All three verbs on `/v1/subscriptions/:agent_id` require bearer + HMAC, and
the bearer-bound agent, signed agent, and path agent must agree
(`403 forbidden` on any mismatch — captured). Field semantics are in
[concepts/subscription](concepts/subscription.md).

- **GET** → `{"subscriptions": [...]}` (captured empty:
  `{"subscriptions":[]}`). Each row is the subscription view: `id`,
  `provider`, `status`, `limits` (provider-reported plan windows),
  `measured`, `block`, `observed_at_ms`, `probe`, `credential`
  (`state`: `active` | `needs_reauthorization` | `disabled`, with `cause`
  and instants), `usage_source` (`provider` | `traffic` | `probe`), `stale`
  (`src/core/server.rs:2800`).
- **POST** — body `{"provider", "label"?, "api_key"}` (unknown fields
  refused). Refusals: `provider must name a supported remote API or
  subscription provider` (400), `api_key must contain 1..8000 characters`
  (400), a document that does not reduce to a bearer (400, donor's to fix),
  a failed vault write (500, this installation's). Success:
  `{"subscription": {"id": "brama-sub-<agent>-<provider>-primary",
  "provider", "agent_id", "status": "active", "label"}}`. The credential
  value is never returned.
- **DELETE** — body `{"subscription_id"}`; `subscription_id is required`
  (400), `subscription not found` (404) for a target the signed agent does
  not own. Success `{"ok": true}`. Retirement is a journal record that
  outranks whatever the last refresh concluded, never a vault deletion.

## Account lifecycle (Wisent user session)

The `/v1/account/subscriptions` family is the same lifecycle for
authenticated Wisent users: the owner is derived from the verified session,
never from a caller-supplied account or agent identifier, and `POST` accepts
an API key for any supported remote provider except `local-openai`. A plain
service bearer is refused (`403 forbidden`, captured).

## `GET /stats`

Bearer-protected, no dependency probing. Captured keys: `build`,
`configuredDirectProviders`, `dependencyPolicy` (`{"capabilityBroker":
"final-use","catalog":"lazy","subscriptions":"lazy"}`), `limits`
(`{"maxOutputTokens":32768,"requestDeadlineSeconds":300}`), `models[]`
(per-model `count`, `latencyMs`, `lastLatencyMs`, `tps`, `lastTps`),
`perfModels`, `providers[]` (per-descriptor `id`, `displayName`,
`wireProtocol`, `configured`), plus cumulative `total_requests`,
`total_failures`, `total_provider_attempts`, `total_input_tokens`,
`total_output_tokens`, and `uptimeSeconds`. Process telemetry is not durable
billing evidence.

## Admin surface

The `/v1/admin/*` family answers only a `brama-desktop` identity with no
model allowlist; every other caller gets `403 forbidden` (captured).
Responses carry identifiers, usage, and status only; subscription
credentials remain write-only.

- **GET `/v1/admin/snapshot`** — captured keys: `schemaVersion` (1),
  `automaticRollback` (true), `boundaries`
  (`{"credentials":"skarbiec","releases":"stado","routing":"brama"}`),
  `providers[]` (as `/stats`), `routes`
  (`{"deployments":[],"routes":{},"fallbacks":{}}` from the routes file).
  `503 route registry unavailable` when the configured file will not read.
- **PUT `/v1/admin/routes`** — body exactly
  `{"alias", "primary", "fallbacks"}` (an unknown field is refused with the
  deserializer sentence, captured). Refusals: `409 runtime route registry is
  not configured` (captured — no routes file in effect), `400 unknown route
  alias`, `400 route chain is unsupported, duplicated, or unavailable`,
  `409 route update was rejected`. Success `{"ok": true, "routes": ...}`;
  the write is atomic and validated before rename.
- **GET `/v1/admin/subscriptions/:agent_id`** — captured:
  `{"agentId":"<agent>","subscriptions":[]}`; rows are the same
  subscription view as the signed listing.
- **POST `/v1/admin/subscriptions/:agent_id`** — same body and refusals as
  the agent-signed donation, authenticated by the desktop identity instead
  of the HMAC trio.
- **DELETE `/v1/admin/subscriptions/:agent_id/:subscription_id`** — retire;
  `404 subscription not found` for an unknown target.
- **POST `.../probe`** — the one endpoint in the product that deliberately
  spends plan quota: one minimal completion against one named subscription,
  recorded as a `probe` with source `completion`, refused with `409` when
  the subscription is inside a recorded block, `404 subscription not found`
  otherwise absent (captured), and never triggered by any timer. Success
  `{"ok": true, "probe": {...}, "subscription": {...}}`.
- **GET `/v1/admin/subscription-pool`** — the same report as
  `brama subscriptions list --json` (captured empty:
  `{"providers":[]}`).
- **POST `/v1/admin/subscription-pool/refresh`** — body
  `{"provider", "reason"}`; runs the same audited refresh as
  `brama subscription refresh` and answers the verdict with status 200
  (captured: `{"attempted":0,"detail":"no usable `codex` subscription is in
  this deployment's pool, ...","provider":"codex","result":"failed"}`); the
  verdict is the answer — only the CLI maps `failed` to a non-zero exit.

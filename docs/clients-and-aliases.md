# Client identities and aliases

Who may call the gateway, and what the names they call with mean. A client is
a dedicated bearer bound to an identity; an alias is a deployment-owned name
that resolves to a route. Neither implies the other: a valid bearer never
implies agent authority, and an alias never unlocks a credential its caller
does not own.

## Client identities

Each accepted client is one entry in `BRAMA_MODEL_ROUTER_CLIENT_IDENTITIES`
(a JSON array the launcher reads from dedicated Skarbiec token items):

- `client_id` — lowercase ASCII letters, digits, and `-` only; unique.
- `token` — the dedicated bearer; no whitespace or control bytes; unique
  across clients (compared by SHA-256 digest in constant time).
- `agent_id` (optional) — binds the bearer to one agent. When the request
  also carries agent HMAC headers, the signed `x-agent-id` must equal this
  binding or the request is `403`.
- `allowed_models` (optional) — an exact, wildcard-free model allowlist. A
  bearer that carries one may reach only the inference and discovery paths
  (`/v1/chat/completions`, `/v1/messages`, `/v1/responses`, `/v1/embeddings`,
  `/v1/moderations`, `/v1/models`) and the account routes.

Missing, malformed, duplicate, or contradictory entries fail startup. The
table is a warm start, not a precondition: a bearer absent from it is
resolved against Skarbiec through the `BRAMA_SKARBIEC_CONSUMER` introspection
grant, because the table is a copy taken at boot — it cannot expire, cannot
be revoked, and cannot contain a client registered since the process started.
Desktop user sessions resolve against Wisent Identity
(`BRAMA_WISENT_AUTH_URL` + `BRAMA_WISENT_AUTH_ANON_KEY`) into the
`brama-user` identity, whose account routes derive the owner from the
verified session. Both authorities fail closed; verdicts are cached five
seconds.

Two client ids are special in code: `brama-desktop` (with no model allowlist)
is the only identity the `/v1/admin/*` family answers, and `brama-user` is
the only identity the `/v1/account/*` family serves.

In managed (non-standalone) deployments, startup requires two exact
allowlists as internal consistency checks when the table is present:
`wisent-backend` must hold exactly the five `wisent-backend/*` aliases, and
`weles` must hold exactly `best` — `best` is the only alias that can reach a
subscription-funded model, and a browser-trajectory drafter must not be sent
to whatever deployment happens to be up.

## Agent identity

An agent caller proves who it is by signing the exact raw request body with
the HMAC trio `x-agent-id`, `x-agent-timestamp`, `x-agent-signature`
(HMAC-SHA256 over `{agent_id}:{timestamp}:{body_sha256_hex}`, ±300-second
window). The per-agent secret is resolved immediately before verification:
Echo, legacy Content Platform, Oko, and Weles are strict central-item
projections through `BRAMA_REQUEST_SIGN_IDENTITIES` /
`BRAMA_REQUEST_SIGN_CAPABILITY_IDS`, and they never fall back to another
product's secret. The signed identity is what selects and spends
subscriptions; see [subscriptions](subscriptions.md).

## Aliases

`BRAMA_MODEL_ALIASES` (assembled by the launcher, extended per request by the
routes file) maps alias names to canonical `provider/model` routes. Seven
aliases are required in managed deployments because callers ship against
them:

| Alias | Shape promise |
|---|---|
| `wisent-backend/chat/primary` | chat |
| `wisent-backend/chat/fallback` | chat |
| `wisent-backend/evaluation` | chat |
| `wisent-backend/embeddings` | embeddings only — never handed a chat model |
| `wisent-backend/moderation` | moderation only |
| `weles/agent/primary` | chat |
| `best` | delegation to subscription dispatch |

The required set must be present; any further name is the operator's to
invent (`smol`, `best-vision`, ...) and is accepted on the general-purpose
chat shape without a gateway release. A route containing `*` or stray
whitespace fails startup. A route naming a provider whose capability was
never issued on this host does not fail startup — the gateway starts, serves
the aliases that are serviceable, and logs
`alias_provider_capability_absent`; at request time such an alias resolves to
nothing, exactly as an undeclared alias does.

`best` is the exception to the direct-capability rule: it (and any alias
whose route delegates to `best`) resolves through subscription dispatch,
where the caller's HMAC identity selects the subscription that pays. Brama
deliberately holds no direct credential for it. In the current deployment
policy `best` maps to `codex/gpt-5.3-codex-spark`; a caller still needs both
an allowlisted bearer and the HMAC identity that owns an eligible
subscription.

## Dynamic routes

When `BRAMA_INFERENCE_ROUTES_FILE` is set, the owner-only snapshot it names
is reloaded per request and its routes extend the launch aliases. The file
must not be a symlink or group/other-readable; deployment endpoints must be
loopback or Tailscale IPv4; a fallback chain may only extend an alias that
has a primary route; malformed updates fail closed. `PUT /v1/admin/routes`
persists operator route changes into the same file atomically. A destination
naming `best` passes through as delegation rather than being resolved as a
local deployment.

## Selectors

Three model names are selectors, available to signed agent callers:

- `any` — active agent-owned stateless routes, ordered by the freest usable
  subscription behind each route, exact ties randomized; at most three model
  candidates, two credentials each, six provider calls total.
- `any-vision-capable` — the `any` contract after filtering to catalog models
  whose input modalities include `image`.
- `task:<task-name>` — the latest active quality observation per active
  model, sorted score-first then newest, plan headroom inside one score;
  same three-candidate, six-call bound. Evidence comes only from
  `brama collect-task-quality` (see [cli](cli.md)); Brama never infers task
  names from prompt text.

Selectors stop at the first successful provider result and never retry
without a finite bound.

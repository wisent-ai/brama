# Core functionality contract

The [root README](README.md) defines Brama's outcomes. This document defines the
public workflows, authority, state, limits, failures, recovery, authorization,
and observability that make those outcomes executable.

## Core versus integrations

Core consists of:

- secure HTTP ingress and exact client/agent binding;
- provider-neutral model, message, tool, usage, and error contracts;
- canonical route, alias, selector, billing-target, and bounded-attempt policy;
- final-use capability redemption seams;
- append-only retirement and quality state;
- build identity, health, statistics, CLI, and read-only MCP boundaries.

Provider protocols, models.dev, Skarbiec transport, entitlements router, host deployment,
and external model services are integrations. They may implement a core seam but
must not redefine client identity, ownership, limits, state authority, or public
error semantics.

## Public workflow: direct route or alias

### Inputs

- one accepted bearer bound to a client identity;
- an allowed logical alias or canonical `provider/model` route;
- nonempty messages;
- `max_tokens` from 1 through 32768;
- temperature and optional provider-neutral tool definitions.

### Decision and result

Brama validates transport, bearer, allowlist, request shape, and limits before
redeeming a provider capability. An alias resolves to one deployment-owned
canonical route. A generic canonical request with no caller-scoped fields uses
Brama's direct provider capability and performs at most one provider attempt.
The result is one buffered OpenAI-compatible response or one normalized error.

No subscription credential is eligible for this workflow.

## Public workflow: exact agent subscription

The caller supplies the three agent HMAC headers over the exact raw request body.
The bearer-bound agent, signed agent, and path agent must agree where present.

The deployment alias `-best` resolves to
`claude-code/claude-opus-4-6`. It is accepted only for bearers whose model
allowlist names `-best`; the HMAC identity still selects the subscription owner.
The alias never authorizes a direct Claude credential or another agent's
subscription.

`billingTarget` contains `providerId`, `accountId`, and `subscriptionId`:

- `providerId` must match the canonical route provider;
- `accountId` is required as caller decision evidence but is not credential
  authority;
- `subscriptionId` must resolve to an active non-retired resource owned by the
  signed agent.

An explicit subscription route tries at most two eligible credentials. A
credential is redeemed immediately before each provider call. Authentication,
quota, and rate-limit failures may rotate; other provider failures return
immediately because replay could duplicate cost or tool effects.

## Public workflow: selectors

### `any`

Select active agent-owned stateless routes, randomize equivalent candidates, and
try at most three model candidates. Each candidate may try at most two
credentials. The whole request therefore performs at most six provider calls.

### `any-vision-capable`

Apply the `any` contract after filtering to catalog models whose input
modalities include `image`.

### `task:<task-name>`

Use the latest active quality observation per active model. Sort by score
highest first and observation time newest first; randomize exact-score ties.
Try at most three model candidates and six provider calls total.

Selectors stop at the first successful result. They do not infer task names,
reuse another agent's evidence, or continue beyond the documented limits.

## Task-quality collection

Collection is an explicit billable operator action. It requires:

- one agent ID and task name;
- a nonempty prompt;
- an exact or substring expectation;
- explicit provider-cost acknowledgement;
- `max_models` from 1 through 25, defaulting to 3;
- explicit `--persist` before observations enter the journal.

Each selected model receives one request through the real agent subscription
path. Collection reports per-model status and score and never silently expands
past `max_models`.

## State and ownership

| Resource | Authority | Brama state | Freshness/recovery |
|---|---|---|---|
| Client bearer and binding | Dedicated central token item and service document | Digest in process memory | Restart after policy/token change |
| Request-sign identity | Exact central item or agent capability | Secret in scoped memory only | Re-redeem on request or restart |
| Provider credential | Skarbiec/entitlements resource | Secret in scoped memory only | Re-redeem at final use |
| Subscription inventory | Live entitlements router | Per-agent cache, 60 seconds | Failed live lookup may use trusted startup metadata only |
| Retirement | Brama journal | Append-only `retire` record | Last matching record wins |
| Task quality | Brama journal | Append-only `quality` record | Latest active record per agent/task/model wins |
| Public model metadata | models.dev | memory and replaceable `/tmp` cache | Degraded catalog is observable; never credential authority |
| Performance telemetry | Brama process | bounded map and replaceable `/tmp` file | Best effort; not business authority |
| Donated-subscription metadata | Brama overlay | owner-only atomic JSON rewrite | Credential remains in entitlements authority |
| Plan usage, its source, and the newest check verdict | Provider's own usage report, plus the answer headers of real traffic | owner-only atomic ledger with each reading's instant | Refreshed by the free usage report sweep and by traffic; a last good reading is served while stale and dropped past `BRAMA_PLAN_USAGE_RETENTION_SECS`; an unparsable ledger is treated as empty |

Journal credentials, encrypted secret overlays, and raw provider responses are
prohibited. The journal schema is append-only; incompatible interpretation
requires a release migration contract.

## Failure contract

Public errors use this envelope:

```json
{
  "error": {
    "message": "bounded human-readable detail",
    "type": "stable_class",
    "code": "stable_code",
    "retryable": false,
    "attempts": 0
  }
}
```

| HTTP | Code | Meaning | Caller action |
|---:|---|---|---|
| 400 | `invalid_request` | malformed JSON, route, selector, or limit | Correct the request; do not retry unchanged |
| 401 | `unauthenticated` | bearer or HMAC missing/invalid | Repair or rotate the exact identity |
| 403 | `forbidden` | valid identity lacks model/agent/path authority | Correct grant or request; do not substitute identity |
| 404 | `subscription_not_found` | owned lifecycle target does not exist | Refresh inventory or correct ID |
| 409 | `state_conflict` | requested mutation conflicts with current state | Read current state before retry |
| 426 | `secure_transport_required` | neither loopback nor trusted HTTPS peer | Use approved HTTPS ingress |
| 429 | `provider_rate_limited` | bounded provider quota/rate attempts exhausted | Wait or choose an explicit authorized target |
| 429 | `subscription_unavailable` | no usable agent credential in bound attempts | Repair intended subscription or wait |
| 502 | `provider_failure` | provider returned permanent/malformed failure | Inspect provider classification; retry only if stated |
| 503 | `dependency_unavailable` | required catalog, broker, vault, or provider unavailable | Restore named dependency |
| 504 | `dependency_timeout` | whole Brama request deadline expired | Inspect dependency; retry only when safe |
| 500 | `internal_error` | Brama failed outside a classified dependency | Operator investigation required |

A response states `retryable` explicitly. Error message text is diagnostic, not
the machine contract. Selector/credential replay never exceeds the attempt
count included in the envelope.

## Cancellation, timeout, and retry

- One HTTP inference request has a 300-second whole-request deadline.
- One provider HTTP attempt has a 255-second timeout.
- Model catalog calls have a 30-second timeout.
- Provider model discovery has a 20-second timeout.
- OAuth refresh has a 15-second timeout and bounded response size.
- Direct routes perform one provider attempt.
- Explicit subscription routes perform at most two provider attempts.
- Selectors perform at most three model candidates and six provider attempts.
- Brama does not retry arbitrary 5xx or malformed results automatically.
- Dropping the client connection cancels the in-process future where the HTTP
  stack propagates cancellation; Brama has no durable job to resume.
- A caller must treat an ambiguous provider outcome as potentially billable and
  must not replay automatically unless the returned code is retryable.

## Configuration contract

Startup configuration is generated, not hand-authored. It must:

- reject missing, malformed, empty, duplicate, unknown, or contradictory client
  identities and alias policy;
- use exact model allowlists without wildcards;
- validate capability IDs and expected resource tuples before redemption;
- keep direct and subscription capability names separate;
- reject non-HTTPS provider origins except explicit loopback overrides;
- reject untrusted forwarded transport headers;
- expose build identity and non-secret configuration categories without
  exposing bearer, HMAC, OAuth, or provider material;
- require restart after startup policy changes.

No security, credential, provider, identity, or storage fallback is silent.

## Authorization and secrets

Authorization is enforced at ingress and mutation boundaries. One valid bearer
never implies agent authority. One agent identity never implies access to
another agent path or subscription. Brama receives only capability handles until
the final-use seam, accepts bounded secret bytes from the owner-bound socket,
and zeroizes scoped OAuth material.

Donation accepts secret input only in the authorized request body, passes it to
the entitlements router through child stdin, and stores only non-secret metadata
in Brama's overlay. Logs and errors must never include the donated value.

## Observability contract

Secret-free build identity is available through `brama version` and `/health`.
A routing event records bounded fields:

- request correlation ID;
- client identity and routing mode;
- requested logical model and selected canonical route;
- provider and attempt count;
- success/failure class, retryability, latency, and token totals;
- whether operator action is required.

The provider usage report reader is logged under its own `plan_usage_*` events,
and the operator-triggered completion probe under `usage_probe_*`, each naming
the subscription, the provider, and the provider's refusal when there was one, so
a request nobody made is never read as a caller's traffic.

Do not log bearer values, HMAC signatures, capability IDs, raw credentials,
request bodies, prompt text, raw provider payloads, or donated secrets. `/stats`
is bearer-protected. Process telemetry is not durable billing evidence.

## Resource behavior

- request body and provider response limits are enforced at their protocol seams;
- provider response bodies are capped at 16 MiB before UTF-8 or JSON parsing;
- `max_tokens` is globally bounded at 32768;
- OAuth credential input and response sizes are bounded;
- model and credential attempts are finite;
- the perf registry tracks at most 500 model keys;
- catalog and subscription caches have explicit TTLs;
- the provider usage report sweep is bounded and spends no plan quota: one read
  per active subscription per its own cache window
  (`BRAMA_PLAN_USAGE_TTL_SECS`, spread by up to a quarter either way),
  single-flighted per subscription, no retry inside a sweep, and the whole task
  disabled by setting `BRAMA_PLAN_USAGE_SWEEP_SECS` to `0`;
- the completion probe is bounded to what an operator asks for: one attempt
  against one named subscription per request to
  `POST /v1/admin/subscriptions/:agent_id/:subscription_id/probe`, no retry, no
  rotation to another credential, refused outright for a subscription inside a
  recorded block, and never triggered by a timer -- the default configuration
  performs no quota-consuming request on any schedule;
- the credential refresh sweep is bounded: one refresh attempt per active
  subscription per interval, single-flighted per subscription, no retry inside a
  sweep, a definitively refused credential left alone until a sign-in replaces
  it, and the whole task disabled by setting
  `BRAMA_CREDENTIAL_REFRESH_INTERVAL_SECS` to `0`;
- no unbounded background retry, queue, or provider worker exists;
- append-only journal growth is operator-monitored until a versioned compaction
  contract exists.

## Evolution and completion

Public changes follow [`RELEASE.md`](RELEASE.md). Provider integrations follow
[`INTEGRATIONS.md`](INTEGRATIONS.md). Every supported outcome maps to
[`examples/`](examples/README.md) and approved evidence in
[`TESTING.md`](TESTING.md). Remove obsolete fields and aliases after callers
move; do not keep hidden compatibility paths or competing authorities.

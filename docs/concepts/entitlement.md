# Entitlement

Which account pays for which request. An entitlement is the decision — made
per request, from the caller's proven identity and the request's shape —
about the one pool that may fund it. There is no silent fallback across
providers, agents, accounts, or credentials.

## The signed identity

Spending a [subscription](subscription.md) requires the caller's own signed
HMAC identity: the trio `x-agent-id`, `x-agent-timestamp`,
`x-agent-signature`, where the signature is HMAC-SHA256 over
`{agent_id}:{timestamp}:{body_sha256_hex}` (`body_sha256_hex` is the SHA-256
hex of the exact raw body, or the empty string when there is no body), with
a ±300-second timestamp window and constant-time comparison
(`src/crypto/hmac_auth.rs`). The per-agent secret is resolved immediately
before verification; `echo`, `content-platform`, `oko`, `weles`, `lem`,
`probierz`, and `wisent-app` are strict central-item projections through
`BRAMA_REQUEST_SIGN_IDENTITIES` / `BRAMA_REQUEST_SIGN_CAPABILITY_IDS` and
never fall back to another product's secret (`src/gateway/broker.rs`).

Where present, three identities must agree: the bearer-bound agent, the
signed agent, and the path agent — any contradiction is `403 forbidden`.
A missing trio on a route that needs one is `401`:

```text
$ model="best" without HMAC headers → 401
{"error":{"code":"unauthenticated","message":"missing x-agent-id header", ...}}
```

## Which pool pays

| Request shape | Pays |
|---|---|
| canonical `provider/model` route | the deployment's direct [capability](capability.md) for that provider; one attempt, fallback routes only after a failed attempt |
| alias resolving to a canonical route | same as above |
| `best` (or an alias delegating to it) | a subscription owned by the signed agent |
| `any`, `any-vision-capable`, `task:<name>` | a subscription owned by the signed agent |
| explicit `billingTarget` | the exact named subscription |

`billingTarget` (`providerId`, `accountId`, `subscriptionId`) names an exact
target: `providerId` must match the route's provider, and `subscriptionId`
must resolve to an active, non-retired resource owned by the signed agent.
Selectors never authorize a direct provider credential or another agent's
subscription.

## Bounds

Every call is finitely bounded before the first byte
(`src/subscription_dispatch/dispatch.rs`):

- a direct route: one attempt;
- an explicit subscription route: at most **2** eligible credentials
  (`max_credential_attempts`);
- a selector: at most **3** model candidates (`max_selector_models`), each
  trying up to 2 credentials — at most 6 provider calls, and a refusal that
  never reached a provider costs no budget;
- a provider whose whole pool emptied is skipped for its remaining
  candidates and named once, not twice;
- all inside the 300-second whole-request deadline.

Once a stream commits, nothing is retried on any credential — a second
attempt would double both the bill and the text. Only authentication, quota,
and rate-limit failures rotate credentials; permanent or malformed provider
failures stop replay.

## Ordering and pinning

Candidates are ordered by what their plans have left — the maximum used
fraction the provider itself reported across current windows, freest first,
read from the ledger at no provider cost. A window past its own reset
instant counts as empty; a subscription with no reading counts as free,
because its first call writes the reading that corrects the placement. Exact
ties are randomized so equal accounts stay decorrelated.

The credential that served an agent is remembered per provider and tried
first on that agent's next request, until the tightest window it reported
resets (five hours when the provider named none, capped at 24 hours). The
pin reorders eligible candidates and nothing else: it never adds a
credential, never survives a block, retirement, or a fully used window,
never overrides `billingTarget`, and cannot outlive the process. Two things
depend on it: provider prompt caches live behind one account, and one
conversation's spend belongs in one account's ledger.

## Selectors

- `any` — active agent-owned stateless routes, ranked as above.
- `any-vision-capable` — the `any` contract after filtering to catalog
  models whose input modalities include `image`.
- `task:<task-name>` — the latest active quality observation per active
  model, sorted score-first then newest, plan headroom inside one score.
  Evidence comes only from `brama collect-task-quality`
  ([cli](../cli.md)); Brama never infers task names from prompt text.

## Refusal sentences

Each layer refuses in its own words (all verbatim from
`src/subscription_dispatch/dispatch.rs`; the summary sentences are shared by
the buffered and streaming walks so the two cannot drift):

| Sentence | Meaning |
|---|---|
| `no active '<provider>' credential for agent` | the signed agent owns no eligible credential for this provider |
| `all bounded '<provider>' credentials unavailable for agent` | every bounded credential was tried or blocked |
| `all bounded '<provider>' credentials were rejected by the provider; re-authorization required` | the provider refused each grant; a sign-in is the repair |
| `no <provider> credential could be redeemed for agent` | redemption itself failed — vault, broker, or trust material |
| `no active stateless provider models for signed agent` | a selector found no candidate at all (observed as `429 subscription_unavailable`) |
| `no working subscription model for signed agent` | `any`/`best` walked its whole candidate list |
| `no working vision-capable subscription model for signed agent` | `any-vision-capable` walked its whole list |

At the HTTP edge these surface as `429 subscription_unavailable` (at least
one credential was actually tried, or none exists), `503
subscription_reauthorization_required`, or `503 credential_unauthorized`
([errors](../errors.md)).

## Not to be confused with

- **Authentication.** A valid bearer ([client identity](client-identity.md))
  proves transport, not spend.
- **Ownership.** A subscription's vault item names an owner; the entitlement
  check is what enforces it per request.

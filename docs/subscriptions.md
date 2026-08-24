# Subscriptions and entitlements

A subscription is one provider account delegated to one agent: a Skarbiec
vault item whose credential Brama redeems at final use and whose plan state
Brama reads from the provider's own reports. This page is the entitlement
model — discovery, spending, the usage ledger, credential lifecycle, and the
repair paths.

## Discovery by tags

A provider subscription is a vault item tagged `brama:subscription` and
`brama:agent:<agent>`, carrying its provider and subscription id in
`brama:provider:<provider>` and `brama:id:<id>`. Both the gateway and Brama
Desktop filter on those tags, so vault item ids stay opaque and renaming one
changes nothing. Discovery shells the local entitlements router's `list`
(binary from `ENTITLEMENTS_ROUTER_BIN`, default `entitlements-router`);
successful listings are cached per agent for 60 seconds, and a failed live
lookup may fall back only to the trusted startup catalog
(`BRAMA_SUBSCRIPTION_CATALOG`). An item that loses its `brama:agent:` tag
keeps a working credential and stops existing for every listing — which is
why `/readyz` reports such accounts explicitly as `unroutable_accounts`.

## Who may spend what

Spending requires the caller's own signed HMAC identity
(`x-agent-id`/`x-agent-timestamp`/`x-agent-signature` over the exact raw
body); the bearer-bound agent, signed agent, and path agent must agree where
present. `billingTarget` (`providerId`, `accountId`, `subscriptionId`) names
an exact target: `providerId` must match the route's provider, and
`subscriptionId` must resolve to an active, non-retired resource owned by the
signed agent. The `best` alias and the selectors resolve to
subscription-funded routes; they never authorize a direct provider credential
or another agent's subscription. Credentials are redeemed from
`provider:<provider>:<subscription>` immediately before each provider call.

## Ordering, bounds, and pinning

An explicit subscription route tries at most two eligible credentials;
selectors try at most three model candidates and six provider calls. Both
order candidates by what their plans have left — the maximum used fraction
the provider itself reported across current windows, freest first, read from
the ledger at no provider cost. A window past its own reset instant counts as
empty; a subscription with no reading counts as free, because its first call
writes the reading that corrects the placement. Exact ties are randomized so
equal accounts stay decorrelated.

The credential that served an agent is remembered per provider and tried
first on that agent's next request, until the tightest window it reported
resets (five hours when the provider named none, capped at 24 hours). The pin
reorders eligible candidates and nothing else: it never adds a credential,
never survives a block, retirement, or a full reported window, never
overrides `billingTarget`, and cannot outlive the process. Two things depend
on it: provider prompt caches live behind one account, and one conversation's
spend belongs in one account's ledger.

## The usage ledger

`BRAMA_SUBSCRIPTION_USAGE_FILE` (default
`~/.config/brama/subscription-usage.json`, owner-readable, written
atomically) holds per-subscription state: measured counters, the newest plan
reading per window with its instant and source, any rate-limit block, and the
newest check verdict. It is not a cache — the question it answers spans
months. A ledger from an older gateway still loads, and one that will not
parse yields an empty ledger rather than stopping the gateway.

Plan windows come from each provider's own free usage report: `claude-code`
publishes `GET /api/oauth/usage` on `api.anthropic.com`, `codex`
`GET /backend-api/wham/usage` on `chatgpt.com`, `kimi`
`GET /coding/v1/usages` on `api.kimi.com`; every other provider publishes
nothing, and that absence is recorded as the provider's own answer. Each
subscription's report is read at most once per `BRAMA_PLAN_USAGE_TTL_SECS`
(default 300), jittered by up to a quarter either way and single-flighted per
subscription; the sweep runs every `BRAMA_PLAN_USAGE_SWEEP_SECS` (default 60,
`0` disables) and is logged under `plan_usage_*`. A failed read never blanks
a row: the last good reading is served with `stale: true` and dropped only
past `BRAMA_PLAN_USAGE_RETENTION_SECS` (default 86400).

In listings, `usage_source` names which statement the newest window is —
`provider`, `traffic`, or `probe` — and an empty `limits` array is one of
four states told apart by the newest check: a refused credential
(`probe.ok: false` with the provider's sentence in `probe.detail`), a
subscription nothing has used, a provider that genuinely publishes no plan
state, or windows that aged out. Only the third may be rendered as "no plan
window"; none of them is a zero.

## Credential lifecycle

OAuth subscription credentials (Claude Code, Codex, Kimi) are refreshed ahead
of expiry: every `BRAMA_CREDENTIAL_REFRESH_INTERVAL_SECS` (default 60, `0`
disables) the gateway refreshes every active credential expiring within
`BRAMA_CREDENTIAL_REFRESH_SKEW_SECS` (default 300), single-flighted per
subscription. Refreshing costs no plan quota. A definitive refusal
(`invalid_grant`, `invalid_token`, a revoked refresh token, a non-transport
401/403) is recorded as `credential.state: needs_reauthorization` with the
provider's own sentence as `cause`, and the credential is left alone until a
sign-in replaces it; a transient failure changes nothing and is retried next
sweep. A refreshed grant that cannot be written back to the vault is a failed
refresh — the rotated grant is dropped, because the provider already
invalidated the one still stored. Events are `credential_refresh_*` and
`credential_refreshed_ahead`. API keys have no refresh path and no invented
expiry.

A provider rejecting a grant whose local expiry still claims validity
triggers one forced refresh on the request path; a rate-limited answer
records a block that stands until its own expiry, and a blocked credential is
skipped by ordering and refused by the probe (`409`).

## Donation and retirement

`POST /v1/subscriptions/:agent_id` (and the account/admin variants) banks a
credential onto the coordinate the operator's routes table already names for
that subscription. A donation is refused unless the document reduces to a
bearer — the same reduction the request path applies — so a non-credential
document can never overwrite the one copy of a working account. The plaintext
crosses only the request body and the entitlements-router stdin pipe; Brama's
overlay file (`BRAMA_DONATED_SUBSCRIPTIONS_FILE`, default
`/tmp/brama-skarbiec/donated-subscriptions.json`) holds metadata only.
`DELETE` retires: an append-only journal record that outranks whatever the
last refresh concluded, never a vault deletion. A retired subscription is
never refreshed, because rotating its grant would put back what somebody
removed.

## Reading and repairing the pool

`brama subscriptions list` is the read-only pool report (states `live`,
`expired`, `burnt`, `unknown`, with `expires_at` and `last_redeem_error` in
the words of whatever refused); `brama subscription refresh <provider>
--reason <text>` runs the sweep's refresh for one provider now, appends the
reason to the journal, and exits non-zero unless a credential was obtained.
Both are specified in [cli](cli.md); the HTTP equivalents are
`GET /v1/admin/subscription-pool` and
`POST /v1/admin/subscription-pool/refresh`, and the one quota-spending check
is `POST /v1/admin/subscriptions/:agent_id/:subscription_id/probe`
([http-api](http-api.md)).

# Subscription

One provider account delegated to one agent: a Skarbiec vault item whose
credential Brama redeems at final use and whose plan state Brama reads from
the provider's own reports. Who may spend it is the
[entitlement](entitlement.md) model; this page is the object itself —
discovery, states, the ledger, and the credential lifecycle.

## Shape and discovery

A subscription is a vault item tagged `brama:subscription` and
`brama:agent:<agent>`, carrying its provider in `brama:provider:<provider>`
and its subscription id in `brama:id:<id>`. Both the gateway and Brama
Desktop filter on those tags, so vault item ids stay opaque and renaming one
changes nothing. Its credential lives at the coordinate
`provider:<provider>:<subscription>` (`src/gateway/broker.rs`).

Discovery shells the local entitlements router's `list` (binary from
`ENTITLEMENTS_ROUTER_BIN`, default `entitlements-router`); successful
listings are cached per agent for 60 seconds, a failed shell never poisons
the cache, and a failed live lookup may fall back only to the trusted
startup catalog (`BRAMA_SUBSCRIPTION_CATALOG`).

An item that loses its `brama:agent:` tag keeps a working credential and
stops existing for every listing — the one failure the deployment had no way
to see, which is why `/readyz` reports such accounts explicitly as
`unroutable_accounts` (metadata only, recognised by tags or by the
coordinate an untagged item lives at).

## States

`brama subscriptions list` and `GET /v1/admin/subscription-pool` report one
of four states per row (`src/subscription_dispatch/pool.rs`):

| State | Meaning |
|---|---|
| `live` | credential recorded `active`, expiry absent (API key) or in the future |
| `expired` | credential `active` but its stated expiry has passed |
| `burnt` | the provider disowned the grant (`needs_reauthorization`) or somebody retired it; the pool serves neither, and `last_redeem_error` says which |
| `unknown` | a subscription whose grant nothing has ever looked at — deliberately not the same statement as a working one |

`last_redeem_error` reports the refusal standing in the way, in the words of
whatever refused, with fixed precedence: the credential's own `cause` first
(set only while the grant is refused; only a sign-in clears it), then a
block still in force, then the newest failed check. A lapsed block is
deliberately not reported.

## The usage ledger

`BRAMA_SUBSCRIPTION_USAGE_FILE` (default
`~/.config/brama/subscription-usage.json`, owner-readable, written
atomically) holds per-subscription state: measured counters, the newest plan
reading per window with its instant and source, any rate-limit block, the
credential state, and the newest probe verdict. It is not a cache — the
question it answers spans months. A ledger from an older gateway still
loads, and one that will not parse yields an empty ledger rather than
stopping the gateway.

Plan windows come from each provider's own free usage report
([provider](provider.md) lists the three endpoints); every other provider
publishes nothing, and that absence is recorded as the provider's own
answer. Each subscription's report is read at most once per
`BRAMA_PLAN_USAGE_TTL_SECS` (default 300), jittered ±25% derived from the
subscription id so accounts stay decorrelated across restarts, and
single-flighted per subscription; the sweep runs every
`BRAMA_PLAN_USAGE_SWEEP_SECS` (default 60, `0` disables). A failed read
never blanks a row: the last good reading is served with `stale: true` and
dropped only past `BRAMA_PLAN_USAGE_RETENTION_SECS` (default 86400).

In listings, `usage_source` names which statement the newest window is —
`provider` (the provider's report, no quota), `traffic` (response headers),
or `probe` (the operator's on-demand check) — and an empty `limits` array is
one of four states told apart by the newest check: a refused credential, a
subscription nothing has used, a provider that genuinely publishes no plan
state, or windows that aged out. Only the third may be rendered as "no plan
window"; none of them is a zero.

## Blocks

A rate-limited provider answer records a block: 15 minutes by default,
capped at 7 days when the provider states a longer reset, and 30 minutes for
a credential awaiting re-authorization (`src/subscription_dispatch/usage.rs`).
A blocked credential is skipped by ordering, refused by the probe (`409`),
and the block stands until its own expiry.

## Credential lifecycle

OAuth subscription credentials (`claude-code`, `codex`, `kimi` — token
endpoints in `src/gateway/oauth_refresh.rs`) are refreshed ahead of expiry:
every `BRAMA_CREDENTIAL_REFRESH_INTERVAL_SECS` (default 60, `0` disables)
the gateway refreshes every active credential expiring within
`BRAMA_CREDENTIAL_REFRESH_SKEW_SECS` (default 300), single-flighted per
subscription. Refreshing costs no plan quota. API keys have no refresh path
and no invented expiry.

A refusal is classified with deliberate asymmetry
(`classify_refusal`): **definitive** only on evidence the provider itself
produced — a body containing `invalid_grant`, `invalid_token`, `revoked`, or
`unauthorized_client`, or a 401/403 that named no reason — and everything
else is **transient**, left for the next sweep. A definitive refusal is
recorded as `credential.state: needs_reauthorization` with the provider's
own sentence as `cause`, and the credential is left alone until a sign-in
replaces it. A refreshed grant that cannot be written back to the vault is a
failed refresh — the rotated grant is dropped, because the provider already
invalidated the one still stored. A provider rejecting a grant whose local
expiry still claims validity triggers one forced refresh on the request
path.

## Donation and retirement

`POST /v1/subscriptions/:agent_id` (and the account/admin variants) banks a
credential onto the coordinate the operator's routes table already names for
that subscription. A donation is refused unless the document reduces to a
bearer — the same reduction the request path applies — so a non-credential
document can never overwrite the one copy of a working account. The
plaintext crosses only the request body and the entitlements-router stdin
pipe; the overlay file (`BRAMA_DONATED_SUBSCRIPTIONS_FILE`, default
`/tmp/brama-skarbiec/donated-subscriptions.json`, atomic 0600 rewrite) holds
metadata only.

`DELETE` retires: an append-only journal record (`{"kind":"retire","id",
"at"}` in `BRAMA_STATE_DIR/journal.jsonl`) that outranks whatever the last
refresh concluded, never a vault deletion. A retired subscription is never
refreshed, because rotating its grant would put back what somebody removed.

## Not to be confused with

- **A capability.** The [capability](capability.md) is the deployment's
  direct spend; a subscription is an agent's delegated account.
- **An entitlement.** Owning the item is not the decision about who may
  spend it on which request — that is [entitlement](entitlement.md).

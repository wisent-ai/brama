# What is Brama

What is Brama, and what is the mental model for reading everything else in
these docs? Brama is Wisent's serving-time LLM gateway: one
OpenAI-compatible endpoint that owns client identities, logical model
aliases, provider capabilities, and subscription entitlements, so callers
never hold provider credentials or provider-specific clients. The whole
product is three moving parts — identities and aliases that declare, provider
capabilities and subscriptions that pay, and a bounded dispatch that records
what happened.

## Identities and aliases declare

Every request enters through one Axum ingress that validates transport,
bearer, model allowlist, and request limits before any provider is
considered. A client is a dedicated bearer bound to a `client_id`, an
optional `agent_id`, and an optional exact model allowlist; a bearer the
process does not recognise is resolved against Skarbiec, the authority that
issued it, and desktop user sessions are resolved against Wisent Identity.
The names callers use are deployment-owned: five exact `wisent-backend/*`
aliases whose shapes are a promise (an embeddings alias is never handed a
chat model), operator-invented aliases, the subscription alias `best`, and
canonical `provider/model` routes. Agent callers may also use the selectors
`any`, `any-vision-capable`, and `task:<task-name>`. The declarations are in
[concepts/client-identity](concepts/client-identity.md) and
[concepts/alias](concepts/alias.md).

## Capabilities and subscriptions pay

Brama holds no secret at rest. A direct provider credential is redeemed
through the local Skarbiec capability socket immediately before the HTTP
call, or read from a zeroizing in-memory map in standalone desktop
deployments; a subscription credential belongs to one agent, is discovered
through the entitlements router by vault tags, and is redeemed at the same
final-use boundary. Which account pays is explicit: a direct route spends the
deployment's provider capability, `best` and the selectors spend a
subscription owned by the caller's signed HMAC identity, and `billingTarget`
names an exact subscription. There is no silent fallback across providers,
agents, accounts, or credentials. The two pools are
[concepts/capability](concepts/capability.md) and
[concepts/subscription](concepts/subscription.md); who may spend what is
[concepts/entitlement](concepts/entitlement.md).

## Bounded dispatch records

Every call is finitely bounded before the first byte: one attempt for a
direct route, at most two credentials for an explicit subscription route, at
most three model candidates and six provider attempts for a selector, all
inside a 300-second whole-request deadline. Once a stream commits, nothing is
retried on any credential — a second attempt would double both the bill and
the text. What happened is recorded: structured logs carry routing mode,
selected route, attempts, and outcome in the fleet's `wisent-errors`
envelope; the subscription usage ledger keeps measured tokens and the plan
windows each provider itself reported; the append-only journal keeps
retirement and task-quality records and never credential material. The
contracts are in [errors](errors.md), [concepts/envelope](concepts/envelope.md),
and [concepts/subscription](concepts/subscription.md).

## What Brama is not

Brama routes model requests and nothing else. It does not run agent runtimes
— Jeden is the runtime; Brama performs provider HTTP requests only. It does
not decide where anything runs: Stado owns placement and stages the
owner-only inference route snapshot Brama reloads per request. It is not a
secret store: Skarbiec is the authority for credentials, and Brama receives
only capability handles until the final-use seam. It is not an identity
provider, billing ledger, or system of record for provider accounts, and it
never infers task intent from prompt text — `task:` uses previously recorded,
explicitly named quality evidence only. The boundaries and data flow are
drawn in [architecture](architecture.md).

## The first three commands

```bash
brama detect
```

Reads local hardware and prints a model recommendation. It performs no
provider request, reads no credential, creates no state, and costs nothing —
the safe first command on any machine.

```bash
curl -s http://127.0.0.1:8080/health
```

Liveness only, and its body says so (`dependencies: "not_probed"`): it
answers `ok` from a gateway whose every credential redemption is being
refused.

```bash
curl -s http://127.0.0.1:8080/readyz
```

The one that answers whether the product works. It redeems one capability per
configured provider and one credential per active subscription, and returns
`503` naming what failed, with no secret in the body. The full path from
nothing to a first completion is [quick-start](quick-start.md); the whole
HTTP surface is [http-api](http-api.md); two executed end-to-end scenarios
are [walkthrough-standalone-stub](walkthrough-standalone-stub.md) and
[walkthrough-subscriptions](walkthrough-subscriptions.md); and when
something looks wrong, start from the [runbook](runbook.md).

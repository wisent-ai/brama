# Testing and qualification contract

Tests defend the observable contracts in [`README.md`](README.md),
[`RELEASE.md`](RELEASE.md), [`ONBOARDING.md`](ONBOARDING.md),
[`CORE.md`](CORE.md), [`INTEGRATIONS.md`](INTEGRATIONS.md), and
[`examples/`](examples/README.md). They do not invent product behavior.

## Consent boundary

No test, test suite, smoke test, dry run, provider acceptance, deployment probe,
or validation command may be created, modified, or executed without explicit
human approval for that exact activity. Documentation and static inspection do
not substitute for execution evidence. Credentialed, billable, destructive, and
production-facing groups require separately controlled approval.

## Required evidence map

| Contract | Required evidence |
|---|---|
| Safe first result | Clean-home process invocation of `brama detect`, observed fields, no network/credential/state side effect |
| Build identity | CLI, MCP, health, provenance, SemVer, source revision, platform, and archive digest agree |
| HTTP ingress | Loopback acceptance; remote cleartext rejection; trusted proxy handling |
| Client auth | Missing, duplicate, malformed, wrong, and correct bearer behavior |
| Agent auth | Exact-body HMAC, timestamp window, bearer-agent/path binding, cross-agent denial |
| Direct route | One final-use redemption and at most one provider attempt |
| Subscription route | Ownership filtering, billing target, two-credential bound, retirement behavior |
| Selectors | Model/credential attempt bounds, modality filter, task ranking, deterministic limits |
| Errors | Stable status/code/retryable envelope for every class in `CORE.md` |
| Provider adapters | Real production client against protocol-compatible loopback services, including malformed/timeout/rate-limit behavior |
| Secret safety | No bearer, HMAC, capability, OAuth, donation, prompt, or provider secret in logs/state/errors/artifacts |
| State | Journal append/read semantics, overlay atomicity, cache TTL, restart behavior |
| Subscription lifecycle | Authorized list/donate/retire plus negative ownership evidence and off-switch |
| Integration outage | Catalog, broker, router, provider, and host dependency failure isolation |
| Examples | Every supported example reaches its documented result and cleanup boundary |
| Release | Immutable publication, provenance, digest, upgrade, rollback, and recovery |

## Execution groups

### Fast local contract suite

Scope: deterministic parsing, validation, HMAC, capability tuple checks, journal
rules, selector ordering, attempt budgets, error classification, protocol
translation, and perf bounds. It owns isolated paths and no network credential.

### Clean onboarding scenario

Start with isolated home/config, no inherited Brama variables, no credentials,
no state, and no background service. Exercise only the documented detection
boundary and prove the visible result plus absence of external/state mutation.

### HTTP loopback component suite

Start the real binary with generated synthetic client policy and a
protocol-compatible local capability/provider boundary. Exercise real HTTP,
transport, bearer, agent HMAC, routes, limits, errors, cancellation, and cleanup.
It must not replace the production middleware or reqwest adapter with helper
mocks.

### Integration loopback suite

Use production adapters against bounded local servers. Cover auth shape without
real secrets, request/response translation, unsupported capability, timeout,
rate limit, malformed response, OAuth refresh limits, model catalog degradation,
and unrelated core operation while an integration is unavailable.

### Credentialed provider acceptance

Use only when a local protocol boundary cannot prove provider compatibility.
Approval states provider account owner, exact routes, credential scope, model
and token limits, maximum attempts, maximum spend, prompt/data class, cleanup,
and evidence retention. Production credentials and personal data are forbidden.

### Release qualification

Run against one selected source revision and exact candidate bytes. Combine the
approved layers, verify identity and digest, exercise supported upgrade and
rollback, and record omitted evidence. A passing narrow group cannot qualify the
whole product.

## Design rules

Every test names the actor, initial state, action, observable contract, expected
result, side effects, plausible defect, bounded timeout, and deterministic
cleanup. Prefer real filesystem/process/protocol boundaries. Tests do not assert
source text, private call order, incidental formatting, or a mocked behavior as
the product outcome.

Security evidence includes positive and negative authorization at the same
boundary. Retry evidence proves the upper bound, not only eventual success.
Failure reports name scenario, expected contract, observed result, dependency
involvement, evidence location, and cleanup status without secret material.

## Current gap

While aligning Brama with the Wisent product guidelines, no test was created,
modified, or executed because explicit testing consent was not provided. The
existing perf-registry test was not run. Release qualification remains blocked
until a human approves the exact proposed test changes and commands and the
resulting evidence is recorded here or in the release record.

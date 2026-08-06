# Recover from provider or capability unavailability

**Goal:** distinguish bounded provider exhaustion, dependency outage, timeout, and permanent provider failure.

**Status:** written against the `0.1.0` contract and not re-verified against the published `0.2.5`.

**Risk:** provider-facing diagnostics may be billable. Do not reproduce against a provider without explicit owner approval.

**Environment:** authenticated Brama service plus redacted routing telemetry.

**Preconditions:** build identity, request ID, logical model/selector, caller identity, status, error code, retryable flag, and attempts. Credential IDs and prompt/provider bodies are prohibited evidence.

**Inputs:** one naturally observed failed response. Do not synthesize a production failure.

**Artifacts and side effects:** counters/logs; a permanently rejected delegated credential may gain an append-only retirement marker.

## Classify

| HTTP | Code | Retryable | Meaning |
|---|---|---:|---|
| 429 | `subscription_unavailable` or `provider_rate_limited` | true | bounded subscription capacity exhausted |
| 503 | `dependency_unavailable` | true | broker/catalog/provider unavailable before a stable result |
| 504 | `dependency_timeout` | true | Brama request deadline expired |
| 502 | `provider_failure` | false | malformed or permanent provider-side result |

`attempts` is the number of outbound provider calls consumed. `any` and `task:` stop after at most six model candidates; each subscription provider stops after at most two credentials. The whole HTTP request stops at five minutes. Caller retries are outside that budget and must remain bounded.

## Recovery

1. Check `/health` for build identity only and `/stats` for redacted counters/limits; neither probes dependencies.
2. For attempt count zero, repair broker/catalog/capability configuration rather than provider quota.
3. For rate limit or retryable outage, honor provider backoff and perform at most one owner-approved replay after state changes.
4. For timeout, determine whether the provider may still have completed before replaying; never replay an ambiguous mutation/tool call automatically.
5. For permanent auth rejection, allow retirement workflow to remove that credential from future dispatch, then provision or rotate through the owning capability system.
6. For malformed provider data, quarantine that adapter/route and preserve a redacted response classification for maintainers.

## Observable result

Recovery means a later independently authorized request succeeds within its documented limits, or the route remains intentionally disabled with actionable diagnostics. A retry loop is not recovery evidence.

## Cleanup

Retain bounded redacted evidence. Do not edit journals, inject ambient credentials, disable TLS, or remove vault resources manually.

## Next

See [`../../INTEGRATIONS.md`](../../INTEGRATIONS.md) for provider lifecycle ownership.

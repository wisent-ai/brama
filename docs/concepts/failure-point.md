# Failure point

Where in Brama a failure was raised, as one stable dotted name the fleet can
group, alert, and grep on. Every operator [envelope](envelope.md) carries
exactly one, declared in `src/core/failure.rs` — the one place Brama's own
vocabulary becomes the fleet's.

## The nine points

| Failure point | Raised when |
|---|---|
| `brama.dispatch.model-selection` | choosing a model for a request |
| `brama.dispatch.credential-selection` | choosing which credential pays |
| `brama.dispatch.bounded-rotation` | every bounded credential has been tried |
| `brama.dispatch.credential-block` | a rate-limited answer put a credential inside a recorded block |
| `brama.providers.provider-call` | a provider API call answered with a failure |
| `brama.gateway.oauth-refresh` | the provider refused an OAuth refresh |
| `brama.gateway.credential-persist` | the vault would not store a refreshed grant |
| `brama.gateway.credential-redeem` | a capability did not yield a usable credential |
| `brama.core.model-request` | one routed model request, as seen at the HTTP edge |

## Impact statements

Beside the point, each envelope states what the failure costs, from the same
module:

| Impact | Used by |
|---|---|
| `one model request` | dispatch and edge points |
| `one credential refresh` | `oauth-refresh` — the credential stays as the provider left it |
| `the refreshed grant every later request would have reused` | `credential-persist` |
| `this subscription until its block expires` | `credential-block` |

## One observed envelope

`brama test` against a deployment whose agent owns no credential logs, at
the `credential-selection` point (captured from 0.2.38):

```text
WARN brama::subscription_dispatch::dispatch: an upstream is throttling us —
the request or its credentials; retry later
{"failure_point":"brama.dispatch.credential-selection","error_code":"rate_limit",
 "service":"brama","impact":"one model request","severity":"warning",
 "retryable":true,"outage":false,
 "detail":"no active 'openai' credential for agent",
 "context":{"model":"openai/stub-ok"}}
event="dispatch_refused" model=openai/stub-ok
```

The `detail` is the sentence the layer below gave, verbatim
([entitlement](entitlement.md) lists them); the code, severity,
retryability, and outage flags are derived by the `wisent-errors` crate,
never by the call site.

## Invariants

- One point per envelope; the point names a seam, not a symptom.
- A point this module got wrong is kept verbatim and flagged in the
  envelope's own context instead of being dropped — the constructor is
  `Failure::or_fallback`, because this is only ever called from an error
  path, and a report that refuses to be made takes the diagnosis with it.
- Points are log vocabulary only. They never appear in an HTTP error body:
  a new key there would be a wire change ([errors](../errors.md)).

## Not to be confused with

- **The envelope.** The [envelope](envelope.md) is the whole record; the
  failure point is one field of it.
- **The client error code.** `code` in the HTTP body (`invalid_request`,
  `credential_unauthorized`, …) is the client contract and predates the
  fleet vocabulary; the failure point is for operators.

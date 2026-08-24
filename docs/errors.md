# Errors

Brama speaks two error vocabularies to two audiences, and the split is
deliberate. What a client reads is the HTTP error body — a stable `type` and
`code` that predate the fleet vocabulary and do not change. What an operator
reads is the `wisent-errors` envelope, logged beside the client body with the
fleet's code, failure point, and impact. Where the two readings differ, both
are on the line; neither stands in for the other.

## The client contract

Every public error uses this body:

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

`retryable` is stated explicitly, and `attempts` bounds any replay the caller
considers: selector and credential replay never exceeds it. Message text is
diagnostic, not the machine contract.

| HTTP | Code | Meaning | Caller action |
|---:|---|---|---|
| 400 | `invalid_request` | malformed JSON, route, selector, or limit | Correct the request; do not retry unchanged |
| 401 | `unauthenticated` | bearer or HMAC missing/invalid | Repair or rotate the exact identity |
| 403 | `forbidden` | valid identity lacks model/agent/path authority | Correct grant or request; do not substitute identity |
| 404 | `subscription_not_found` | owned lifecycle target does not exist | Refresh inventory or correct ID |
| 409 | `state_conflict` | requested mutation conflicts with current state | Read current state before retry |
| 426 | `secure_transport_required` | neither loopback nor trusted HTTPS peer | Use approved HTTPS ingress |
| 429 | `provider_rate_limited` | bounded provider quota/rate attempts exhausted | Wait or choose an explicit authorized target |
| 429 | `subscription_unavailable` | no usable agent credential in bound attempts, and at least one was actually tried | Repair intended subscription or wait |
| 502 | `provider_failure` | provider returned permanent/malformed failure | Inspect provider classification; retry only if stated |
| 503 | `credential_unauthorized` | redemption was refused, or no capability, read grant, or trust material could produce a credential at all | Repair the authorization chain; waiting does not reach it |
| 503 | `subscription_reauthorization_required` | every bounded credential was refused by the provider | Sign the subscription in again |
| 503 | `dependency_unavailable` | required catalog, broker, vault, or provider unavailable | Restore named dependency |
| 504 | `dependency_timeout` | whole Brama request deadline expired | Inspect dependency; retry only when safe |
| 500 | `internal_error` | Brama failed outside a classified dependency | Operator investigation required |

One boundary is worth naming twice: a refused redemption is
`503 credential_unauthorized` with `retryable: false`, never a
`429 capacity_error`. Waiting does not repair an authorization that does not
match, and classifying it as capacity sends the caller into retries and the
operator into the wrong catalogue.

## The operator envelope

`src/core/failure.rs` is the one place Brama's own vocabulary becomes the
fleet's, using the `wisent-errors` crate for everything derivable and
deciding nothing itself. Every envelope carries service `brama`, one failure
point, an impact, and the reason the layer below gave, verbatim — the field
shape and a captured example are in [concepts/envelope](concepts/envelope.md)
and [concepts/failure-point](concepts/failure-point.md):

| Failure point | Raised when |
|---|---|
| `brama.dispatch.model-selection` | choosing a model for a request |
| `brama.dispatch.credential-selection` | choosing which credential pays |
| `brama.dispatch.bounded-rotation` | every bounded credential has been tried |
| `brama.dispatch.credential-block` | a rate-limited answer put a credential in a block |
| `brama.providers.provider-call` | a provider API call answered with a failure |
| `brama.gateway.oauth-refresh` | the provider refused an OAuth refresh |
| `brama.gateway.credential-persist` | the vault would not store a refreshed grant |
| `brama.gateway.credential-redeem` | a capability yielded no usable credential |
| `brama.core.model-request` | one routed model request, at the HTTP edge |

Derivation is centralised: severity, retryability, and outage always come
from the crate, never from a call site. Where an upstream HTTP status exists,
the fleet code is classified from that status by the crate's catalogue;
otherwise Brama's own kind is translated exactly once:

| Brama kind | Fleet code |
|---|---|
| `provider_authentication`, `unauthenticated`, `credential_unauthorized` | `Auth` |
| `provider_rate_limited`, `subscription_unavailable` | `RateLimit` |
| `dependency_timeout` | `Timeout` |
| `dependency_unavailable` | `InfraDown` |
| `provider_quota_exhausted` | `Config` — a spent quota is not a busy provider; no wait repairs it, and the account it names is ours |
| `invalid_request` | `NotFound` — the route, selector, or evidence asked for does not exist |
| `provider_failure`, `internal_error` | `Unknown` |

Every arm preserves what the client contract already said about retrying:
retryable kinds map to retryable codes and permanent ones do not. The
envelope stays in the log — a new key in the HTTP error body would be a wire
change — and rendering stays local: the HTTP body, the log fields, and the
usage ledger keep the exact shapes their readers already parse.

## Where errors are not allowed

Logs and error bodies never include bearer values, HMAC signatures,
capability IDs, raw credentials, request bodies, prompt text, raw provider
payloads, or donated secrets. `/readyz` reports a refusal in the words of
whatever refused it — provider name and verdict only, no secret in the body
([http-api](http-api.md)). The full state and recovery contract behind each
code is [`CORE.md`](../CORE.md).

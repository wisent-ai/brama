# Envelope

The record Brama logs beside every failure, in the fleet's `wisent-errors`
shape. Brama speaks two error vocabularies to two audiences, and the split
is deliberate: what a client reads is the HTTP error body — a stable `type`
and `code` that predate the fleet vocabulary and do not change — and what an
operator reads is the envelope. Where the two readings differ, both are on
the line; neither stands in for the other.

## Fields

Observed shape (captured from a 0.2.38 `dispatch_refused` log line):

```json
{
  "failure_point": "brama.dispatch.credential-selection",
  "error_code": "rate_limit",
  "service": "brama",
  "impact": "one model request",
  "severity": "warning",
  "retryable": true,
  "outage": false,
  "detail": "no active 'openai' credential for agent",
  "context": {"model": "openai/stub-ok"}
}
```

- `service` is always `brama`.
- `failure_point` — exactly one of the nine seams
  ([failure-point](failure-point.md)).
- `error_code` — the fleet's code, derived as below.
- `impact` — what this failure costs its caller.
- `detail` — the reason the layer below gave, **verbatim**. Nothing is
  paraphrased; the provider's text is data.
- `severity`, `retryable`, `outage` — always derived by the `wisent-errors`
  crate, never decided at a call site.

## How the fleet code is derived

Derivation is centralised in `src/core/failure.rs`. Where an upstream HTTP
status exists, the code is classified from that status by the crate's
catalogue — the catalogue is finer than Brama's kinds, and being finer is
the point. Otherwise Brama's own kind is translated exactly once:

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
retryable kinds map to retryable codes and permanent ones do not. A message
whose kind prefix is not one of Brama's is classified from the contract code
the edge already derived (`code_for_message`), and an unrecognised kind is
`Unknown` rather than a guess.

Brama's provider layer writes the kind as a message prefix — the captured
provider-authentication failure reads
`provider_authentication: Incorrect API key provided: sk-…` — which is how a
sentence carries its own classification across layers.

## Rendering stays local

The envelope travels beside the shapes readers already parse, never inside
them:

- the HTTP error body keeps its exact client contract
  ([errors](../errors.md)) — a new key there would be a wire change;
- log fields keep their names (`event`, `model`, …) with the envelope as one
  structured field;
- the usage ledger keeps its own schema; a definitive refresh refusal lands
  there as `credential.state: needs_reauthorization` with the provider's
  sentence as `cause` ([subscription](subscription.md)).

## Where envelopes are not allowed

Logs and envelopes never include bearer values, HMAC signatures, capability
IDs, raw credentials, request bodies, prompt text, raw provider payloads, or
donated secrets. `/readyz` reports a refusal in the words of whatever
refused it — provider name and verdict only, no secret in the body.

## Not to be confused with

- **The client error body.** That is the wire contract in
  [errors](../errors.md); the envelope is the operator's record.
- **The journal.** `journal.jsonl` records decisions (retirement, refresh
  verdicts, quality checks); the envelope records failures.

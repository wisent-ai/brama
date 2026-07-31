# Diagnose transport and authorization rejection

**Goal:** recover from a pre-provider HTTP rejection without exposing or rotating unrelated secrets.

**Status:** implemented on development source `0.1.0`; release requires `0.2.0`.

**Risk:** read-only negative path. Use only loopback or an approved non-production diagnostic environment; do not intentionally send malformed authentication to production.

**Environment:** known Brama URL and binary/build identity.

**Preconditions:** owner authorization to inspect the caller configuration and redacted logs. No provider capability is needed because these failures occur before final-use redemption.

**Inputs:** observed HTTP status and structured `error` object.

**Artifacts and side effects:** bounded redacted access/decision logs and failure counters only.

## Interpret the stable error shape

```json
{
  "error": {
    "message": "actionable redacted detail",
    "type": "transport_error|authentication_error|authorization_error|request_error",
    "code": "secure_transport_required|unauthenticated|forbidden|invalid_request",
    "retryable": false,
    "attempts": 0
  }
}
```

- `426 secure_transport_required`: use approved HTTPS, or authenticated direct loopback. Never add a proxy IP casually.
- `401 unauthenticated`: repair the exact caller bearer or body-bound HMAC. Check timestamp window and sign the exact bytes sent.
- `403 forbidden`: the bearer is valid but model allowlist, assigned agent, or path identity does not authorize the request.
- `400 invalid_request`: repair schema, required model/messages, limits, supported selector, or unknown fields.

## Recovery order

1. Record build identity, timestamp, caller client ID, HTTP status, error code, and request ID from redacted logs.
2. Confirm the caller uses its dedicated `<service>-model-router/token` item.
3. Confirm optional bearer-agent binding and `x-agent-id` agree exactly.
4. For signed calls, regenerate timestamp/signature over the unchanged raw body.
5. For `403`, change the authoritative allowlist or request the already authorized alias; never borrow another product token.
6. Retry once only after the cause changed.

## Observable result

A repaired request proceeds past authentication. This example does not claim provider success. Provider attempt counters remain zero for pre-provider failures.

## Cleanup

Remove temporary body/signature files, unset process-local values, and retain only redacted correlation evidence. Do not delete capabilities or journal state.

## Next

If authentication succeeds but the provider fails, continue with [`provider-unavailable.md`](provider-unavailable.md).

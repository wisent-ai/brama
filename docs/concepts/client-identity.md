# Client identity

Who may call the gateway, and as whom. A client identity is a dedicated
bearer bound to a `client_id`; it decides which routes a caller may reach and
nothing about which credential pays — spending is decided by the
[entitlement](entitlement.md) model, and a valid bearer never implies agent
authority.

## Shape

Each boot-time identity is one entry in the JSON array
`BRAMA_MODEL_ROUTER_CLIENT_IDENTITIES` (assembled by the launcher from
dedicated Skarbiec token items; see [configuration](../configuration.md)):

| Field | Required | Meaning |
|---|---|---|
| `client_id` | yes | lowercase ASCII letters, digits, and `-` only; unique |
| `token` | yes | the dedicated bearer; no whitespace or control bytes; unique across clients, compared by SHA-256 digest in constant time |
| `agent_id` | no | binds the bearer to one agent; a signed `x-agent-id` that differs from this binding is `403` |
| `allowed_models` | no | exact, wildcard-free model allowlist; its presence also narrows the reachable paths |

Missing, malformed, duplicate, or contradictory entries fail startup. A
bearer that carries `allowed_models` may reach only the six inference and
discovery paths (`/v1/chat/completions`, `/v1/messages`, `/v1/responses`,
`/v1/embeddings`, `/v1/moderations`, `/v1/models`) plus the account routes;
anything else answers `403`.

## Resolution order

The boot table is a warm start, not a precondition — it is a copy taken at
boot, so it cannot expire, cannot be revoked, and cannot contain a client
registered since the process started. A presented bearer resolves in order:

1. Constant-time lookup against the boot table (SHA-256 digests).
2. Skarbiec token introspection through the `BRAMA_SKARBIEC_CONSUMER` grant
   (`WC_SKARBIEC_URL`) — the authority that issued the token.
3. Wisent Identity (`BRAMA_WISENT_AUTH_URL` + `BRAMA_WISENT_AUTH_ANON_KEY`)
   for desktop user sessions, resolved into the `brama-user` identity.

Both authorities fail closed, and every verdict is cached for five seconds
keyed by the bearer's SHA-256 hex — long enough to keep authorities off the
hot path, short enough that revocation stays effective without a restart
(`src/core/server.rs`).

## Special identities

Two `client_id` values are special in code:

- `brama-desktop` — the only identity the `/v1/admin/*` family answers, and
  it must carry no model allowlist. Every other caller gets `403`.
- `brama-user` — the identity Wisent Identity sessions resolve into; the
  only identity the `/v1/account/*` family serves, and the owner of an
  account operation is derived from the verified session, never from a
  caller-supplied identifier.

In managed deployments, startup additionally requires two exact allowlists as
internal consistency checks when the table is present: `wisent-backend` must
hold exactly the five `wisent-backend/*` aliases, and `weles` must hold
exactly `best`.

## Refusals

Observed against a running gateway (0.2.38):

```text
$ curl -s http://127.0.0.1:8080/v1/models        # no bearer
{"error":{"attempts":0,"code":"unauthenticated","message":"unauthorized",
 "retryable":false,"type":"authentication_error"}}       # HTTP 401
```

- Missing or unknown bearer → `401` `unauthorized`. The body is deliberately
  blank about why: an unauthenticated caller learns nothing.
- Valid bearer outside its path scope, or a signed agent that contradicts
  the bearer's `agent_id` binding or the path agent → `403` `forbidden`.
- Non-`brama-desktop` identity on `/v1/admin/*` → `403` `forbidden`.

## Lifecycle

Identities are read once at startup; the gateway does not reload the client
table in place — restart after any identity change
([configuration](../configuration.md)). Bearers not in the table live and
die with their authority: Skarbiec revocation takes effect within the
five-second cache.

## Not to be confused with

- **An agent identity.** The bearer authenticates transport; the HMAC trio
  (`x-agent-id`/`x-agent-timestamp`/`x-agent-signature`) proves an agent and
  is what selects and spends [subscriptions](subscription.md). See
  [entitlement](entitlement.md).
- **An alias.** An [alias](alias.md) is a deployment-owned model name; it
  never unlocks a credential its caller does not own.

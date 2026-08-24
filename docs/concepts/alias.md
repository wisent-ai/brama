# Alias

What the model names callers use actually mean. An alias is a
deployment-owned name that resolves to a canonical `provider/model` route.
Callers ship against aliases so routes, providers, and subscription-backed
selection can change without a client release; an alias never unlocks a
credential its caller does not own.

## The model-name vocabulary

A request's `model` field must be one of exactly four things:

1. an alias the deployment declares (`wisent-backend/chat/primary`, `smol`, …);
2. the delegation alias `best`;
3. a canonical `provider/model` route (`openai/gpt-4o-mini`);
4. a selector (`any`, `any-vision-capable`, `task:<task-name>`) — signed
   agents only, see [entitlement](entitlement.md).

Nothing else is guessed. A bare vendor model name is refused before any
provider is considered:

```text
$ model="gpt-4o" → 400
{"error":{"code":"invalid_request","message":"model must be a canonical
 provider/model route or a supported selector", ...}}
```

## Declared aliases

`BRAMA_MODEL_ALIASES` (assembled by the launcher from the sealed policy
document, extended per request by the routes file) maps alias names to
canonical routes. Seven aliases are required in managed deployments because
callers ship against them:

| Alias | Shape promise |
|---|---|
| `wisent-backend/chat/primary` | chat |
| `wisent-backend/chat/fallback` | chat |
| `wisent-backend/evaluation` | chat |
| `wisent-backend/embeddings` | embeddings only — never handed a chat model |
| `wisent-backend/moderation` | moderation only |
| `weles/agent/primary` | chat |
| `best` | delegation to subscription dispatch |

Any further name is the operator's to invent and is accepted on the
general-purpose chat shape without a gateway release. A route containing `*`
or stray whitespace fails startup. A route naming a provider whose capability
was never issued on this host does not fail startup: the gateway starts,
serves the aliases that are serviceable, and logs
`alias_provider_capability_absent` — "no provider capability was issued for
this route; the alias will not serve" (`src/core/server.rs`). At request time
such an alias resolves to nothing, exactly as an undeclared alias does.

In standalone desktop mode an absent `BRAMA_MODEL_ALIASES` means `{}`; in
managed mode a bare `brama serve` fails closed with the exact sentence in the
[runbook](../runbook.md#brama-serve-refuses-to-start).

## `best` is delegation

`best` (and any alias whose route delegates to `best`) resolves through
subscription dispatch, where the caller's signed HMAC identity selects the
subscription that pays. Brama deliberately holds no direct credential for it.
A destination naming `best` inside the routes file passes through untouched
rather than being resolved as a local deployment
(`src/core/inference_routes.rs`).

## Dynamic routes: the routes file

When `BRAMA_INFERENCE_ROUTES_FILE` is set, the owner-only snapshot it names
is reloaded per request and its routes extend the launch aliases. The file is
a JSON registry:

```json
{
  "deployments": [
    {"name": "qwen3-8b", "adapters": [{"name": "chat"}],
     "endpoint": {"host": "127.0.0.1", "port": 8000}}
  ],
  "routes":    {"wisent-backend/chat/primary": "openai/gpt-4o-mini"},
  "fallbacks": {"wisent-backend/chat/primary": ["anthropic/claude-3-5-haiku-latest"]}
}
```

Guards, each failing closed with its own sentence
(`src/core/inference_routes.rs`):

- `inference routes must be a regular non-symlink file`
- `inference routes must be owned by the Brama user`
- `inference routes must not be accessible by group or other` (mode `0o077`
  bits must be clear)
- a fallback for an alias with no primary: `inference fallback route
  '<alias>' has no primary destination`
- a repeated destination: `inference route '<alias>' repeats destination
  '<destination>'`
- a bare deployment name resolves to `local-openai/<name>` only when its
  endpoint host is loopback or Tailscale IPv4 (`100.64.0.0/10`) and its port
  is nonzero; otherwise `inference deployment '<name>' has no safe local or
  Tailscale endpoint`.

`PUT /v1/admin/routes` (`{"alias", "primary", "fallbacks"}`) persists
operator route changes into the same file atomically (staging file, `0600`,
validate, rename). Without a configured routes file the update is refused:

```text
409 {"error":{"code":"state_conflict",
     "message":"runtime route registry is not configured", ...}}
```

## Lifecycle

Launch aliases live for the process lifetime; restart after policy changes.
Routes-file aliases change whenever the file does — the snapshot is re-read
per request, so an update is visible to the next request without a restart.

## Not to be confused with

- **A selector.** `any`, `any-vision-capable`, and `task:<name>` are not
  names for one route; they rank candidates per caller identity
  ([entitlement](entitlement.md)).
- **A canonical route.** `provider/model` names one provider directly and is
  paid by the deployment's own [capability](capability.md), never by a
  subscription.

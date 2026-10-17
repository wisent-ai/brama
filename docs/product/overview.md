# Brama overview

Who Brama is for, what it does and does not do, and how a request travels
through it. Back to the [README](../../README.md).

## Problem and intended users

Wisent services need models from several providers without copying provider
credentials into every caller, coupling callers to provider wire formats, or
silently charging the wrong account. Direct provider integrations also duplicate
model discovery, OAuth refresh, retry, error handling, and access policy.

Brama serves four audiences:
- **Desktop users** run a private Brama process on their own computer and add
  their own provider credentials or subscriptions.
- **Wisent service developers** use one OpenAI-compatible API and stable logical
  aliases instead of provider credentials and provider-specific clients.
- **Jeden runtimes** use an agent-bound HMAC identity that authenticates and
  attributes each request they spend from the shared subscription pool.
- **Operators** publish immutable runtimes, grant finite Skarbiec capabilities,
  configure aliases and client allowlists, and diagnose routing without exposing
  secret material.

Brama is preferable to direct integrations when the required outcome is one
least-privilege enforcement point with explicit billing ownership, bounded
provider attempts, normalized errors, and auditable routing decisions.

## Product boundaries

### Included

- OpenAI-compatible chat completions, embeddings, moderations, and model catalog.
- Native Anthropic Messages and OpenAI Responses ingress on the same routing
  decision, so a caller that speaks one of those two first-party formats needs
  no shim in front of Brama.
- Server-sent event streaming on all three chat formats, with every retry
  bounded to the time before the first caller byte.
- Canonical `provider/model` routing and deployment-owned logical aliases,
  including `-best` for the strongest operator-approved subscription route.
- Agent-scoped selectors: `any`, `any-vision-capable`, and `task:<task-name>`.
- Direct provider capabilities owned by Brama and subscription capabilities
  available through the shared vault pool.
- Final-use secret redemption through the local Skarbiec capability socket in
  managed deployments, or a zeroizing in-memory credential map in standalone
  desktop deployments.
- Bounded credential rotation for authentication, quota, and rate-limit failures.
- OAuth refresh for Claude Code, Codex, and Kimi subscription credentials.
- An append-only operational journal for retirement and task-quality evidence.
- Secret-free build identity, health, statistics, hardware detection, and a
  read-only stdio MCP surface.

### Explicit non-goals

- Running Claude Code, Codex, Kimi Code, OpenCode, or any other agent runtime.
  Jeden is the agent runtime; Brama performs provider HTTP requests only.
- Starting or supervising a local inference engine. The deployment owner controls the
  digest-pinned vLLM lifecycle; Brama reads its owner-only route snapshot and performs
  authenticated OpenAI-compatible requests over the target's Tailscale address.
- Acting as a general secret store, identity provider, billing ledger, or system
  of record for provider accounts.
- Inferring task intent from prompt text. `task:` uses previously recorded,
  explicitly named quality evidence only.
- Silent fallback across products, agents, provider accounts, credentials, or
  storage authorities.
- Owning production DNS, ingress, host registration, orchestration, or Skarbiec grants.
- Continuing a cut generation. Once a stream has committed, a provider failure
  ends that stream; Brama never resumes it on another credential, because a
  second attempt would double both the bill and the text.

### Supported environments and current capability

| Capability | Environment | Current state |
|---|---|---|
| `brama detect` and read-only MCP detection | macOS or Linux | Implemented |
| OpenAI-compatible HTTP gateway | Linux service host; authenticated loopback is also supported | Implemented |
| Direct API-provider routing | Provider capability configured in Skarbiec | Implemented |
| Agent subscription routing | Jeden HMAC identity plus delegated capability | Implemented |
| Claude Code donation endpoint | Authorized agent and entitlements router | Implemented |
| Deployment-managed local inference routing | Linux GPU target over Tailscale | Implemented |
| Outbound response streaming | Chat completions, Anthropic Messages, OpenAI Responses | Implemented |
| Immutable public release | Canonical Stado channels for `linux-amd64` and `darwin-arm64` | Implemented; `released-surface.json` names the newest recorded release |
| Per-installation Skarbiec trust material | Operator-managed host | Implemented; `bin/provision-skarbiec-trust` generates it and the launcher refuses to start without it |
| Declared 1.0 contract stability | — | Not yet declared |

The current-state column is authoritative. Unavailable capability must not be
advertised by the API, MCP server, examples, or release notes.

## Core use cases

### Call a deployment-owned model alias

- **Actor:** an authenticated Wisent service.
- **Initial state:** the service has its dedicated bearer and the alias maps to a
  configured direct-provider route, optionally followed by ordered fallback
  routes.
- **Outcome:** Brama validates transport, identity, allowlist, and request limits;
  invokes the primary, then each fallback only after a failed attempt, and
  returns the first normalized success or the final normalized failure.
- **Safety boundary:** no agent subscription is discovered or charged; every
  fallback names an explicit provider capability.

### Use an exact agent subscription

- **Actor:** a Jeden runtime with a bearer bound to its `agent_id`.
- **Initial state:** the agent signs the exact request body and owns an active
  delegated provider capability.
- **Outcome:** Brama redeems the selected credential at final use and returns the
  normalized provider result.
- **Safety boundary:** `billingTarget` must name the same provider and an active
  non-retired subscription in the shared pool; provenance tags do not gate routing.

### Select an available or task-ranked model

- **Actor:** an authenticated Jeden runtime.
- **Initial state:** the agent has active subscriptions; `task:` additionally
  requires persisted quality observations for the named task.
- **Outcome:** Brama chooses eligible candidates, applies the documented bounded
  attempt policy, and stops at the first successful provider result.
- **Cost boundary:** selectors may invoke more than one provider attempt; limits
  are defined in [`CORE.md`](https://brama.wisent.com/docs/core). They never retry without a finite bound.

### Ask for an answer in a declared shape

- **Actor:** any caller of `POST /v1/chat/completions`.
- **Initial state:** the caller names the shape in `response_format`:
  `{"type": "json_schema", "json_schema": {"name": ..., "schema": ..., "strict": true}}`.
- **Outcome:** a provider that decodes under a schema is sent it; every
  answer, from any provider, must be one JSON object or the call answers
  `502` quoting what the model wrote.
- **Refusals:** `400` for another `type` (`text`, `json_object`), a
  `json_schema` without its object, `strict: false`, or `stream: true`,
  since a streamed answer is sent before it can be checked.
- **Proof:** `tests/chat/response_format.sh` asks a running Brama for one
  schema-bound answer and checks the four refusals, keeping every request
  and answer in its report.

### Operate and recover the gateway

- **Actor:** a Brama operator.
- **Initial state:** an immutable runtime and its scoped Skarbiec grants are
  available on the operator-managed Linux host.
- **Outcome:** health and version output identify the build; structured routing
  logs and protected stats explain bounded decisions without secret material.
- **Recovery boundary:** rollback restores one immutable runtime coordinate and
  compatible non-secret journal state as described in [`RELEASE.md`](https://brama.wisent.com/docs/release).

## How Brama works

```text
Wisent service / Jeden
        │ HTTPS or authenticated loopback
        │ dedicated bearer
        │ optional exact-body agent HMAC
        ▼
     Axum ingress ── client binding + model allowlist + request limits
        │
        ├── logical alias ──────────────── direct provider capability
        ├── canonical provider/model ──── direct or agent subscription
        └── any / vision / task ───────── bounded candidate selection
                                                │
                                                ▼
                            entitlements-router metadata discovery
                                                │
                                                ▼
                         Skarbiec final-use capability redemption
                                                │
                                                ▼
                                   provider protocol adapter
                                                │
                                                ▼
                         normalized response, error, metrics, journal
```

Skarbiec is authoritative for secret capability redemption. The entitlements
router is authoritative for live subscription resources. `-best` is an explicit
deployment alias for `codex/gpt-5.3-codex-spark`; a caller still needs both an
allowlisted bearer and a signed HMAC identity, and the subscription comes from
the shared pool.
It does not infer quality from prompt text or unlock a direct provider
credential. Brama's journal stores only retirement markers and task-quality
observations; provider credentials are forbidden. Public model metadata is read
from models.dev (`https://models.dev/api.json`) and is never credential
authority. `BRAMA_MODEL_CATALOG_URL` names a mirror instead (HTTPS, or plain
HTTP to a loopback IP address; no credentials in the URL),
`BRAMA_MODEL_CATALOG_PATH` a file, and `BRAMA_MODEL_CATALOG_CACHE` where the
last fetched copy is kept (Brama's state directory when unset).

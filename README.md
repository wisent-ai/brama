# Brama

Brama gives Wisent services one authenticated, provider-neutral HTTP gateway for
LLM inference while keeping client identity, subscription ownership, provider
credentials, and routing policy under Wisent control.

Canonical repository: [`wisent-ai/model-router`](https://github.com/wisent-ai/model-router).
The product, Rust crate, binary, CLI, MCP server, and service are named `brama`.

## Problem and intended users

Wisent services need models from several providers without copying provider
credentials into every caller, coupling callers to provider wire formats, or
silently charging the wrong account. Direct provider integrations also duplicate
model discovery, OAuth refresh, retry, error handling, and access policy.

Brama serves three audiences:

- **Wisent service developers** use one OpenAI-compatible API and stable logical
  aliases instead of provider credentials and provider-specific clients.
- **Jeden runtimes** use an agent-bound HMAC identity to discover and spend only
  subscriptions delegated to that exact agent.
- **Operators** publish immutable runtimes, grant finite Skarbiec capabilities,
  configure aliases and client allowlists, and diagnose routing without exposing
  secret material.

Brama is preferable to direct integrations when the required outcome is one
least-privilege enforcement point with explicit billing ownership, bounded
provider attempts, normalized errors, and auditable routing decisions.

## Product boundaries

### Included

- OpenAI-compatible chat completions, embeddings, moderations, and model catalog.
- Canonical `provider/model` routing and deployment-owned logical aliases.
- Agent-scoped selectors: `any`, `any-vision-capable`, and `task:<task-name>`.
- Direct provider capabilities owned by Brama and subscription capabilities
  delegated to one agent.
- Final-use secret redemption through the local Skarbiec capability socket.
- Live subscription discovery through `entitlements-router` with a bounded cache.
- Bounded credential rotation for authentication, quota, and rate-limit failures.
- OAuth refresh for Claude Code, Codex, and Kimi subscription credentials.
- An append-only operational journal for retirement and task-quality evidence.
- Secret-free build identity, health, statistics, hardware detection, and a
  read-only stdio MCP surface.

### Explicit non-goals

- Running Claude Code, Codex, Kimi Code, OpenCode, or any other agent runtime.
  Jeden is the agent runtime; Brama performs provider HTTP requests only.
- Starting or supervising a local inference engine. Stado owns the digest-pinned
  vLLM lifecycle; Brama only reads its owner-only route snapshot and performs
  authenticated OpenAI-compatible requests over the target's Tailscale address.
- Acting as a general secret store, identity provider, billing ledger, or system
  of record for provider accounts.
- Inferring task intent from prompt text. `task:` uses previously recorded,
  explicitly named quality evidence only.
- Silent fallback across products, agents, provider accounts, credentials, or
  storage authorities.
- Owning production DNS, ingress, host registration, Stado, or Skarbiec grants.
- Streaming OpenAI responses to callers. The current HTTP contract returns one
  buffered completion.

### Supported environments and current capability

| Capability | Environment | Current state |
|---|---|---|
| `brama detect` and read-only MCP detection | macOS or Linux | Implemented |
| OpenAI-compatible HTTP gateway | Linux service host; authenticated loopback is also supported | Implemented |
| Direct API-provider routing | Provider capability configured in Skarbiec | Implemented |
| Agent subscription routing | Jeden HMAC identity plus delegated capability | Implemented |
| Claude Code donation endpoint | Authorized agent and entitlements router | Implemented |
| Stado-managed local inference routing | Linux GPU target over Tailscale | Implemented |
| Outbound response streaming | — | Not supported |
| Stable public release | — | Not yet published |

The current-state column is authoritative. Unavailable capability must not be
advertised by the API, MCP server, examples, or release notes.

## Core use cases

### Call a deployment-owned model alias

- **Actor:** an authenticated Wisent service.
- **Initial state:** the service has its dedicated bearer and the alias maps to a
  configured direct provider capability.
- **Outcome:** Brama validates transport, identity, allowlist, and request limits;
  invokes the mapped provider once; and returns a normalized response.
- **Safety boundary:** no agent subscription is discovered or charged.

### Use an exact agent subscription

- **Actor:** a Jeden runtime with a bearer bound to its `agent_id`.
- **Initial state:** the agent signs the exact request body and owns an active
  delegated provider capability.
- **Outcome:** Brama redeems the selected credential at final use and returns the
  normalized provider result.
- **Safety boundary:** `billingTarget` must name the same provider and an active
  subscription belonging to the signed agent.

### Select an available or task-ranked model

- **Actor:** an authenticated Jeden runtime.
- **Initial state:** the agent has active subscriptions; `task:` additionally
  requires persisted quality observations for the named task.
- **Outcome:** Brama chooses eligible candidates, applies the documented bounded
  attempt policy, and stops at the first successful provider result.
- **Cost boundary:** selectors may invoke more than one provider attempt; limits
  are defined in [`CORE.md`](CORE.md). They never retry without a finite bound.

### Operate and recover the gateway

- **Actor:** a Brama operator.
- **Initial state:** an immutable runtime and its scoped Stado and Skarbiec grants
  are available on the registered Linux host.
- **Outcome:** health and version output identify the build; structured routing
  logs and protected stats explain bounded decisions without secret material.
- **Recovery boundary:** rollback restores one immutable runtime coordinate and
  compatible non-secret journal state as described in [`RELEASE.md`](RELEASE.md).

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
                              provider protocol adapter + timeout
                                                │
                                                ▼
                         normalized response, error, metrics, journal
```

Skarbiec is authoritative for secret capability redemption. The entitlements
router is authoritative for live subscription resources. Brama's journal stores
only retirement markers and task-quality observations; provider credentials are
forbidden. Public model metadata is read from models.dev and is never credential
authority.

## Quick start

No immutable public release exists yet. Production installation from `main` is
unsupported. This is the safe maintainer pre-release path: it detects local
hardware, performs no provider request, reads no credential, creates no Brama
state, and incurs no model cost.

Prerequisites:

- macOS or Linux;
- Git;
- the Rust toolchain required by `Cargo.lock` (the production build uses the
  pinned builder in `Dockerfile`);
- a source checkout of the private repository.

```bash
git clone https://github.com/wisent-ai/model-router.git brama
cd brama
cargo run --locked -- detect
```

Expected output contains these fields with host-specific values:

```text
GPU Type: ...
VRAM: ... GB
RAM: ... GB
CPU Cores: ...
Recommended model: ...
Recommended backend: ...
```

This command does not start a service. Cargo build output may remain under
`target/`; it is a local build cache, not product state. Continue with
[`ONBOARDING.md`](ONBOARDING.md) for the authenticated loopback and production
operator paths. Runnable, risk-labeled workflows are indexed in
[`examples/`](examples/README.md).

## Primary interfaces

- **HTTP inference:** `POST /v1/chat/completions`, `/v1/embeddings`, and
  `/v1/moderations` are the canonical model-execution interfaces.
- **HTTP discovery:** `GET /v1/models`; bearer-only discovery is public catalog
  scope, while signed discovery includes agent-owned availability.
- **Subscription lifecycle:** `GET`, `POST`, and `DELETE`
  `/v1/subscriptions/:agent_id`; always bearer- and HMAC-protected.
- **Operations:** public `GET /health`; protected `GET /stats`.
- **CLI:** `serve`, `version`, `detect`, `test`, `collect-task-quality`, and
  `mcp`. Billable commands require an explicit cost acknowledgement.
- **MCP:** read-only stdio JSON-RPC exposing `brama_detect` only. Model execution,
  credential discovery, collection, and mutation are deliberately excluded.

The complete state, error, retry, authorization, and resource contract is in
[`CORE.md`](CORE.md). Provider capability and lifecycle contracts are in
[`INTEGRATIONS.md`](INTEGRATIONS.md).

## Operational model

- **Configuration:** production policy is generated by
  `scripts/start-with-skarbiec.sh` from the central Stado service document and
  scoped secret consumers. Missing, malformed, duplicate, or contradictory
  security configuration fails startup.
- **Dynamic inference routes:** `BRAMA_INFERENCE_ROUTES_FILE` points at the
  owner-only snapshot committed by `stado inference route set`. Brama reloads it
  per request, rejects symlinks and group/other-readable files, accepts only
  Tailscale IPv4 deployment endpoints, fails closed on malformed updates, and
  attempts centrally declared fallback routes in order.
- **State:** `$BRAMA_STATE_DIR/journal.jsonl` contains retirement and quality
  records. `/tmp/brama-perf.json` contains replaceable process telemetry. The
  entitlements router owns encrypted provider credential storage.
- **Credentials:** callers use dedicated bearer items. Request-sign identities
  and provider capabilities remain separate. Secrets are redeemed at final use
  and are not written to JSON configuration, logs, or Brama state.
- **Network:** the service binds `0.0.0.0`; only loopback or an explicitly trusted
  HTTPS terminator is accepted. Provider endpoints require approved HTTPS hosts,
  disable redirects, and bypass ambient proxies.
- **Failure:** stable HTTP error codes distinguish invalid input, authentication,
  authorization, quota, timeout, dependency unavailability, and provider
  failure. Retryability is included in the error envelope.
- **Observability:** health and `brama version` expose secret-free build identity;
  structured logs record routing mode, selected route, attempts, outcome, and
  remediation class. `/stats` remains bearer-protected.
- **Upgrade and rollback:** follow [`RELEASE.md`](RELEASE.md). Immutable product
  version, source revision, platform, digest, and provenance are separate facts.
- **Qualification:** evidence groups and consent boundaries are defined in
  [`TESTING.md`](TESTING.md).

## Project status and support

- **Maturity:** pre-1.0. Public contract changes follow the `0.x` policy in
  [`RELEASE.md`](RELEASE.md).
- **Current source version:** `0.1.0`, owned by `Cargo.toml`; the incompatible Unreleased contract requires `0.2.0` before publication.
- **Supported source:** `main` for development only; no stable channel is
  currently published.
- **Issues and operator support:** private
  [`wisent-ai/model-router` issues](https://github.com/wisent-ai/model-router/issues);
  see [`SUPPORT.md`](SUPPORT.md).
- **Security reports:** use the private GitHub Security Advisory channel defined
  in [`SECURITY.md`](SECURITY.md); never put credentials in an issue.
- **License:** proprietary Wisent AI software; see [`LICENSE`](LICENSE).

Rust code defines executable behavior. This README owns the product promise,
boundaries, use cases, terminology, status, and operator entry points. Detailed
documents must link here and must not claim a conflicting capability state.

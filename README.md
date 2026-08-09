# Brama

<!-- wisent-readme-signals:start -->
[![version-check](https://github.com/wisent-ai/brama/actions/workflows/version-check.yml/badge.svg?branch=main)](https://github.com/wisent-ai/brama/actions/workflows/version-check.yml)
<!-- wisent-readme-signals:end -->


Brama gives Wisent services one authenticated, provider-neutral HTTP gateway for
LLM inference while keeping client identity, subscription ownership, provider
credentials, and routing policy under Wisent control.

Canonical repository: [`wisent-ai/brama`](https://github.com/wisent-ai/brama).
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
- Canonical `provider/model` routing and deployment-owned logical aliases,
  including `-best` for the strongest operator-approved subscription route.
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
| Deployment-managed local inference routing | Linux GPU target over Tailscale | Implemented |
| Outbound response streaming | — | Not supported |
| Immutable public release | GitHub Releases for `linux-amd64` and `darwin-arm64` | Implemented; `released-surface.json` names the newest recorded release |
| Per-installation Skarbiec trust material | Operator-managed host | Implemented; `bin/provision-skarbiec-trust` generates it and the launcher refuses to start without it |
| Declared 1.0 contract stability | — | Not yet declared |

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
- **Initial state:** an immutable runtime and its scoped Skarbiec grants are
  available on the operator-managed Linux host.
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
router is authoritative for live subscription resources. `-best` is an explicit
deployment alias for `claude-code/claude-opus-4-6`; a caller still needs both an
allowlisted bearer and the HMAC identity that owns the eligible subscription.
It does not infer quality from prompt text or unlock a direct provider
credential. Brama's journal stores only retirement markers and task-quality
observations; provider credentials are forbidden. Public model metadata is read
from models.dev and is never credential authority.

## Quick start

The normal path is the newest published release. `brama detect` is the safe first
command either way: it reads local hardware, performs no provider request, reads
no credential, creates no Brama state, and incurs no model cost.

Install the archive for your platform and verify its published checksum before
extracting it:

```bash
# List published releases and choose one. There is no `latest` production
# contract, so the version is selected deliberately, never resolved for you.
curl --fail --silent --proto '=https' --tlsv1.2 \
  https://api.github.com/repos/wisent-ai/brama/releases | grep '"tag_name"'

version=<chosen SemVer, without the v prefix>
platform=darwin-arm64   # or linux-amd64
base="https://github.com/wisent-ai/brama/releases/download/v${version}"
curl --fail --location --proto '=https' --tlsv1.2 --remote-name-all \
  "${base}/brama-v${version}-${platform}.tar.gz" \
  "${base}/brama-v${version}-${platform}.tar.gz.sha256"
shasum -a 256 --check "brama-v${version}-${platform}.tar.gz.sha256"
tar -xzf "brama-v${version}-${platform}.tar.gz"
./bin/brama detect
```

Serving traffic takes more than the archive. Provision this installation's trust
material once with `bin/provision-skarbiec-trust` — the archive ships no signing
key and the launcher refuses to start until that material exists — then read
[`ONBOARDING.md`](ONBOARDING.md) before the first authenticated request.

Maintainers working on unreleased source run the same command from a checkout,
which needs Git and the Rust toolchain required by `Cargo.lock` (the production
build uses the pinned builder in `Dockerfile`):

```bash
git clone https://github.com/wisent-ai/brama.git brama
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

Neither command starts a service. The checkout path may leave build output under
`target/`; it is a local build cache, not product state. Continue with
[`ONBOARDING.md`](ONBOARDING.md) for the authenticated loopback and production
operator paths. Runnable, risk-labeled workflows are indexed in
[`examples/`](examples/README.md).

## Primary interfaces

- **HTTP inference:** `POST /v1/chat/completions`, `/v1/embeddings`, and
  `/v1/moderations` are the canonical model-execution interfaces.
- **Account subscriptions:** authenticated Wisent users use
  `GET|POST /v1/account/subscriptions` and
  `DELETE /v1/account/subscriptions/:subscription_id`. Brama derives the
  subscription owner from the verified Wisent session; the caller never
  supplies an account or agent identifier.
- **HTTP discovery:** `GET /v1/models`; bearer-only discovery is public catalog
  scope, while signed discovery includes agent-owned availability.
- **Subscription lifecycle:** `GET`, `POST`, and `DELETE`
  `/v1/subscriptions/:agent_id`; always bearer- and HMAC-protected.
- **Operations:** public `GET /health`; protected `GET /stats`.
- **Desktop control plane:** `brama-desktop` alone may call
  `GET /v1/admin/snapshot`, `PUT /v1/admin/routes`, and the `GET`, `POST`, and
  `DELETE` `/v1/admin/subscriptions/:agent_id` family. These endpoints return
  identifiers and status only; subscription credentials remain write-only.
- **CLI:** `serve`, `version`, `detect`, `test`, `collect-task-quality`, and
  `mcp`. Billable commands require an explicit cost acknowledgement.
- **MCP:** read-only stdio JSON-RPC exposing `brama_detect` only. Model execution,
  credential discovery, collection, and mutation are deliberately excluded.

The complete state, error, retry, authorization, and resource contract is in
[`CORE.md`](CORE.md). Provider capability and lifecycle contracts are in
[`INTEGRATIONS.md`](INTEGRATIONS.md).

## Operational model

- **Configuration:** production policy is generated by
  `scripts/start-with-skarbiec.sh` from operator-owned configuration and scoped
  secret consumers. Missing, malformed, duplicate, or contradictory security
  configuration fails startup.
- **Dynamic inference routes:** `BRAMA_INFERENCE_ROUTES_FILE` points at an
  owner-only snapshot maintained by the deployment operator. Brama reloads it
  per request, rejects symlinks and group/other-readable files, accepts only
  Tailscale IPv4 deployment endpoints, fails closed on malformed updates, and
  attempts centrally declared fallback routes in order.
- **Desktop credential:** Brama Desktop proves its local Ed25519 workload key
  to Skarbiec for the exact
  `acquire:brama-desktop-model-router#token` scope. The one-time acquisition
  bearer is consumed immediately; the Brama model-router bearer remains only
  in process memory and is reacquired after restart.
- **State:** `$BRAMA_STATE_DIR/journal.jsonl` contains retirement and quality
  records. `/tmp/brama-perf.json` contains replaceable process telemetry. The
  entitlements router owns encrypted provider credential storage.
- **Credentials:** callers use dedicated bearer items. Request-sign identities
  and provider capabilities remain separate. Secrets are redeemed at final use
  and are not written to JSON configuration, logs, or Brama state.
- **Network:** desktop account clients use the hosted
  `https://charless-mac-mini.tail6443b3.ts.net` endpoint. Brama remains bound
  to loopback; the placed host's Tailscale Funnel connector terminates HTTPS
  and proxies to the `BRAMA_PORT` listener. Self-hosted clients continue to
  discover logical service addresses through Stado. Provider endpoints require
  approved HTTPS hosts, disable redirects, and bypass ambient proxies.
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
- **Current source version:** the `version` field in `Cargo.toml`, which is the
  single canonical source; this page does not duplicate the number.
- **Supported source:** public `main` for development; immutable releases are
  published on GitHub Releases.
- **Issues and operator support:**
  [`wisent-ai/brama` issues](https://github.com/wisent-ai/brama/issues);
  see [`SUPPORT.md`](SUPPORT.md).
- **Security reports:** use the private GitHub Security Advisory channel defined
  in [`SECURITY.md`](SECURITY.md); never put credentials in an issue.
- **License:** Apache License 2.0; see [`LICENSE`](LICENSE).

Rust code defines executable behavior. This README owns the product promise,
boundaries, use cases, terminology, status, and operator entry points. Detailed
documents must link here and must not claim a conflicting capability state.

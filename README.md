<!-- wisent-banner:start -->
<p align="center">
  <img src="assets/readme-banner.webp" alt="brama by Wisent" width="100%">
</p>
<!-- wisent-banner:end -->

<!-- wisent-readme-signals:start -->
[![Source](https://img.shields.io/badge/GitHub-Source-181717?logo=github)](https://github.com/wisent-ai/brama) [![Issues](https://img.shields.io/badge/GitHub-Issues-181717?logo=github)](https://github.com/wisent-ai/brama/issues) [![Wisent](https://img.shields.io/badge/Wisent-Website-0B0B0B)](https://wisent.com) [![Discord](https://img.shields.io/badge/Discord-Join-5865F2?logo=discord&logoColor=white)](https://discord.gg/qRjpkthq54) [![LinkedIn](https://img.shields.io/badge/LinkedIn-Follow-0A66C2?logo=linkedin&logoColor=white)](https://www.linkedin.com/company/wisent-ai/) [![X](https://img.shields.io/badge/X-Follow-000000?logo=x&logoColor=white)](https://x.com/wisentai) [![Enterprise](https://img.shields.io/badge/Enterprise-Book%20a%20call-0B0B0B?logo=calendly)](https://calendly.com/lbartoszcze)
<!-- wisent-readme-signals:end -->

# Brama: Keep All Your Models Accessible Through One Endpoint

All Models and Providers United in One API.

Brama is the simplicity your stack needs. One API, one identity, uniting every
model you own in one API. When you switch from Claude-Cthulu to GPT-Bazillion or
cancel your subscription, you won’t have to update it everywhere. Define
heuristics such as best, fast, or cheap to call this endpoint with intelligent
routing. And every token is attributed, so you can audit where your subscription
and API money is going. Host it anywhere — even on remote devices.

Canonical repository: [`wisent-ai/brama`](https://github.com/wisent-ai/brama).
The product, Rust crate, binary, CLI, MCP server, and service are named `brama`.

The published documentation at [brama.wisent.com/docs](https://brama.wisent.com/docs)
is the contract: this page owns the product promise, the boundaries, and how to
get the binary running, and it links the page that owns each detail rather than
restating it. Where a sentence here and a documentation page disagree, the
documentation page is the one that was written against the code.

## Problem and intended users

Wisent services need models from several providers without copying provider
credentials into every caller, coupling callers to provider wire formats, or
charging the wrong account. Direct provider integrations also duplicate model
discovery, OAuth refresh, credential rotation, error handling, and access
policy.

Brama serves four audiences:

- **Desktop users** run a private Brama process on their own computer and add
  their own provider credentials or subscriptions.
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

## What an alias promises

An alias is a deployment-owned model name that resolves to exactly one
canonical `provider/model` route. There is no ordered list of spare
destinations behind it: when the route refuses, that refusal is the caller's
answer, carrying the state and the route it was refused for. An operator who
wants a different destination changes the one route.

The route registry is a JSON document with three known top-level fields —
`schema_version`, `deployments`, and `routes`, which maps each alias to its
single destination. An unknown top-level field is refused by name and the whole
document is rejected, so nothing in it serves until it is repaired.
`brama routes migrate` performs that repair in one idempotent operation, and
[`/docs/cli/routes/migrate`](https://brama.wisent.com/docs/cli/routes/migrate)
documents it. The shape, the validation guards, and the atomic owner-only write
are in
[`/docs/configuration/route-registry`](https://brama.wisent.com/docs/configuration/route-registry);
the alias vocabulary and its four states are in
[`/docs/concepts/alias`](https://brama.wisent.com/docs/concepts/alias).

## Product boundaries

### Included

- OpenAI-compatible chat completions, embeddings, moderations, and model catalog.
- Native Anthropic Messages and OpenAI Responses ingress on the same routing
  decision, so a caller that speaks one of those two first-party formats needs
  no shim in front of Brama.
- Server-sent event streaming on all three chat formats, with every credential
  rotation bounded to the time before the first caller byte.
- Canonical `provider/model` routing and deployment-owned logical aliases,
  including `best` for the strongest operator-approved subscription route.
- Agent-scoped selectors: `any`, `any-vision-capable`, and `task:<task-name>`.
- Direct provider capabilities owned by Brama and subscription capabilities
  delegated to one agent.
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
- Starting or supervising a local inference engine. The deployment owner controls
  the digest-pinned vLLM lifecycle; Brama reads its owner-only route snapshot and
  performs authenticated OpenAI-compatible requests over the target's Tailscale
  address.
- Acting as a general secret store, identity provider, billing ledger, or system
  of record for provider accounts.
- Inferring task intent from prompt text. `task:` uses previously recorded,
  explicitly named quality evidence only.
- Quietly substituting another product, agent, provider account, credential, or
  storage authority for the one that was named. The named one either answers or
  its refusal reaches the caller.
- Serving a second route for an alias whose route refused. One alias, one route,
  one answer.
- Owning production DNS, ingress, host registration, orchestration, or Skarbiec
  grants.
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

## Get the binary running

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
[the onboarding journey](https://brama.wisent.com/docs/onboarding) before the
first authenticated request.

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
[onboarding](https://brama.wisent.com/docs/onboarding) for the authenticated
loopback and production operator paths. Runnable, risk-labeled workflows are
indexed in [examples](https://brama.wisent.com/docs/examples).

## Where each contract is documented

| Subject | Page |
|---|---|
| What Brama is, in one read | [`/docs/what-is-brama`](https://brama.wisent.com/docs/what-is-brama) |
| Every HTTP path, method, body and refusal | [`/docs/http-api`](https://brama.wisent.com/docs/http-api) |
| Alias vocabulary, states and diagnosis | [`/docs/concepts/alias`](https://brama.wisent.com/docs/concepts/alias) |
| Which account pays for a request | [`/docs/concepts/entitlement`](https://brama.wisent.com/docs/concepts/entitlement) |
| The route registry document and its guards | [`/docs/configuration/route-registry`](https://brama.wisent.com/docs/configuration/route-registry) |
| Every environment variable the gateway reads | [`/docs/configuration`](https://brama.wisent.com/docs/configuration) |
| The complete command tree | [`/docs/cli`](https://brama.wisent.com/docs/cli) |
| Moving a registry file onto the current shape | [`/docs/cli/routes/migrate`](https://brama.wisent.com/docs/cli/routes/migrate) |
| Reading and repairing the subscription pool | [`/docs/walkthrough-subscriptions`](https://brama.wisent.com/docs/walkthrough-subscriptions) |
| Routing, streaming, state and failure contract | [`/docs/core`](https://brama.wisent.com/docs/core) |
| Error codes and retryability | [`/docs/errors`](https://brama.wisent.com/docs/errors) |
| Symptom-first operator repairs | [`/docs/runbook`](https://brama.wisent.com/docs/runbook) |
| Boundaries Brama depends on but does not own | [`/docs/integrations`](https://brama.wisent.com/docs/integrations) |
| Release, upgrade and rollback | [`/docs/release`](https://brama.wisent.com/docs/release) |
| Qualification evidence and consent boundaries | [`/docs/testing`](https://brama.wisent.com/docs/testing) |
| Security posture and reporting | [`/docs/security`](https://brama.wisent.com/docs/security) |
| What changed, release by release | [`/docs/changelog`](https://brama.wisent.com/docs/changelog) |

## Operating rules that outrank convenience

- Secrets are redeemed at final use and never written to JSON configuration,
  logs, or Brama state. Standalone launchers pass a provider-to-credential JSON
  object to `brama serve --local-credentials-stdin` over standard input.
- Brama binds to loopback. Provider endpoints require approved HTTPS hosts,
  disable redirects, and are reached without an ambient proxy.
- Production policy is generated by `src/release/bin/start-with-skarbiec` from
  operator-owned configuration and scoped secret consumers. Missing, malformed,
  duplicate, or contradictory security configuration fails startup.
- `BRAMA_INFERENCE_ROUTES_FILE` names an owner-only registry snapshot that Brama
  re-reads per request, rejecting symlinks and group- or other-readable files and
  accepting only loopback or Tailscale IPv4 deployment endpoints.
- Every call is finitely bounded before the first byte, and the request deadline
  is the deadline of the attempt being made — it is never multiplied by a number
  of routes.
- Stable HTTP error codes distinguish invalid input, authentication,
  authorization, quota, timeout, dependency unavailability, and provider failure;
  retryability is part of the envelope.
- Health and `brama version` expose secret-free build identity; structured logs
  record routing mode, selected route, attempts, and outcome. `/stats` stays
  bearer-protected.

## Project status and support

- **Maturity:** pre-1.0. Public contract changes follow the `0.x` policy in
  [the release page](https://brama.wisent.com/docs/release).
- **Current source version:** the `version` field in `Cargo.toml`, which is the
  single canonical source; this page does not duplicate the number.
- **Supported source:** public `main` for development; immutable releases are
  built, stored, and promoted through Stado.
- **Issues and operator support:**
  [`wisent-ai/brama` issues](https://github.com/wisent-ai/brama/issues); see
  [support](https://brama.wisent.com/docs/support).
- **Security reports:** use the private GitHub Security Advisory channel defined
  in [security](https://brama.wisent.com/docs/security); never put credentials in
  an issue.
- **License:** Apache License 2.0; see [`LICENSE`](LICENSE).

Rust code defines executable behavior. This page owns the product promise, the
boundaries, and the entry points; each documentation page above owns its own
contract and must not be restated here.

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
- Native Anthropic Messages and OpenAI Responses ingress on the same routing
  decision, so a caller that speaks one of those two first-party formats needs
  no shim in front of Brama.
- Server-sent event streaming on all three chat formats, with every retry
  bounded to the time before the first caller byte.
- Canonical `provider/model` routing and deployment-owned logical aliases,
  including `-best` for the strongest operator-approved subscription route.
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
  subscription belonging to the signed agent.

### Select an available or task-ranked model

- **Actor:** an authenticated Jeden runtime.
- **Initial state:** the agent has active subscriptions; `task:` additionally
  requires persisted quality observations for the named task.
- **Outcome:** Brama chooses eligible candidates, applies the documented bounded
  attempt policy, and stops at the first successful provider result.
- **Cost boundary:** selectors may invoke more than one provider attempt; limits
  are defined in [`CORE.md`](https://brama.wisent.com/docs/core). They never retry without a finite bound.

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
                              provider protocol adapter + timeout
                                                │
                                                ▼
                         normalized response, error, metrics, journal
```

Skarbiec is authoritative for secret capability redemption. The entitlements
router is authoritative for live subscription resources. `-best` is an explicit
deployment alias for `codex/gpt-5.3-codex-spark`; a caller still needs both an
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
[`ONBOARDING.md`](https://brama.wisent.com/docs/onboarding) before the first authenticated request.

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
[`ONBOARDING.md`](https://brama.wisent.com/docs/onboarding) for the authenticated loopback and production
operator paths. Runnable, risk-labeled workflows are indexed in
[`examples/`](https://brama.wisent.com/docs/examples).

## Primary interfaces

- **HTTP inference:** `POST /v1/chat/completions`, `/v1/messages`,
  `/v1/responses`, `/v1/embeddings`, and `/v1/moderations` are the canonical
  model-execution interfaces. The three chat formats share one routing
  decision, one identity contract, and one attempt budget; they differ only in
  the shape of the request and the answer.
- **Streaming:** any of the three chat formats streams when the request asks
  for it -- `"stream": true` on chat completions and Anthropic Messages,
  `"stream": true` on Responses. The response is `text/event-stream` in the
  caller's own dialect: `chat.completion.chunk` frames closed by
  `data: [DONE]`, Anthropic `message_start`/`content_block_*`/`message_stop`
  events, or `response.*` events closed by `response.completed`. A stream that
  ends without its terminal event is a generation that was cut after it
  committed; the caller holds an incomplete answer and Brama has already
  stopped. Rotation across models and credentials happens only before the
  first byte, so a committed stream is never silently re-run.
- **Account API keys and subscriptions:** authenticated Wisent users use
  `GET|POST /v1/account/subscriptions` and
  `DELETE /v1/account/subscriptions/:subscription_id`. Brama derives the owner
  from the verified Wisent session; the caller never supplies an account or
  agent identifier. `POST` accepts an API key for any supported remote provider,
  stores it through Skarbiec without returning it, and makes that account's
  canonical `provider/model` routes available for buffered and streamed calls.
- **HTTP discovery:** `GET /v1/models`; account discovery combines public
  catalog metadata with models executable by that account's stored keys, while
  signed agent discovery includes agent-owned subscriptions.
- **Subscription lifecycle:** `GET`, `POST`, and `DELETE`
  `/v1/subscriptions/:agent_id`; always bearer- and HMAC-protected. A `GET`
  returns, per subscription, the plan windows the provider itself reported
  (`limits`: `used_fraction`, `window_label`, `resets_at_ms`, and the
  `recorded_at_ms` the reading was taken at), what Brama measured (`measured`:
  requests, failures, input and output tokens, first and last use), any
  rate-limit `block` in force, when the record last changed
  (`observed_at_ms`), where the newest window came from and whether it is still
  current (`usage_source`: `provider`, `traffic` or `probe`, and `stale`), the
  newest proactive check (`probe`: `attempted_at_ms`, `ok`, `detail` when there
  is something to explain, and `source` -- `usage_report` or `completion`), and
  where the credential itself stands (`credential`: `state` -- `active`,
  `needs_reauthorization` or `disabled` -- with `cause`, `recorded_at_ms`,
  `expires_at_ms` and `refreshed_at_ms`; `null` while nothing has been recorded
  about the grant). `credential.state` is what separates a subscription that is
  quiet from one whose sign-in is overdue, and `cause` is the provider's own
  sentence for the refusal.
- **How fresh a plan window is:** `usage_source` names which statement the
  newest window is -- the provider's own usage report, the headers of real
  traffic, or an operator's probe -- and is `null` when there is no window to
  attribute. `stale` is true once the newest reading has aged past the freshness
  window (`BRAMA_PLAN_USAGE_TTL_SECS`, default 300). A stale reading is still
  served, because a number that says when it was taken is information and an
  empty plan is not; a reading older than the retention window
  (`BRAMA_PLAN_USAGE_RETENTION_SECS`, default 86400) stops being served, because
  a fraction of a five-hour window that has since reset several times describes
  nothing.
- **What an empty `limits` array means:** one of four states, and the newest
  check is what tells them apart. Windows present: render them, aged by the
  newest `recorded_at_ms`, and say "as of" only when `stale` is false. Empty with
  `probe.ok` false: the credential or the provider refused, and `probe.detail` is
  its own sentence. Empty with no `probe` and nothing measured: nothing has ever
  gone through this subscription. Empty with `probe.ok` true: the provider
  genuinely publishes no plan state, and `probe.detail` says so when the provider
  publishes no usage report at all. Only the last of those may be shown as "no
  plan window", and none of them is a zero.
- **Operations:** public `GET /health` and `GET /readyz`; protected `GET /stats`.
  `/health` is liveness only and says so in its body (`dependencies:
  not_probed`): it answers `ok` from a gateway whose every credential
  redemption is being refused. `/readyz` is the one that answers whether the
  product works: it redeems one capability per configured provider and returns
  `503` naming the providers that failed, with no secret in the body. Deploy
  checks and uptime monitors should read `/readyz`; `/health` only proves the
  process is running.
- **Error contract:** a refused redemption is `503 authorization_error` with
  code `credential_unauthorized` and `retryable: false`, never a `429
  capacity_error`. Waiting does not repair an authorization id that does not
  match, and classifying it as capacity sends the caller into retries and the
  operator into the subscription catalogue.
- **Desktop control plane:** `brama-desktop` alone may call
  `GET /v1/admin/snapshot`, `PUT /v1/admin/routes`, the `GET`, `POST`, and
  `DELETE` `/v1/admin/subscriptions/:agent_id` family, and
  `POST /v1/admin/subscriptions/:agent_id/:subscription_id/probe`, which is the
  only endpoint in the product that deliberately spends plan quota. These
  endpoints return identifiers, usage and status only; subscription credentials
  remain write-only.
- **CLI:** `serve`, `version`, `detect`, `test`, `subscriptions list`,
  `subscription refresh`, `collect-task-quality`, and `mcp`. Billable commands
  require an explicit cost acknowledgement, and commands that mutate state
  require an explicit `--reason`.
- **MCP:** read-only stdio JSON-RPC exposing `brama_detect` only. Model execution,
  credential discovery, collection, and mutation are deliberately excluded.

## Complete administration lifecycles

Every mutable Brama resource has one owner, one full create/read/update/delete
path, and one equivalent Brama Desktop surface. The desktop bearer may call the
administration endpoints; ordinary model clients may not. Credential values are
write-only and never appear in list, snapshot, stats, readiness, or error
responses.

### Route aliases

Read the registry with `GET /v1/admin/snapshot`. Create an alias with:

```http
PUT /v1/admin/routes
Authorization: Bearer <brama-desktop bearer>
Content-Type: application/json

{"alias":"support/chat","primary":"openai/gpt-5.4","fallbacks":["anthropic/claude-sonnet-4-6"]}
```

Send another `PUT` for the same alias to replace its primary and ordered
fallback chain. Delete it with
`DELETE /v1/admin/routes` and `{"alias":"support/chat"}`. Names must use
lowercase ASCII letters, digits, `-`, `_`, `.`, or `/`; every route must be
available and support the alias's request shape; duplicate routes are refused.
The required product aliases cannot be deleted. Brama Desktop exposes the same
create, replace, and delete lifecycle under **Routing**: **Add alias…**, select a
user-owned alias and **Edit this alias…**, or **Delete this alias…**.

### Standalone provider keys

This lifecycle exists only when Brama was started with its standalone
in-memory credential store:

```http
GET /v1/admin/credentials
PUT /v1/admin/credentials
{"provider":"openai","credential":"<new key>"}
DELETE /v1/admin/credentials
{"provider":"openai"}
```

`GET` returns provider names only. The first `PUT` adds the key; another `PUT`
for that provider atomically replaces it; `DELETE` removes it. An empty,
unsupported, local-only, or absent provider is refused without changing the
store. Brama Desktop exposes the same lifecycle under **Subscriptions** →
**Local provider keys**: **Add local key…**, select the provider and **Replace
this provider key…**, or **Remove this provider key…**. The desktop app keeps
the durable copy in macOS Keychain and sends the current set to its private
Brama process over standard input.

### Managed agent subscriptions

List one agent's subscriptions with
`GET /v1/admin/subscriptions/:agent_id`. Add one with:

```http
POST /v1/admin/subscriptions/wisent-app
Authorization: Bearer <brama-desktop bearer>
Content-Type: application/json

{"provider":"openai","label":"primary","api_key":"<credential>"}
```

Brama maintains one deterministic subscription per agent and provider. Repeating
the `POST` for that provider replaces the credential and label in place instead
of creating an unroutable duplicate. A deliberate provider check is
`POST /v1/admin/subscriptions/:agent_id/:subscription_id/probe`; it performs one
minimal real completion and therefore spends provider quota. Retire the
subscription and its credential with
`DELETE /v1/admin/subscriptions/:agent_id/:subscription_id`. Listing, probing,
and deleting never return the credential.

Brama Desktop exposes this lifecycle under **Subscriptions** → **Managed
agent**: **Connect a subscription** adds an agent/provider subscription,
**Replace this subscription credential…** replaces the credential and optional
label, **Verify with provider…** runs the deliberate one-request probe, and
**Retire this subscription…** removes it. Under **My account**, the same
add-or-replace and retire semantics are scoped by the signed-in Wisent identity
through `GET`/`POST /v1/account/subscriptions` and
`DELETE /v1/account/subscriptions/:subscription_id`; an account can never read
or mutate another account's subscriptions.

### Subscription pool

`GET /v1/admin/subscription-pool` and `brama subscriptions list` expose the
same secret-free pool states. Refresh one provider through
`POST /v1/admin/subscription-pool/refresh` with
`{"provider":"codex","reason":"<operator reason>"}`, or through
`brama subscription refresh codex --reason '<operator reason>'`. Repair a
provider-disowned Claude Code, Codex, or Kimi grant with
`brama subscription sign-in <provider> --reason '<operator reason>'`; Brama
calls Weles's real `/reauth` trajectory, confirms the exact account row, then
refreshes the grant. Brama Desktop exposes these operations under
**Subscription Pool**. A non-empty reason is required, the result is appended
to the operational journal, and no credential value is returned.

The complete state, error, retry, authorization, and resource contract is in
[`CORE.md`](https://brama.wisent.com/docs/core). Provider capability and lifecycle contracts are in
[`INTEGRATIONS.md`](https://brama.wisent.com/docs/integrations).

### Functional test journeys

The provider-facing tests run the public Brama binary against the real
Skarbiec vault, real provider accounts, real quota, and Weles sign-ins. They
contain no provider server, canned provider response, fake key, dry run, or
smoke-test substitute. Run them inside the launcher environment, with the
source-tree binary named explicitly so Skarbiec binds capabilities to the
binary Cargo executes:

```console
BRAMA_BIN_OVERRIDE="$PWD/target/debug/brama" \
  scripts/start-with-skarbiec.sh --exec "$HOME/.cargo/bin/cargo" \
  test --test admin_real -- --test-threads=1
BRAMA_BIN_OVERRIDE="$PWD/target/debug/brama" \
  scripts/start-with-skarbiec.sh --exec "$HOME/.cargo/bin/cargo" \
  test --test http_api_real -- --test-threads=1
BRAMA_BIN_OVERRIDE="$PWD/target/debug/brama" \
  scripts/start-with-skarbiec.sh --exec "$HOME/.cargo/bin/cargo" \
  test --test capability_real -- --test-threads=1
BRAMA_BIN_OVERRIDE="$PWD/target/debug/brama" \
  scripts/start-with-skarbiec.sh --exec "$HOME/.cargo/bin/cargo" \
  test --test subscription_real -- --test-threads=1
```

`admin_real` reads the real OpenRouter credential through Brama's configured
Skarbiec route without printing it. It then proves the full alias, standalone
key, and managed-subscription lifecycles: add, read or provider probe, replace,
another real completion, delete, and the final refusal. Its subscription uses
a dedicated qualification agent and removes that credential before the test
returns. The same target sends real buffered and streamed requests through the
OpenAI Chat, Anthropic Messages, and OpenAI Responses surfaces, then reads the
resulting model catalogue, readiness, and statistics. `http_api_real` starts
`brama serve` and requires a real authenticated
completion plus Brama's persisted perf record. `capability_real` requires a
real completion funded by each deployment capability. `subscription_real`
requires a real completion, a provider-side OAuth rotation, and a Weles-driven
sign-in for each subscription provider, checking the usage ledger and journal
after every operation.

An expired grant, invalid key, exhausted provider balance, missing Weles token,
or unavailable account fails the corresponding journey with the provider or
product sentence; none is converted into a pass. Lower-level parser and refusal
contracts remain useful tests, but they are not reported as functional evidence.

## Reading and repairing the subscription pool

Every `best`-aliased call is paid for by a subscription credential, so an empty
pool stops browser automation across the company. It did: both codex grants were
burnt at the same time, every call answered `429 subscription_unavailable`, and
the state that explained it -- `needs_reauthorization` with the provider's own
sentence beside it -- was reachable only by grepping `brama-always-on.err` for
the code and reading timestamps by hand. Two commands report that pool and repair
it.

### `brama subscriptions list`

Read-only. It contacts no provider, redeems no capability and writes nothing: it
joins the deployment's subscription listing to the usage ledger and states what
is already recorded, so it is safe to run against a gateway that is serving
traffic.

```bash
brama subscriptions list --json
```

```json
{
  "providers": [
    {
      "provider": "codex",
      "subscription_id": "brama-sub-wisent-app-codex-primary",
      "state": "burnt",
      "expires_at": null,
      "last_redeem_error": "invalid_grant: refresh token is no longer accepted"
    }
  ]
}
```

`state` is one of four words. `live`: nothing has refused this grant and its
expiry, if it states one at all, is still ahead. `expired`: the recorded expiry
has passed. `burnt`: the provider disowned the grant, or somebody retired the
subscription, and only a sign-in returns it to the pool. `unknown`: nothing has
ever been recorded about this grant, which is not the same statement as a working
one. `expires_at` is the provider's own instant, and `null` for an API key that
states none. `last_redeem_error` is the refusal standing in the way, in the words
of whatever refused it: the credential's own cause first, then a rate-limit block
still in force, then the newest failed check. A lapsed block is deliberately not
reported, because a stale refusal printed beside a `live` grant is what sends an
operator looking for a sign-in nothing needs.

Without `--json` the same report is printed as lines, led by how many credentials
are live -- which is the question that gets the command run.

### `brama subscription refresh <provider> --reason <text>`

Runs the refresh the gateway's own timer runs, for one provider, now. A burnt or
expired grant is never inside the timer's skew window, and a timer that will not
try it is exactly why an empty pool stays empty. `--reason` is required because
this rotates a grant -- the provider invalidates the previous refresh token the
moment it issues a new one -- and the reason is appended to the journal beside
the verdict.

```bash
brama subscription refresh codex --reason 'pool empty; every best call answered 429' --json
```

```json
{
  "provider": "codex",
  "attempted": 2,
  "result": "failed",
  "detail": "refreshed no `codex` grant out of 2 tried; brama-sub-wisent-app-codex-primary: invalid_grant: refresh token is no longer accepted"
}
```

`attempted` counts the subscriptions a refresh was tried for, so `0` means the
command found nothing to do and `detail` says which of three reasons it was: no
usable subscription for that provider in the pool, a provider whose credentials
are API keys and have no refresh path at all, or no usable credential source in
this environment -- the last being what a shell without the launcher's capability
environment gets, and not a broken account. A retired subscription is never
refreshed, because rotating its grant would put back what somebody removed. The
exit status is non-zero unless a credential was obtained.

### `brama subscription sign-in <provider> --reason <text>`

Repairs a provider-disowned `claude-code`, `codex`, or `kimi` grant by running
the provider's real login trajectory through Weles. Before any browser opens,
Brama reads Weles's health contract, resolves exactly one `login_item`, and
refuses an unknown or ambiguous account. Success requires Weles to echo that
exact row and the refresh that follows to answer `refreshed`.

```bash
brama subscription sign-in codex \
  --login-item codex-wisent-app-login \
  --reason 'provider disowned the stored grant' \
  --json
```

Brama and Weles each acquire `brama-weles-reauth/token` from Skarbiec under
their own workload identities when their service starts. Brama receives
`BRAMA_WELES_REAUTH_TOKEN` and presents it only to `POST /reauth`; Weles
receives the same field and accepts it only on that route. At every start the
Brama launcher reads `agent_skarbiec_url` from the host's fleet Stado config
while retaining the dedicated `brama-service` identity; a stale endpoint in an
older service-specific config therefore cannot disconnect Brama from the
canonical vault. `BRAMA_WELES_URL` names the Weles worker API and defaults to
`http://127.0.0.1:8788`. Neither service reads the other's files, the token is
never placed in argv or the journal, and no browser opens on the machine running
the Brama command.

The real functional journeys in `tests/providers/subscription_real.rs` run one
Weles login and one provider refresh for each of Claude Code, Codex, and Kimi.
They pass only when the exact login row is confirmed, the provider returns a
usable grant, the pool leaves `needs_reauthorization`, and Brama records the
`subscription_sign_in` journal entry.

No credential material is printed by either command: the listing reads a ledger
that has never held any, and the refresh drops the credential it obtains without
looking at it.

## Operational model

- **Configuration:** production policy is generated by
  `scripts/start-with-skarbiec.sh` from operator-owned configuration and scoped
  secret consumers. Missing, malformed, duplicate, or contradictory security
  configuration fails startup.
- **Dynamic inference routes:** `BRAMA_INFERENCE_ROUTES_FILE` points at an
  owner-only snapshot maintained by the deployment operator. Brama reloads it
  per request, rejects symlinks and group/other-readable files, accepts only
  loopback or Tailscale IPv4 deployment endpoints, fails closed on malformed
  updates, and attempts centrally declared fallback routes in order.
- **Desktop credentials:** standalone Brama Desktop launches its bundled Brama
  binary on loopback, sends provider credentials once over the child process's
  standard input, and keeps both its router bearer and provider credentials out
  of process arguments, files, logs, and Brama state. A Stado-discovered Brama
  installation may instead acquire its scoped bearer from Skarbiec.
- **State:** `$BRAMA_STATE_DIR/journal.jsonl` contains retirement and quality
  records. `$BRAMA_SUBSCRIPTION_USAGE_FILE`, by default
  `~/.config/brama/subscription-usage.json` and owner-readable only, holds the
  per-subscription usage ledger: measured counters, the newest plan reading per
  window with the instant it was read and where it came from, when the provider's
  usage report was last checked, any block, and the newest check verdict. It is
  written atomically and is not a cache — the
  question it answers spans months, not process lifetimes. A ledger written by
  an older gateway still loads: readings that carry no instant of their own are
  given the ledger file's modification time, and a ledger that will not parse
  yields an empty one rather than stopping the gateway. `/tmp/brama-perf.json`
  contains replaceable process telemetry. The entitlements router owns encrypted
  subscription credential storage in managed deployments.
- **Subscription discovery:** a provider subscription is a Skarbiec item tagged
  `brama:subscription` and `brama:agent:<agent>`, carrying its provider and
  subscription id in `brama:provider:<provider>` and `brama:id:<id>`. Both the
  gateway and Brama Desktop filter on those tags, so vault item ids are opaque
  and renaming one changes nothing. `scripts/provision-desktop-subscriptions.py`
  writes the owner vault and `scripts/provision-host-subscriptions.sh` writes a
  managed host's vault; both are idempotent and derive their tags from
  `scripts/skarbiec-subscriptions.json`.
- **Plan usage from the provider's own report:** every provider that rations a
  subscription publishes a report of how much of the ration is gone, and reading
  it spends no quota at all. `claude-code` publishes
  `GET /api/oauth/usage` on `api.anthropic.com` (`five_hour`, `seven_day`,
  `seven_day_opus` and `seven_day_sonnet`, each a utilization percentage with a
  reset instant), `codex` publishes `GET /backend-api/wham/usage` on
  `chatgpt.com` (`rate_limit.primary_window` and `.secondary_window`, each a
  used percentage with its window length and reset), and `kimi` publishes
  `GET /coding/v1/usages` on `api.kimi.com` (a `usage` object and a `limits`
  array of limit/used/remaining counts with their windows). Every other provider
  publishes nothing, and that absence is recorded as the provider's own answer
  rather than left as an unexplained blank. Each subscription's report is read at
  most once per `BRAMA_PLAN_USAGE_TTL_SECS` (default 300), spread by up to a
  quarter either way from the subscription's own id so a fan-out of accounts on
  one host never becomes one burst against a provider that rate-limits usage
  reads per address, and single-flighted per subscription. The sweep that notices
  aged-out rows runs every `BRAMA_PLAN_USAGE_SWEEP_SECS` (default 60, `0`
  disables it) and is logged under `plan_usage_*`. A failed read never blanks a
  row: the last good reading is kept, served with `stale` true, and dropped only
  once it is older than `BRAMA_PLAN_USAGE_RETENTION_SECS` (default 86400).
- **Routing by what the plans have left:** the readings above are not only for
  reading. A selector orders its candidate routes by the freest usable
  subscription behind each one, and an explicit route orders its bounded
  credentials the same way, both from the ledger and neither costing a provider
  call. Chance still breaks exact ties, so accounts at equal utilization stay
  decorrelated, but an account the provider says is 90 percent spent is no
  longer tried ahead of one at 10 percent. A window whose own reset instant has
  passed counts as empty; a subscription with no reading counts as free,
  because its first call writes the reading that corrects the placement.
- **One agent, one account, for the length of a window:** the credential that
  served an agent is remembered per provider and tried first on that agent's
  next request, until the tightest window it reported resets (five hours when
  the provider named no reset, capped at a day). Two things depend on it: a
  provider's prompt cache lives behind one account, so scattering an agent's
  turns across a pool throws the cache away, and one conversation's spend
  belongs in one account's ledger rather than smeared across every account the
  agent owns. It is a preference and never a grant -- it is consulted after
  eligibility, skipped for a credential inside a block or reporting a full
  window, and it cannot outlive the process, because a pin whose window has
  passed is worth nothing anyway.
- **The one check that costs quota, and only on request:** whether a provider
  will actually serve a credential can only be answered by a real request, so
  `POST /v1/admin/subscriptions/:agent_id/:subscription_id/probe` spends one
  minimal completion against one named subscription and records the verdict as
  `probe` with `source` `completion`. Nothing triggers it on a timer: with default
  configuration no timer performs a quota-consuming request. It is a route rather
  than a subcommand because redeeming the credential needs the capabilities and
  identities the launcher installed in the serving process, and a standalone
  desktop install holds its provider credentials only in that process's memory. A
  subscription inside a recorded block is refused with `409` rather than probed:
  the block already says the account is out of quota, and re-reading that sentence
  is what the block exists to prevent. The probe rotates to no other credential,
  retires nothing, and is logged under its own `usage_probe_*` events so it is
  never mistaken for a caller's request.
- **Refreshing ahead of expiry:** an access token is replaced before it dies
  rather than when a request trips over it. Every
  `BRAMA_CREDENTIAL_REFRESH_INTERVAL_SECS` seconds (default 60, `0` disables it)
  the gateway refreshes every active subscription credential that expires within
  `BRAMA_CREDENTIAL_REFRESH_SKEW_SECS` (default 300), single-flighted per
  subscription so a slow refresh is never started twice. Refreshing costs no plan
  quota: a token endpoint is not a metered endpoint. A refusal is classified. A
  definitive one -- `invalid_grant`, `invalid_token`, a revoked or unauthorized
  refresh token, or a 401/403 that is not a transport failure -- is recorded as
  `credential.state` `needs_reauthorization` with the provider's own sentence as
  `credential.cause`, and that credential is left alone until a sign-in replaces
  it. A transient one -- a timeout, a refused connection, any transport failure --
  changes nothing and is retried by the next sweep. A refreshed grant that cannot
  be written back to the vault is a failed refresh, not a success: the rotated
  grant is dropped rather than spent from memory, because the provider has
  already invalidated the one still in the vault. Events are `credential_refresh_*`
  and `credential_refreshed_ahead`.
- **Credentials:** callers use dedicated bearer items. Request-sign identities
  and provider capabilities remain separate. Secrets are redeemed at final use
  and are not written to JSON configuration, logs, or Brama state. Standalone
  launchers pass a provider-to-credential JSON object to
  `brama serve --local-credentials-stdin` over standard input.
- **Network:** Brama binds to loopback. Standalone desktop clients use their
  bundled process; managed clients may discover a local Brama service through
  Stado. Provider endpoints require approved HTTPS hosts, disable redirects,
  and bypass ambient proxies.
- **Failure:** stable HTTP error codes distinguish invalid input, authentication,
  authorization, quota, timeout, dependency unavailability, and provider
  failure. Retryability is included in the error envelope.
- **Observability:** health and `brama version` expose secret-free build identity;
  structured logs record routing mode, selected route, attempts, outcome, and
  remediation class. `/stats` remains bearer-protected.
- **Upgrade and rollback:** follow [`RELEASE.md`](https://brama.wisent.com/docs/release). Immutable product
  version, source revision, platform, digest, and provenance are separate facts.
- **Qualification:** evidence groups and consent boundaries are defined in
  [`TESTING.md`](https://brama.wisent.com/docs/testing).

## Project status and support

- **Maturity:** pre-1.0. Public contract changes follow the `0.x` policy in
  [`RELEASE.md`](https://brama.wisent.com/docs/release).
- **Current source version:** the `version` field in `Cargo.toml`, which is the
  single canonical source; this page does not duplicate the number.
- **Supported source:** public `main` for development; immutable releases are
  built, stored, and promoted through Stado.
- **Issues and operator support:**
  [`wisent-ai/brama` issues](https://github.com/wisent-ai/brama/issues);
  see [`SUPPORT.md`](https://brama.wisent.com/docs/support).
- **Security reports:** use the private GitHub Security Advisory channel defined
  in [`SECURITY.md`](https://brama.wisent.com/docs/security); never put credentials in an issue.
- **License:** Apache License 2.0; see [`LICENSE`](LICENSE).

Rust code defines executable behavior. This README owns the product promise,
boundaries, use cases, terminology, status, and operator entry points. Detailed
documents must link here and must not claim a conflicting capability state.
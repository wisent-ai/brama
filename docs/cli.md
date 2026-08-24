# CLI

The `brama` binary is the gateway, its diagnostics, and its operator repair
commands in one. Billable commands require an explicit cost acknowledgement,
and commands that mutate state require an explicit `--reason`. Nothing here
prints credential material. Output below was captured from a development
build of 0.2.38; the top-level surface is:

```console
$ brama --help
Multi-provider LLM router

Usage: brama <COMMAND>

Commands:
  version               Print secret-free product and build identity as JSON
  serve                 Start the OpenAI-compatible HTTP server
  onboard               Follow Brama's first-use journey and optionally receive one real model response
  test                  Run a test inference through the router
  detect                Detect local hardware capabilities
  mcp                   Serve the read-only stdio MCP server (agent surface)
  subscriptions         Report the subscription pool this gateway routes over
  subscription          Act on one provider's subscription credentials
  collect-task-quality  Collect deterministic task-quality checks for active provider routes
  help                  Print this message or the help of the given subcommand(s)
```

## `brama version`

Prints secret-free product and build identity as one JSON object; exit 0.

```console
$ brama version
{"product":"brama","version":"0.2.38","source_revision":"development","platform":"development-host","built_at":"not-recorded"}
```

`source_revision`, `platform`, and `built_at` are baked at compile time from
`BRAMA_SOURCE_REVISION`, `BRAMA_BUILD_PLATFORM`, `BRAMA_BUILD_TIMESTAMP`;
a development build reports the placeholders above.

## `brama detect`

Detects local hardware and prints a model recommendation. Performs no
provider request, reads no credential, creates no state, and costs nothing;
exit 0.

```console
$ brama detect
GPU Type: apple_silicon
GPU Name: Apple M2 Max
VRAM: 48.0 GB
RAM: 64.0 GB
CPU Cores: 12
CUDA: false
Metal: true

Recommended model: qwen3-8b
Recommended backend: local
```

## `brama serve`

Starts the OpenAI-compatible HTTP server ([http-api](http-api.md)).

| Flag | Default | Meaning |
|---|---|---|
| `-p, --port <PORT>` | `8080` | port to listen on |
| `--local-credentials-stdin` | off | read a standalone provider-to-credential JSON object (`{"openai": "sk-...", ...}`) from stdin; the input is consumed and zeroized, and the server runs in standalone mode |

In managed deployments, launch through `scripts/start-with-skarbiec.sh`; a
bare start fails closed (exit 1) with the exact sentence in the
[runbook](runbook.md#brama-serve-refuses-to-start). A failed stdin read or
credential install prints `Server error: ...` and exits 1.

## `brama onboard`

Follows the first-use journey and optionally receives one real model
response.

| Flag | Default | Meaning |
|---|---|---|
| `-m, --model <ROUTE>` | `openai/default` | canonical `provider/model` route for the first real response |
| `--agent-id <ID>` | `wisent-app` | workload id whose separately provisioned provider credential is used |
| `--allow-provider-cost` | off | acknowledge one billable provider request |

Without the acknowledgement the journey prints its steps (the routing
contract, the request/response shapes, the completion criterion) and ends
(captured, exit 0):

```text
Next: configure provider/auth separately if needed, then re-run this command
with --allow-provider-cost.
No provider request was sent and onboarding remains in progress.
```

Completion is recorded only after a successful real model response — viewing
the steps or allowing cost is not sufficient. With `--allow-provider-cost`
and no successful response the exit status is 1. Progress persists in
`~/.local/state/brama/onboarding.json`.

## `brama test`

Runs one test inference through the subscription dispatch path (the signed
agent's credentials, not the deployment's direct capability) and prints
model, response, token counts, latency, and cost. Refuses without the
acknowledgement (captured, exit 1):

```console
$ brama test
refusing billable inference without explicit --allow-provider-cost
```

| Flag | Default | Meaning |
|---|---|---|
| `-m, --model <ROUTE>` | `openai/default` | canonical route to test |
| `--agent-id <ID>` | `wisent-app` | agent whose provider credential is used |
| `--allow-provider-cost` | off | acknowledge one billable provider request |

On success (exit 0) it prints `Model:`, `Response:`, `Tokens: <in> in /
<out> out`, `Latency: <n>ms`, `Cost: $<n>`. On refusal it prints the
dispatch sentence and exits 1 (captured against an empty pool):

```text
Error: no active 'openai' credential for agent
```

with the operator envelope logged to stderr beside it
([concepts/failure-point](concepts/failure-point.md)).

## `brama subscriptions list [--json]`

Read-only pool report. It contacts no provider, redeems no capability, and
writes nothing: it joins the deployment's subscription listing to the usage
ledger, so it is safe against a serving gateway. Captured on an empty pool
(exit 0):

```console
$ brama subscriptions list
0 of 0 subscription credentials are live

$ brama subscriptions list --json
{
  "providers": []
}
```

Per subscription it prints `state` (`live`, `expired`, `burnt`, `unknown` —
[concepts/subscription](concepts/subscription.md#states)), `expires_at` (the
provider's own instant, `null` for an API key that states none), and
`last_redeem_error` (the refusal standing in the way, in the words of
whatever refused: credential cause first, then a block still in force, then
the newest failed check; a lapsed block is deliberately not reported).
Without `--json` the same report prints as lines, led by how many
credentials are live — a healthy pool is short, and a broken one is where
the sentences are.

## `brama subscription refresh <provider> --reason <text> [--json]`

Runs the refresh the gateway's own timer runs, for one provider (`codex`,
`claude-code`, `kimi`), now — including grants the timer's skew window would
never try, which is exactly what leaves an empty pool empty. `--reason` is
required because this rotates a grant (the provider invalidates the previous
refresh token on issue), and the reason is appended to the journal beside
the verdict. Captured on an empty pool (exit 1):

```console
$ brama subscription refresh codex --reason "docs walkthrough: exercise the empty-pool refresh path"
provider: codex
attempted: 0
result: failed
detail: no usable `codex` subscription is in this deployment's pool, so no
credential source is configured to refresh: one has to be signed in and
stored in the vault before this command has anything to act on
```

The verdict reports `provider`, `attempted`, `result` (`refreshed` or
`failed`), and `detail` quoting each refusal in the provider's own words.
Exit status is non-zero unless a credential was obtained. A retired
subscription is never refreshed. The journal record it appends is shown in
[walkthrough-subscriptions](walkthrough-subscriptions.md#2-refreshing-an-empty-pool-tells-you-it-is-empty).

## `brama collect-task-quality`

Collects deterministic task-quality checks for active provider routes; the
observations later serve `model="task:<task>"` selection
([concepts/entitlement](concepts/entitlement.md#selectors)).

| Flag | Default | Meaning |
|---|---|---|
| `--agent-id <ID>` | required | agent whose provider credentials are checked |
| `--task <KEY>` | required | task key used later as `task:<task>` |
| `--prompt <TEXT>` | required | prompt sent to each active stateless route |
| `--expected-exact <TEXT>` | unset | exact expected response for score=1 |
| `--expected-contains <TEXT>` | unset | expected substring for score=1 |
| `--persist` | off | write results into the journal (`check` records) |
| `--max-models <N>` | `3` | maximum active models to check; the library bounds it again to 1..25 (`max_models must be between one and 25`) |
| `--allow-provider-cost` | off | acknowledge billable provider requests |

Without the acknowledgement it refuses with `refusing billable task-quality
collection without explicit cost acknowledgement`
(`src/subscription_dispatch/quality.rs`) and exits 1. Each selected model
receives one request through the real agent subscription path; the report is
one JSON document (`ok`, `agentId`, `task`, `persisted`, `maxModels`,
per-model `checks` with status and score, `bestModel`/`bestModels`) and
never expands past `max_models`.

## `brama mcp`

Serves the read-only stdio MCP server (JSON-RPC 2.0, protocol version
`2024-11-05`), exposing `brama_detect` only. Model execution, credential
discovery, collection, and mutation are deliberately excluded from the MCP
surface. Captured session:

```console
$ printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize",...}' \
    '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
    '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"brama_detect","arguments":{}}}' | brama mcp
{"id":1,"jsonrpc":"2.0","result":{"capabilities":{"tools":{}},"protocolVersion":"2024-11-05","serverInfo":{"name":"brama","version":"0.2.38"}}}
{"id":2,"jsonrpc":"2.0","result":{"tools":[{"description":"Detect local compute resources (GPU type/name, VRAM, RAM, CPU cores, CUDA/Metal) and the model + backend brama would recommend for this host. Local only; no network, no credentials, no cost.","inputSchema":{"properties":{},"required":[],"type":"object"},"name":"brama_detect"}]}}
{"id":3,"jsonrpc":"2.0","result":{"content":[{"text":"{\n  \"cpu_cores\": 12,\n  \"gpu_name\": \"Apple M2 Max\", ...}","type":"text"}]}}
```

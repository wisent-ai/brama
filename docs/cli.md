# CLI

The `brama` binary is the gateway, its diagnostics, and its operator repair
commands in one. Billable commands require an explicit cost acknowledgement,
and commands that mutate state require an explicit `--reason`. Nothing here
prints credential material.

## `brama version`

Prints secret-free product and build identity as one JSON object: crate
version, source revision, platform, and build timestamp.

## `brama detect`

Detects local hardware and prints a model recommendation: GPU type and name,
VRAM, RAM, CPU cores, CUDA/Metal availability, recommended model and backend.
Performs no provider request, reads no credential, creates no state, and
costs nothing.

## `brama serve`

Starts the OpenAI-compatible HTTP server.

| Flag | Default | Meaning |
|---|---|---|
| `-p, --port <PORT>` | `8080` | port to listen on |
| `--local-credentials-stdin` | off | read a standalone provider-to-credential JSON object from stdin; the input is consumed and zeroized, and the server runs in standalone mode |

In managed deployments, launch through `scripts/start-with-skarbiec.sh`
rather than directly: the aliases and capability maps are assembled by the
launcher, and a bare start fails closed naming that path
([configuration](configuration.md)).

## `brama onboard`

Follows the first-use journey and optionally receives one real model
response.

| Flag | Default | Meaning |
|---|---|---|
| `-m, --model <ROUTE>` | `openai/default` | canonical `provider/model` route for the first real response |
| `--agent-id <ID>` | `wisent-app` | workload id whose separately provisioned provider credential is used |
| `--allow-provider-cost` | off | acknowledge one billable provider request |

## `brama test`

Runs one test inference through the router and prints model, response, token
counts, latency, and cost. Refuses to run without `--allow-provider-cost`.

| Flag | Default | Meaning |
|---|---|---|
| `-m, --model <ROUTE>` | `openai/default` | canonical route to test |
| `--agent-id <ID>` | `wisent-app` | agent whose provider credential is used |
| `--allow-provider-cost` | off | acknowledge one billable provider request |

## `brama subscriptions list [--json]`

Read-only pool report. It contacts no provider, redeems no capability, and
writes nothing: it joins the deployment's subscription listing to the usage
ledger, so it is safe against a serving gateway. Per subscription it prints
`state` (`live`, `expired`, `burnt`, `unknown`), `expires_at` (the provider's
own instant, `null` for an API key that states none), and
`last_redeem_error` (the refusal standing in the way, in the words of
whatever refused: credential cause first, then a block still in force, then
the newest failed check; a lapsed block is deliberately not reported).
Without `--json` the same report prints as lines, led by how many credentials
are live.

## `brama subscription refresh <provider> --reason <text> [--json]`

Runs the refresh the gateway's own timer runs, for one provider
(`codex`, `claude-code`, `kimi`), now — including grants the timer's skew
window would never try, which is exactly what leaves an empty pool empty.
`--reason` is required because this rotates a grant (the provider invalidates
the previous refresh token on issue), and the reason is appended to the
journal beside the verdict. The verdict reports `attempted`, `result`
(`refreshed` or `failed`), and `detail` quoting each refusal in the
provider's own words. Exit status is non-zero unless a credential was
obtained. A retired subscription is never refreshed.

## `brama collect-task-quality`

Collects deterministic task-quality checks for active provider routes; the
observations later serve `model="task:<task>"` selection.

| Flag | Default | Meaning |
|---|---|---|
| `--agent-id <ID>` | required | agent whose provider credentials are checked |
| `--task <KEY>` | required | task key used later as `task:<task>` |
| `--prompt <TEXT>` | required | prompt sent to each active stateless route |
| `--expected-exact <TEXT>` | unset | exact expected response for score=1 |
| `--expected-contains <TEXT>` | unset | expected substring for score=1 |
| `--persist` | off | write results into the journal |
| `--max-models <N>` | `3` | maximum active models to check (bounded again by the library, 1 through 25) |
| `--allow-provider-cost` | off | acknowledge billable provider requests |

Each selected model receives one request through the real agent subscription
path; the report is per-model status and score, and never expands past
`max_models`.

## `brama mcp`

Serves the read-only stdio MCP server, exposing `brama_detect` only. Model
execution, credential discovery, collection, and mutation are deliberately
excluded from the MCP surface.

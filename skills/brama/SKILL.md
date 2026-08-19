---
name: brama
description: Use Brama, the multi-provider LLM gateway formerly named model-router, through its authenticated HTTP API, Rust CLI, or read-only MCP server. Brama is the gate for Wisent model calls: canonical routing, agent subscriptions, task-quality selection, bounded provider attempts, and local hardware detection. Use MCP only for credential-free hardware detection; model execution and every collect or mutation operation stay in HTTP and CLI.
---

# brama

Brama (Polish for “gate”) is the multi-provider LLM gateway in this ecosystem.
It authenticates a service or signed agent, selects an allowed provider route,
redeems the exact provider capability at final use, applies bounded attempts,
and returns one normalized response. It detects local hardware but does not
start or manage local inference. The canonical repository remains
`wisent-ai/brama`; the product, crate, binary, CLI, MCP server, and
service are `brama`.

## Canonical engine

The source of truth is the `brama` Rust crate (axum HTTP server + provider
router). Do not reimplement its routing, auth, or provider logic.

- `core/` — the router and the HTTP server (`/v1/chat/completions`, `/v1/models`,
  `/health`, `/stats`).
- `detection.rs` — local compute detection and the model recommendation.
- `subscription_dispatch/` — stateless provider selection and task-quality routing.

Build the development source with `cargo build --locked --bin brama`. No stable
release is currently published; `main` is not a production coordinate.

## CLI

```bash
brama version                        # print product and build identity as JSON
brama serve --port <port>            # start the authenticated HTTP gateway
brama test --allow-provider-cost …   # execute one billable inference
brama detect                         # local hardware; no provider or credential
brama subscriptions list [--json]    # pool state per credential; reads nothing but the ledger
brama subscription refresh <provider> --reason <text> # rotate that provider's grants now
brama collect-task-quality --allow-provider-cost … # bounded billable collection
brama mcp                            # read-only stdio MCP server
```

## MCP

Run the stdio server with:

```bash
brama mcp
```

The server writes JSON-RPC frames only to stdout and routes diagnostics to
stderr. It handles the standard `initialize`, `ping`, `tools/list`, and
`tools/call` methods, and pins the MCP protocol version the sibling servers use.
Every MCP tool runs in-process on credential-free, network-free logic. The only
exposed tool is:

- `brama_detect` — local compute resources (GPU type and name, VRAM, RAM, CPU
  cores, CUDA/Metal) plus the model and backend Brama recommends for an external
  local-inference runtime.

The token-spending path — `/v1/chat/completions` and the `test` inference — and
the collecting/persisting `collect-task-quality` command are deliberately not
exposed over MCP. They cost money or change stored evidence, so they stay in the
CLI and HTTP server, off the agent surface.

The HTTP envelope is fail closed: every non-health route requires the exact
bearer assigned to one client identity from that client's dedicated Skarbiec
item. Generic canonical calls may use bearer identity alone. Subscription
providers, selectors, caller-specific model discovery, and mutations also
require body/path-bound agent HMAC headers, which must match the bearer binding.
Bearer-only model discovery exposes only the caller-authorized catalog.
Use HTTPS unless an authenticated loopback caller explicitly opts into HTTP.

## Operational rules

- Keep MCP stdout clean: only JSON-RPC frames go to stdout; every diagnostic and
  the tracing log belong on stderr.
- The MCP surface is read-only and free by construction. To route a real
  completion, call the HTTP server; to refresh stored evidence, use the collect
  commands — never through MCP.
- The deployed service URL is `BRAMA_URL`; each caller uses
  only its own dedicated model-router item and consumer. Product HMAC identities
  remain separate and must match the bearer binding when present.
- brama is the single gate for model traffic; keep provider and routing logic in
  the crate, not in callers.

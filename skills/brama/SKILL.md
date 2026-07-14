---
name: brama
description: Use brama, the multi-provider LLM gateway (formerly model-router), through its Rust CLI or its read-only MCP server. brama is the gate every agent's model calls pass through — an OpenAI-compatible HTTP router with subscription routing, task-quality selection, ranked provider retry, and local-inference detection. Use the agent surface when a task needs to see local hardware and the recommended local model, or the list of routable model ids. The token-spending completions endpoint and every collect/mutate command stay in the HTTP server and CLI, off the agent surface.
---

# brama

brama (Polish for "gate") is the multi-provider LLM gateway in this ecosystem —
the gate every agent's model traffic passes through. It is an OpenAI-compatible
HTTP router: it authenticates a signed agent, picks a provider by subscription
routing or stored task-quality evidence, advances through ranked candidates when
a provider call fails, and can manage local inference. The repository is still
`model-router` on GitHub and the deployed service keeps that name until a
coordinated redeploy; the crate, binary, and directory are `brama`.

## Canonical engine

The source of truth is the `brama` Rust crate (axum HTTP server + provider
router). Do not reimplement its routing, auth, or provider logic.

- `core/` — the router and the HTTP server (`/v1/chat/completions`, `/v1/models`,
  `/health`, `/stats`, subscription-router snapshot).
- `detection.rs` — local compute detection and the model recommendation.
- `subscription_dispatch/` — provider selection, task-quality checks, reauth.

Build with `cargo build --bin brama`.

## CLI

```bash
brama serve --port <port>            # start the OpenAI-compatible HTTP server
brama test --model <model>           # run one inference through the router
brama detect                         # print local hardware + recommended model
brama collect-subscription-checks …  # persist native-CLI subscription checks
brama collect-task-quality …         # persist deterministic task-quality checks
brama mcp                            # the read-only stdio MCP server (below)
```

## MCP

Run the stdio server with:

```bash
brama mcp
```

The server writes JSON-RPC frames only to stdout and routes diagnostics to
stderr. It handles the standard `initialize`, `ping`, `tools/list`, and
`tools/call` methods, and pins the MCP protocol version the sibling servers use.
Every tool runs in-process on the crate's own logic and is credential-free,
network-free, and zero-cost. Exposed tools:

- `brama_detect` — local compute resources (GPU type and name, VRAM, RAM, CPU
  cores, CUDA/Metal) plus the model and backend brama would recommend for this
  host.
- `brama_models` — the model ids brama can route to (the default router's known
  models).

The token-spending path — `/v1/chat/completions` and the `test` inference — and
the collecting/persisting commands (`collect-subscription-checks`,
`collect-task-quality`) are deliberately not exposed over MCP. They cost money or
change stored evidence, so they stay in the CLI and HTTP server, off the agent
surface.

## Operational rules

- Keep MCP stdout clean: only JSON-RPC frames go to stdout; every diagnostic and
  the tracing log belong on stderr.
- The MCP surface is read-only and free by construction. To route a real
  completion, call the HTTP server; to refresh stored evidence, use the collect
  commands — never through MCP.
- The deployed service, its HTTP URL, the `MODEL_ROUTER_*` environment variables,
  the client-secret table, and the request headers still carry the old
  `model-router` name; they change only in a coordinated redeploy, not as part of
  the crate rename.
- brama is the single gate for model traffic; keep provider and routing logic in
  the crate, not in callers.

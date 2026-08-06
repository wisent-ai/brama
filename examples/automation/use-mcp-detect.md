# Detect hardware through stdio MCP

**Goal:** let an MCP host obtain Brama's local resource recommendation without network or credentials.

**Status:** written against the `0.1.0` contract and not re-verified against the published `0.2.5`.

**Risk:** local read-only. The MCP surface exposes only `brama_detect`; it cannot list models, spend tokens, collect evidence, or mutate state.

**Environment:** MCP-capable host that launches the exact Brama binary with argument `mcp`.

**Preconditions:** binary identity inspected with `brama version`; no service configuration is required.

**Inputs:** JSON-RPC initialize, tools/list, and tools/call frames. `brama_detect` accepts an empty object.

**Artifacts and side effects:** none beyond host process lifecycle and local system inspection. stdout is reserved for JSON-RPC; diagnostics use stderr.

## Host configuration

Register a stdio server equivalent to:

```json
{
  "command": "/absolute/path/to/brama",
  "args": ["mcp"]
}
```

Use an absolute, immutable path in a release environment. Start the server through the MCP host, call `tools/list`, then call:

```json
{"name":"brama_detect","arguments":{}}
```

## Observable result

`tools/list` exposes exactly one tool, `brama_detect`. Its result is text containing JSON fields for GPU type/name, VRAM, RAM, CPU cores, CUDA, Metal, recommended model, and recommended backend.

## Failure path

Unknown tools return a JSON-RPC error. Any `brama_models` result indicates a wrong or stale binary because that capability is intentionally absent. Repair the registered executable path; do not grant credentials to MCP.

## Cleanup

Stop the MCP child through the owning host. No Brama state or provider resource exists to remove.

## Next

Use authenticated HTTP, not MCP, for model discovery or execution; see [`../core/call-http-api.md`](../core/call-http-api.md).

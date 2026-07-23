# brama

Multi-provider LLM gateway (formerly `model-router`) with auto-detection, ranked
provider retry, and local inference management. The crate, binary, directory,
CLI, MCP server, and skill are `brama`.

brama is a **pure gateway: it keeps no database**. Per-agent HMAC auth values and
per-subscription provider credentials are read at request time from the skarbiec
vault through the entitlements-router broker (never fetched directly). Non-secret
operational state — subscription retirements, rotated-value overlays, and
task-quality history — lives in a local append-only journal.

## Provider routing contract

OpenAI-compatible `/v1/chat/completions` requests must include a canonical
`provider/model` route, for example `anthropic/claude-sonnet-4-6` or
`openai/gpt-5.4`.

- `model: "any"` chooses among active stateless provider routes for the signed
  agent and rotates credentials on authentication, quota, or rate-limit failure.
- `model: "any-vision-capable"` limits that pool to models whose catalog
  metadata advertises image input.
- `model: "task:<task-name>"` ranks active stateless routes using measured
  task-quality evidence from the local journal.

Jeden is the only agent runtime. Brama performs exactly one provider API call
per attempt; it does not start Claude Code, Codex, Kimi Code, OpenCode, or any
other agent CLI. Provider credentials are discovered live from the Skarbiec
vault (`entitlements-router list`, keeping non-deleted
`provider:<provider>:brama-sub-<slugged-agent>-*` resources) and redeemed from
Skarbiec only at the final-use boundary.

## Secrets and state

brama redeems opaque capabilities over the owner-bound Skarbiec Unix socket.
The canonical client signs `skarbiec.redeem.v1` requests and accepts the secret
only as exact-length raw bytes followed by EOF. Redemption happens immediately
before HMAC verification or provider invocation; no plaintext credential is
placed in JSON, stdout, or an environment file.
- **Operational journal** (`$BRAMA_STATE_DIR`, default `$HOME/.brama`): an
  append-only JSONL file containing only subscription retirement markers and
  task-quality observations. Credential values and encrypted overlays are
  forbidden in runtime state.

Environment:

- `SKARBIEC_CAP_SOCKET`, `SKARBIEC_WORKLOAD_ID`,
  `SKARBIEC_WORKLOAD_SIGNING_KEY_FILE` — canonical workload identity. The key
  file must be a regular owner-owned file with no group/other permissions.
- `ENTITLEMENTS_ROUTER_BIN` — optional path to the broker executable; defaults
  to `entitlements-router`. Brama shells the bare `list` command (cached per
  agent for 60s) and keeps non-deleted `provider:` rows whose resource tail
  starts with the bound `brama-sub-<slugged-agent>-` prefix. On command
  failure it falls back to `BRAMA_SUBSCRIPTION_CATALOG`, then to the legacy
  `list-items` probe; malformed rows yield an empty subscription set.
- `BRAMA_REQUEST_SIGN_CAPABILITY_IDS` — trusted JSON object mapping agent IDs
  to opaque request-sign capability IDs (`agent:<slugged-agent>` bindings).
- `BRAMA_PROVIDER_CAPABILITY_IDS` — trusted JSON object. Direct API providers
  use their exact provider name as the key (`provider:<slugged-provider>`);
  subscription providers use the internal subscription ID
  (`provider:<slugged-provider>:<slugged-subscription>`).
- `BRAMA_STATE_DIR` — journal directory (default `$HOME/.brama`).

All capability IDs are 64-character lowercase hexadecimal opaque handles.
Missing, malformed, mistargeted, or mismatched configuration fails closed.

## Task-quality collection

The `brama` task-quality collector runs each active model against a task prompt
and records each outcome to the journal, which the `task:` selector then ranks
over. Subscription metadata comes from the broker; provider credentials are
redeemed independently at their final-use boundary.


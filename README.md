# brama

Multi-provider LLM gateway (formerly `model-router`) with auto-detection, ranked
provider retry, and local inference management. The crate, binary, directory,
CLI, MCP server, and skill are `brama`.

brama is a **pure gateway: it keeps no database**. Per-agent HMAC auth values and
per-subscription provider credentials are read at request time from the skarbiec
vault through the entitlements-router broker (never fetched directly). Non-secret
operational state — subscription retirements, rotated-value overlays, and
task-quality history — lives in a local append-only journal.

## Subscription routing contract

OpenAI-compatible `/v1/chat/completions` requests must include `model`.

- `model: "codex-subscription"` routes to that explicit subscription provider.
- `model: "any"` chooses a random active supported subscription model for the
  signed agent and keeps trying the randomized list until one succeeds.
- `model: "any-vision-capable"` chooses a random active supported subscription
  model that can read image references through the current router path. Today
  that means Claude Code-backed subscription models, because that engine
  materializes `image_url` inputs for the CLI.
- `model: "task:<task-name>"` routes by measured task-quality evidence recorded
  in the local journal. The router does not infer the task from the prompt and
  uses no hard-coded provider priority. It reads the journal's task-quality
  observations for `<task-name>`, takes the highest measured score, randomizes
  ties, and falls through the ranked candidates if a provider call fails.

An agent's active subscriptions are enumerated from the broker
(`entitlements-router list-items brama-sub-<slugged-agent>-`); each entry's
credential is then resolved on demand.

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
  to `entitlements-router`. Brama invokes `list-items` with the bound
  `brama-sub-<slugged-agent>-` prefix and treats command failures or malformed rows as
  an empty subscription set.
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

## Provider reauth

Subscription-backed providers must not rely only on provider CLI auto-refresh.
When a runtime value fails with an auth error, brama may ask Weles to rotate the
capability-backed vault entry out of band, then retries through a fresh
capability redemption. Plaintext credentials returned by reauth are rejected;
Brama never writes refreshed values or encrypted credential overlays.

Configure the reauth endpoint through env (no database lookup):

- URL: `WELES_BRAMA_REAUTH_URL` / `MODEL_ROUTER_REAUTH_URL` / `WELES_REAUTH_URL`.
- Optional bearer: `WELES_BRAMA_REAUTH_TOKEN` / `WELES_REAUTH_TOKEN` /
  `WELES_API_TOKEN`.
- Optional `WELES_REAUTH_SECRET`, sent as the `x-weles-reauth-secret` header.

# model-router
Multi-provider LLM router with auto-detection, fallback chains, and local inference management

## Subscription routing contract

OpenAI-compatible `/v1/chat/completions` requests must include `model`.

- `model: "codex-subscription"` routes to that explicit subscription provider.
- `model: "any"` chooses a random active supported subscription model for the
  signed agent and keeps trying the randomized list until one succeeds.
- `model: "any-vision-capable"` chooses a random active supported subscription
  model that can read image references through the current router path. Today
  that means Claude Code-backed subscription models, because that engine
  materializes `image_url` inputs for the CLI.
- `model: "task:<task-name>"` routes by stored task-quality evidence. The router
  does not infer the task from the prompt and does not use a hard-coded provider
  priority. It loads `subscription_router_checks` rows where
  `source = "model-router-task-quality"` and `account_identifier = <task-name>`,
  takes the highest measured score, randomizes ties, and falls back through the
  ranked candidates if a provider call fails.

Collect production task-quality evidence through the deployed router:

```bash
set -a
source ../backends/weles-web/.env.local
export SUPABASE_URL="$NEXT_PUBLIC_SUPABASE_URL"
set +a

node scripts/collect-task-quality-via-router-from-weles-config.mjs \
  --task hello-smoke \
  --prompt 'Say Hello in one short sentence.' \
  --expected-contains Hello \
  --persist
```

Then call the task selector:

```json
{
  "model": "task:hello-smoke",
  "messages": [{ "role": "user", "content": "Say hello in one short sentence." }]
}
```

## Weles provider reauth

Subscription-backed providers must not rely only on provider CLI auto-refresh.
When a runtime credential fails with an auth error, model-router calls Weles to
run the provider-specific reauth trajectory and then retries the dispatch once.

Configure one of:

- `WELES_MODEL_ROUTER_REAUTH_URL`
- `MODEL_ROUTER_REAUTH_URL`
- `WELES_REAUTH_URL`
- `WELES_API_URL` / `WELES_BASE_URL` / `WELES_URL` plus the default path
  `/api/model-router/reauth`

Optional auth headers:

- `WELES_MODEL_ROUTER_REAUTH_TOKEN`, `WELES_REAUTH_TOKEN`, or `WELES_API_TOKEN`
  is sent as bearer auth.
- `WELES_REAUTH_SECRET` is sent as `x-weles-reauth-secret`.

The Weles response may either return a fresh credential in `credential`,
`credentials`, `api_key`, `apiKey`, `token`, or `key`, or report that Weles
already updated router state with `refreshed: true` / `updated: true`.

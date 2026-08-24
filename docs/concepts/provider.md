# Provider

A protocol adapter with a trusted endpoint. The provider table says what the
gateway can speak; whether it may spend through a provider is a separate
question answered by a [capability](capability.md) or a
[subscription](subscription.md). A catalog entry never unlocks a credential
or manufactures availability.

## The descriptor table

`src/providers/adapter.rs` declares 23 provider descriptors. Each carries an
id, display name, base URL, models path, chat path, wire protocol, and auth
kind; `/stats` and `/v1/admin/snapshot` report every descriptor with a
`configured` flag.

| id | Display name | Wire protocol | Base URL |
|---|---|---|---|
| `anthropic` | Anthropic | anthropic-messages | `https://api.anthropic.com` |
| `claude-code` | Claude Code (subscription) | anthropic-messages | `https://api.anthropic.com` |
| `kimi` | Kimi (subscription) | openai-chat | `https://api.kimi.com/coding` |
| `openai` | OpenAI | openai-chat | `https://api.openai.com` |
| `codex` | Codex (ChatGPT subscription) | openai-responses | `https://chatgpt.com/backend-api/codex` |
| `openrouter` | OpenRouter | openai-chat | `https://openrouter.ai/api` |
| `groq` | Groq | openai-chat | `https://api.groq.com/openai` |
| `mistral` | Mistral | openai-chat | `https://api.mistral.ai` |
| `xai` | xAI | openai-chat | `https://api.x.ai` |
| `deepseek` | DeepSeek | openai-chat | `https://api.deepseek.com` |
| `cerebras` | Cerebras | openai-chat | `https://api.cerebras.ai` |
| `fireworks` | Fireworks | openai-chat | `https://api.fireworks.ai/inference` |
| `together` | Together | openai-chat | `https://api.together.xyz` |
| `nvidia` | NVIDIA NIM | openai-chat | `https://integrate.api.nvidia.com` |
| `moonshot` | Moonshot | openai-chat | `https://api.moonshot.ai` |
| `zai` | Z.AI | openai-chat | `https://api.z.ai/api/paas` |
| `qwen` | Qwen | openai-chat | `https://dashscope-intl.aliyuncs.com/compatible-mode` |
| `huggingface` | Hugging Face Inference | openai-chat | `https://router.huggingface.co` |
| `featherless` | Featherless | openai-chat | `https://api.featherless.ai` |
| `venice` | Venice | openai-chat | `https://api.venice.ai/api` |
| `novita` | Novita | openai-chat | `https://api.novita.ai/openai` |
| `synthetic` | Synthetic | openai-chat | `https://api.synthetic.new` |
| `local-openai` | Local OpenAI | openai-chat | via routes file |

Four convenience routes resolve to pinned concrete models:
`openai/default` → `gpt-5.4`, `openai/embeddings` →
`text-embedding-3-small`, `openai/moderation` → `omni-moderation-latest`,
`qwen/default` → `qwen-max`.

`local-openai` is the deployment-owned inference target: its base URL
resolves per request through the routes file, and only loopback or Tailscale
IPv4 endpoints are accepted ([alias](alias.md)). Brama does not start or
supervise a local inference engine — the deployment owner controls the
digest-pinned vLLM lifecycle.

## Endpoint trust and overrides

Provider clients require approved HTTPS hosts, disable redirects, and bypass
ambient proxies. `BRAMA_PROVIDER_<PROVIDER>_BASE_URL` (the provider id
uppercased, non-alphanumerics to `_`) overrides one provider's base, and the
override is validated with these exact refusals
(`src/providers/adapter.rs:1066-1117`):

- `provider `<id>` base URL must not contain surrounding whitespace`
- `provider `<id>` has an invalid base URL: <error>`
- `provider `<id>` base URL must be an absolute URL without user info`
- `provider `<id>` base URL must not contain a query or fragment`
- `provider `<id>` base URL has no host`
- `provider `<id>` base URL must use HTTPS` (non-loopback hosts)
- `provider `<id>` host `<host>` is not trusted` (non-loopback host outside
  the provider's trusted-host policy)
- `provider `<id>` loopback endpoint requires an explicit deployment
  override` (loopback is accepted only through the env override, `http` or
  `https`)

The same validated, override-aware base serves both the chat route and the
plan-usage route, so a provider pointed at a proxy does not keep one of its
two endpoints pointed at the open internet.

## Plan-usage publishers

Exactly three providers publish a free plan-usage report; the absence
everywhere else is recorded as the provider's own answer
([subscription](subscription.md)):

| Provider | Usage endpoint |
|---|---|
| `claude-code` | `https://api.anthropic.com/api/oauth/usage` |
| `codex` | `https://chatgpt.com/backend-api/wham/usage` |
| `kimi` | `https://api.kimi.com/coding/v1/usages` |

## Bounds

Per attempt: 255-second provider timeout (applied between reads on a
stream); response bodies capped at 16 MiB before UTF-8 or JSON parsing —
`provider_failure: provider response exceeded byte limit`. Model catalog
downloads 30 seconds; provider model discovery 20 seconds; OAuth refresh 15
seconds with a 64 KiB response cap. The whole-request deadline is 300
seconds (`whole request deadline exceeded`). Public model metadata comes
from models.dev (`BRAMA_MODEL_CATALOG_URL`, default
`https://models.dev/api.json`, cached 900 seconds) and is advisory only.

## Not to be confused with

- **A capability.** The descriptor is speech; the
  [capability](capability.md) is spend.
- **A subscription provider.** `claude-code`, `codex`, and `kimi` are normal
  descriptors whose credentials happen to be agent-owned OAuth grants —
  the [subscription](subscription.md) pool, not the deployment's.

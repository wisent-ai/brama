# Walkthrough: standalone gateway against a stub provider

From nothing to a routed completion — and three refusals — without a
Skarbiec deployment, a real provider credential, or a cent of provider
spend. Everything below was executed against a development build of 0.2.38
on macOS; output is pasted as captured, with only the port (18321) and temp
paths as local choices. The runnable pieces live in
[examples/](examples/README.md).

## 0. The safe first commands

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

$ brama version
{"product":"brama","version":"0.2.38","source_revision":"development","platform":"development-host","built_at":"not-recorded"}
```

Neither command starts a service, reads a credential, or performs a provider
request.

## 1. What a bare start refuses

Managed configuration is generated, not hand-authored, and a bare start says
so and exits 1:

```console
$ brama serve --port 18321
Server error: BRAMA_MODEL_ALIASES is required and is assembled by
scripts/start-with-skarbiec.sh from the sealed policy directory. Starting the
binary directly cannot obtain it: launch the gateway through that script, or
export the variable yourself. Restarting an unlaunched process will not
repair this.
```

## 2. A stub provider on loopback

To exercise the full request path without calling a real provider, run a
local OpenAI-shaped stub ([examples/stub-provider.py](examples/stub-provider.py))
and point the `openai` adapter at it. Loopback is accepted only through the
explicit deployment override ([concepts/provider](concepts/provider.md)):

```bash
python3 docs/examples/stub-provider.py &          # listens on 127.0.0.1:18999

export BRAMA_PROVIDER_OPENAI_BASE_URL=http://127.0.0.1:18999
export BRAMA_MODEL_ROUTER_CLIENT_IDENTITIES='[{"client_id":"docs-test","token":"docs-test-token"}]'
printf %s '{"openai": "sk-brama-docs-invalid"}' | \
  brama serve --port 18321 --local-credentials-stdin
```

The credential is syntactically a bearer and semantically garbage — the stub
never checks it, which is the point: nothing here can spend money.

## 3. Liveness, then readiness

```console
$ curl -s http://127.0.0.1:18321/health
{"build":{...,"version":"0.2.38"},"dependencies":"not_probed","status":"ok"}

$ curl -s http://127.0.0.1:18321/readyz
{"build":{...},"denied":[],"operator_action_required":false,
 "providers":[{"credential":true,"provider":"openai"}],
 "ready":true,
 "reason":"every configured provider credential was obtained, every active
subscription redeemed, and every vault account carries the agent tag that
makes it routable",
 "routing":[],"subscriptions":[],"unredeemable":[],"unroutable":[],
 "unroutable_accounts":[]}
```

`/health` answers `ok` from any live process; `/readyz` actually obtained
the `openai` credential (here: from the standalone in-memory map). The
[architecture](architecture.md#health-versus-readyz) page states the
evidence rule.

## 4. One routed completion

```console
$ curl -s -H "Authorization: Bearer docs-test-token" \
    -H 'Content-Type: application/json' \
    -d '{"model":"openai/stub-ok","messages":[{"role":"user","content":"Say hello in one sentence."}]}' \
    http://127.0.0.1:18321/v1/chat/completions
{"choices":[{"finish_reason":"stop","index":0,"message":{"content":"Hello from
the stub provider.","role":"assistant"}}],
 "id":"chatcmpl-000000000000000018cedce5443df048","model":"openai/stub-ok",
 "object":"chat.completion",
 "usage":{"completion_tokens":7,"prompt_tokens":9,"total_tokens":16}}
```

With `"stream": true` the same request answers `text/event-stream`:

```text
data: {"choices":[{"delta":{"role":"assistant"},"finish_reason":null,"index":0}],...,"object":"chat.completion.chunk"}
data: {"choices":[{"delta":{"content":"Hello "},"finish_reason":null,"index":0}],...}
data: {"choices":[{"delta":{"content":"from "},"finish_reason":null,"index":0}],...}
data: {"choices":[{"delta":{"content":"the stub."},"finish_reason":null,"index":0}],...}
data: {"choices":[{"delta":{},"finish_reason":"stop","index":0}],...}
data: {"choices":[],...,"usage":{"completion_tokens":7,"prompt_tokens":9,"total_tokens":16}}
data: [DONE]
```

The same route answers all three dialects — the Anthropic shape via
`POST /v1/messages` returned `"type":"message"` with
`"stop_reason":"end_turn"`, and `POST /v1/responses` returned
`"object":"response"` with `"status":"completed"`, from the identical stub
call ([http-api](http-api.md)).

## 5. The error paths, deliberately

Request-shape refusals never reach a provider:

```console
$ # missing model
{"error":{"attempts":0,"code":"invalid_request","message":"missing field `model`",...}}   # 400
$ # bare vendor name: model="gpt-4o"
{"error":{...,"message":"model must be a canonical provider/model route or a supported selector",...}}  # 400
$ # unknown field
{"error":{...,"message":"invalid JSON: unknown field `frequency_penalty`, expected one of
 `model`, `messages`, `max_tokens`, `temperature`, `tools`, `tool_choice`,
 `billingTarget`, `stream` at line 1 column 93",...}}  # 400
```

Provider failures are classified, not replayed. The stub returns an
OpenAI-shaped 401 for `stub-401` and a 429 for `stub-429`:

```console
$ # model="openai/stub-401" — the provider refuses the key
{"error":{"attempts":1,"code":"provider_failure",
 "message":"provider_authentication: Incorrect API key provided: sk-brama-docs-invalid.",
 "retryable":false,"type":"provider_error"}}          # HTTP 502

$ # model="openai/stub-429" — the provider throttles
{"error":{"attempts":1,"code":"provider_rate_limited",
 "message":"provider_rate_limited: Rate limit reached for stub-429.",
 "retryable":true,"type":"capacity_error"}}           # HTTP 429
```

Note `attempts: 1` on both: a direct route gets one attempt, and the
provider's own sentence travels verbatim behind the kind prefix
([concepts/envelope](concepts/envelope.md)).

## 6. The transport guard

Rebinding the same gateway to `0.0.0.0` and calling it over the machine's
LAN address — instead of loopback — answers:

```console
$ curl -s http://<lan-ip>:18322/health
{"error":{"attempts":0,"code":"secure_transport_required",
 "message":"HTTPS is required except for direct loopback requests",
 "retryable":false,"type":"transport_error"}}         # HTTP 426
```

A forged `X-Forwarded-Proto: https` from an unlisted peer changes nothing —
forwarded headers are trusted only from `BRAMA_TRUSTED_PROXY_IPS`
([architecture](architecture.md#trust-boundaries)).

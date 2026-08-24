# Runnable examples

Scripts behind the [standalone walkthrough](../walkthrough-standalone-stub.md)
and the [subscription walkthrough](../walkthrough-subscriptions.md). All of
them run against a development build on loopback with fresh temp-directory
state: no Skarbiec deployment, no real credential, no provider spend, no
operator state read or written. Output shown in the walkthroughs was captured
from exactly these scripts.

| Script | What it does |
|---|---|
| [stub-provider.py](stub-provider.py) | OpenAI-shaped provider stub on `127.0.0.1:18999`: `stub-ok` answers a completion (buffered or SSE), `stub-401` refuses the key, `stub-429` throttles |
| [standalone-serve-with-stub.sh](standalone-serve-with-stub.sh) | starts the stub plus a gateway routed at it, with client identity, request-sign secret, and fully isolated state |
| [health-then-ready.sh](health-then-ready.sh) | `GET /health` then `GET /readyz` — liveness, then the readiness probe that actually redeems credentials |
| [signed-agent-listing.sh](signed-agent-listing.sh) | computes the `x-agent-*` HMAC trio with `openssl` and lists one agent's subscriptions |
| [subscription-pool-report.sh](subscription-pool-report.sh) | `brama subscriptions list` (text and `--json`) against isolated state — contacts no provider, writes nothing |

## Run it

Terminal one — build once, then serve (foreground; Ctrl-C stops both
processes):

```console
$ cargo build
$ BRAMA_BIN=target/debug/brama docs/examples/standalone-serve-with-stub.sh
isolated state: /var/folders/.../tmp.WGUxXgcjh6
2026-08-24T22:33:34Z  INFO brama: Starting server on port 18321
```

Terminal two:

```console
$ docs/examples/health-then-ready.sh
== GET http://127.0.0.1:18321/health
{"build":{...},"dependencies":"not_probed","status":"ok"}
== GET http://127.0.0.1:18321/readyz
{...,"providers":[{"credential":true,"provider":"openai"}],"ready":true,...}
HTTP 200

$ docs/examples/signed-agent-listing.sh
{"subscriptions":[]}

$ BRAMA_BIN=target/debug/brama docs/examples/subscription-pool-report.sh
== brama subscriptions list
0 of 0 subscription credentials are live
```

One routed completion and the two classified provider failures
([walkthrough §4–5](../walkthrough-standalone-stub.md#4-one-routed-completion)):

```console
$ curl -s -H "Authorization: Bearer docs-test-token" -H 'Content-Type: application/json' \
    -d '{"model":"openai/stub-ok","messages":[{"role":"user","content":"Say hello in one sentence."}]}' \
    http://127.0.0.1:18321/v1/chat/completions
{"choices":[{"finish_reason":"stop","index":0,"message":{"content":"Hello from the stub provider.","role":"assistant"}}],...}

$ # model "openai/stub-401" → HTTP 502, code provider_failure, retryable false
$ # model "openai/stub-429" → HTTP 429, code provider_rate_limited, retryable true
```

## Conventions

- Every script takes its knobs from `BRAMA_`-prefixed environment variables
  (`BRAMA_BIN`, `BRAMA_PORT`, `BRAMA_URL`, `BRAMA_TOKEN`, `BRAMA_AGENT`,
  `BRAMA_AGENT_SECRET`) with working defaults; nothing is positional.
- `docs-test-token` and `docs-signing-secret` are documentation values wired
  into the isolated gateway the serve script starts — they are not
  credentials and unlock nothing outside it.
- The stub's port (`18999`) is fixed: the serve script points the `openai`
  adapter at it via `BRAMA_PROVIDER_OPENAI_BASE_URL`, the explicit
  per-deployment override that makes a loopback provider endpoint acceptable
  ([concepts/provider](../concepts/provider.md)).

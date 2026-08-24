# Providers and capabilities

A provider is a protocol adapter with a trusted endpoint; a capability is the
authority to obtain that provider's credential at final use. Brama keeps the
two separate on purpose: the adapter table says what the gateway can speak,
the capability configuration says what it may spend.

## Built-in providers

`src/providers/adapter.rs` declares the provider descriptors. The OpenAI
Chat-compatible family is `openai`, `openrouter`, `groq`, `mistral`, `xai`,
`deepseek`, `cerebras`, `fireworks`, `together`, `nvidia`, `moonshot`, `zai`,
`qwen`, `huggingface`, `featherless`, `venice`, `novita`, `synthetic`, and
`kimi`; `anthropic` and `claude-code` speak Anthropic Messages; `codex`
speaks the OpenAI Responses event stream; Google catalog providers speak
GenerateContent; `local-openai` is the deployment-owned loopback/Tailscale
inference target. Requests to `/v1/models` and `/stats` report each
provider's wire protocol and whether a direct capability is configured.

Provider endpoints require approved HTTPS hosts, disable redirects, and
bypass ambient proxies. `BRAMA_PROVIDER_<PROVIDER>_BASE_URL` overrides one
provider's base URL, accepted only for that provider's exact trusted host
over HTTPS or an explicitly configured loopback URL. Public model metadata
comes from models.dev (`BRAMA_MODEL_CATALOG_URL`, default
`https://models.dev/api.json`, cached 900 seconds) and is advisory only — a
catalog entry never unlocks a credential or manufactures availability.

## Direct provider capabilities

A direct credential is read from the vault coordinate
`provider:<slugged-provider>`. The launcher seeds one capability id per
provider in `BRAMA_PROVIDER_CAPABILITY_IDS`; those expire within the hour, so
a refusal is the expected steady state — the request path obtains a fresh
capability with purpose `brama.provider.authenticate` and redeems it through
the local Skarbiec broker socket, immediately before the provider HTTP call.
No lifetime or use count is requested: the broker's defaults are a short life
and a single use, and nothing is cached, because a single-use capability has
nothing worth keeping.

Redemption is the stronger path and goes first, but it is not the only one
the fleet provisions: some providers are granted as a plain per-field read to
a named consumer (`read:provider:local-openai#token` is exactly that), and
where the grant that exists is a read, the gateway uses it. Nothing is
widened — the router presents this host's consumer identity and the authority
still decides, and the coordinate comes from the operator's routes table
rather than a guess. Startup, alias resolution, and `/readyz` all ask the
same question the request path asks: is there a capability or a read grant
for this provider.

In standalone desktop deployments there is no broker: the launcher passes a
provider-to-credential JSON object once over `brama serve
--local-credentials-stdin`, and the plaintext lives only in zeroizing process
memory for that server lifetime.

## What a capability is not

Holding a direct capability for a provider does not let a caller spend an
agent's subscription on that provider, and owning a subscription does not
unlock the deployment's direct credential — the two pools never substitute
for each other, and no fallback between them is silent. Skarbiec owns the
secrets and the redemption verdict; Brama owns only the seam. Which pool pays
for which request shape is defined in
[clients and aliases](clients-and-aliases.md) and
[subscriptions](subscriptions.md).

## Deployment-managed inference routes

Brama does not start or supervise a local inference engine. The deployment
owner controls the digest-pinned vLLM lifecycle; Brama reads the owner-only
snapshot named by `BRAMA_INFERENCE_ROUTES_FILE` per request, rejects symlinks
and group/other-readable files, accepts only loopback or Tailscale IPv4
deployment endpoints, fails closed on malformed updates, and attempts
centrally declared fallback routes in order. `local-openai` routes resolve
their base URL through this file.

## Bounds

Per attempt: 255-second provider timeout (applied between reads on a
stream), response bodies capped at 16 MiB before UTF-8 or JSON parsing.
Per request: one attempt for a direct route; ordered fallback routes are
attempted only after a failed attempt; the whole-request deadline is 300
seconds. Model catalog calls have a 30-second timeout, provider model
discovery 20 seconds, OAuth refresh 15 seconds. Only authentication, quota,
and rate-limit failures rotate credentials; permanent or malformed provider
failures stop replay and return the normalized failure in
[the error envelope](errors.md).

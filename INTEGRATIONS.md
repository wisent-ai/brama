# Integration contracts

Integrations extend the stable core in [`CORE.md`](CORE.md). They do not redefine
client identity, subscription ownership, routing limits, state authority, or
public error semantics.

## Adapter boundary

`subscription_dispatch` owns provider-neutral eligibility and bounded selection.
`providers` owns external protocol adapters. An adapter owns:

- endpoint and trusted-host policy;
- credential extraction and provider authorization shape;
- request and response translation;
- model discovery and capability declaration;
- timeout and response limits;
- provider-specific error classification and telemetry;
- compatibility and disable/removal behavior.

Core HTTP handlers never construct raw provider requests. Protocol adapters are
shared only when their wire behavior is genuinely compatible; provider identity
and endpoint policy remain explicit.

## Capability declaration

Every catalog entry distinguishes:

- executable versus unsupported protocol;
- text and image input modalities;
- tool calling;
- reasoning metadata;
- context and output limits;
- input, output, cache-read, and cache-write price metadata;
- direct capability configured versus agent subscription available.

Catalog metadata is untrusted advisory data. A model is not executable merely
because models.dev lists it; Brama must have a supported protocol, trusted
endpoint, and correct capability.

## Summary

| Integration | Protocol | Identity | Current capability | Runtime owner |
|---|---|---|---|---|
| OpenAI, OpenRouter, Groq, Mistral, xAI, DeepSeek, Cerebras, Fireworks, Together, NVIDIA, Moonshot, Z.AI, Qwen, Hugging Face, Venice, Novita, Synthetic, Kimi | OpenAI Chat compatible | direct API capability or agent subscription | buffered chat; tools where advertised | Brama operator / subscription owner |
| Anthropic and Claude Code | Anthropic Messages | direct API capability or agent OAuth subscription | buffered chat and tool use | Brama operator / subscription owner |
| Codex | OpenAI Responses event stream consumed internally | agent OAuth subscription | buffered normalized final response and tool calls | subscription owner |
| Google catalog providers | GenerateContent | per-subscription API capability | buffered chat and tools where advertised | subscription owner |
| OpenAI embeddings | OpenAI embeddings | direct OpenAI capability | alias-only typed endpoint | Brama operator |
| OpenAI moderation | OpenAI moderation | direct OpenAI capability | alias-only typed endpoint | Brama operator |
| models.dev | HTTPS JSON catalog | no credential | public metadata and protocol discovery | models.dev; cached by Brama |
| Skarbiec capability broker | `skarbiec.redeem.v1` over Unix socket | Brama workload Ed25519 proof | final-use bounded secret redemption | Skarbiec operator |
| entitlements router | local process/CLI | local vault authorization | live subscription list, credential put, capability service | Skarbiec operator |
| GitHub Releases and host service manager | HTTPS assets plus operator-owned installation | GitHub publisher and host operator | immutable publication and versioned host installation | Brama release owner / deployment operator |
| Weles reauthentication | dedicated HTTPS operation | finite Brama reauth token | refresh of the accepted runtime identity only | Weles operator |

`POST /v1/chat/completions` accepts OpenAI-compatible `tool_choice`. OpenAI Chat
providers receive it unchanged; the Responses, Anthropic, and Google adapters
translate a named function choice into their native forced-tool shape.

## Model providers

### Outcome

Execute one provider-neutral `ModelRequest` and return one normalized
`ModelResponse` without exposing provider credentials or raw SDK types to the
caller.

### Configuration and identity

- direct capabilities use key `provider:<slugged-provider>`;
- subscription capabilities use internal subscription ID and resource
  `provider:<provider>:<subscription>`;
- endpoint overrides are namespaced by provider and accept HTTPS approved hosts
  or explicit loopback only;
- ambient proxy, redirects, metadata identity, and unrelated provider
  credentials are disabled as fallback paths;
- credentials are redeemed immediately before each attempt.

### Reliability

- provider attempt timeout: 255 seconds;
- provider response bodies are capped at 16 MiB before UTF-8 or JSON parsing;
- selector and credential attempts are bounded by [`CORE.md`](CORE.md);
- only authentication, quota, and rate-limit exhaustion rotate credentials;
- permanent authentication failure appends a retirement marker;
- malformed or permanent failures stop replay and return normalized failure;
- provider status and body are treated as untrusted and bounded before parsing.

### Compatibility

Brama supports the request/response shapes implemented by its protocol adapters,
not every feature exposed by a provider SDK. Streaming output, batch APIs,
files, assistants, fine-tuning, and provider control planes are unsupported.
Provider-only corrections may remain compatible when the normalized contract is
unchanged; schema, behavior, or error changes visible to callers follow
[`RELEASE.md`](RELEASE.md).

### Disable and removal

Remove the provider capability mapping and alias references, stop advertising
its executable routes, revoke the exact direct or subscription capability, and
retain journal evidence according to policy. Removing one provider must not
prevent unrelated direct routes, local detection, version, or MCP startup.

## models.dev catalog

### Outcome

Provide public provider/model metadata without making the catalog a credential,
subscription, or billing authority.

### Data and limits

- default origin: `https://models.dev/api.json`;
- request timeout: 30 seconds;
- default cache TTL: 900 seconds;
- default replaceable cache: `/tmp/brama-models-dev-cache.json`;
- provider/model identifiers and numeric metadata are validated and normalized;
- unsupported protocols remain non-executable.

### Failure and removal

Catalog outage sets discovery to degraded and may use the bounded local cache.
It must not unlock a credential, manufacture availability, or corrupt journal
state. Removing the integration disables dynamic public metadata; statically
registered, explicitly configured routes remain governed by their own contract.

## Skarbiec capability broker

### Outcome

Redeem one opaque capability for one expected purpose/resource tuple at the
trusted final-use boundary.

### Identity and data

Brama signs `skarbiec.redeem.v1` with its owner-protected workload key. It accepts
one bounded control line and at most 64 KiB of secret bytes followed by EOF.
Capability IDs are lowercase 64-character hexadecimal handles; local tuple
validation prevents accidental use at another seam, while the broker remains
authoritative.

### Failure and removal

Missing socket, registry, proof key, invalid ownership/mode, malformed response,
or tuple mismatch fails closed. There is no environment-secret or remote-vault
fallback. Stopping the broker prevents credentialed inference but must not
prevent `brama detect`, `brama version`, or read-only MCP detection.

## Entitlements router

### Outcome

Discover one agent's non-deleted subscription resources and persist an OAuth
refresh or authorized donation without placing plaintext in Brama state.

### Data and reliability

- live `list` output is validated as untrusted JSON;
- only resource tails beginning `brama-sub-<slugged-agent>-` are eligible;
- successful live results are cached per agent for 60 seconds;
- failed live discovery may use only the trusted startup metadata snapshot;
- credential writes pass secret bytes through child stdin;
- donation metadata uses an owner-only atomic file replacement.

### Disable and removal

Stop new subscription selection, revoke router/vault access, preserve existing
journal evidence, and remove only Brama-owned non-secret overlay state after its
retention decision. Direct provider routes remain usable when configured.

## Publication and host installation

### Outcome

Publish immutable per-platform archives and checksums on GitHub Releases.
Deployment operators download one exact version, verify its digest, install it
under a versioned host path, and control process activation with their own
service manager.

### Identity

GitHub publication, host installation, bearer verification, request-sign
reading, and provider access are separate identities. Runtime credentials never
enter GitHub, a release archive, or a command line.

### Failure and removal

Build, checksum, publication, download, and host activation failures stop
promotion. GitHub availability does not control an already installed service.
Decommissioning stops the host process, revokes runtime grants separately,
preserves the previous version until retention expires, and never deletes
provider vault resources as a shortcut.

## Integration health

Public `/health` proves only that ingress and static startup policy are ready. It
returns `status: "ok"`, build identity, and `dependencies: "not_probed"`.
It never redeems a credential, lists agent subscriptions, contacts a catalog or
model provider, or performs a billable operation.

Protected `/stats` exposes bounded aggregate routing telemetry and configured
direct-provider count without probing dependencies. Per-request structured
errors and logs carry actual integration state:

- `dependency_unavailable`: the required broker, catalog, or provider seam
  could not serve that workflow;
- `dependency_timeout`: the whole request deadline elapsed;
- `provider_rate_limited` or `subscription_unavailable`: bounded capacity was
  exhausted;
- `provider_failure`: a permanent or malformed provider result stopped replay.

Logs identify the integration class, route, bounded attempt count, outcome, and
remediation code without capability IDs, raw external payloads, prompts, or
credentials.

## Ownership and change gate

Brama maintainers own normalized protocols and routing. Provider-account owners
own subscriptions, billing, quota, and revocation. Skarbiec owns secret
authority; Brama release owners own GitHub publication; deployment operators
own installation, process control, network, and host recovery.

An integration change is complete only when capability, configuration, data,
timeout, errors, compatibility, observability, disablement, removal, examples,
and approved evidence agree.

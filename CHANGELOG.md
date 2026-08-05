# Changelog

All notable Brama changes are documented here. Brama follows Semantic
Versioning and the pre-one compatibility policy in [`RELEASE.md`](RELEASE.md).

## 0.2.2 - 2026-08-04

The `v0.2.0` tag produced no release artifacts because its external onboarding
source was not yet pinned for isolated CI builds.
The `v0.2.1` artifacts were built but not qualified or promoted because the
Weles reauthorization reader still expected the retired Skarbiec item shape.

### Added

- Product-contract README, onboarding, core, integration, release, security,
  support, example, and qualification documentation.
- Secret-free CLI and health build identity.
- Stable machine-readable error codes, retryability, and attempt counts.
- Whole-request, selector, credential, output-token, collection, and cost
  acknowledgement limits.
- Public-surface versioning for HTTP, MCP, configuration, state, and CLI
  contracts rather than command names alone.
- Immutable release provenance and digest sidecars.
- Signed immutable Stado release publication, promotion, blue-green rollout,
  centralized status, quarantine, and automatic rollback integration.

### Changed

- Brama is documented as an HTTP gateway and hardware detector, not a local
  inference runtime manager.
- Billable CLI operations require explicit cost acknowledgement.
- Selector routing is bounded to three model candidates and six provider calls;
  explicit subscription routing is bounded to two provider calls.
- The global output-token limit is 32768 and the whole inference request deadline
  is 300 seconds.
- Provider failures use stable normalized HTTP semantics.

### Removed

- The unused `subscriptionDecisionId` request field.
- The undocumented `brama_models` MCP claim. The read-only MCP surface exposes
  `brama_detect` only.

### Security

- No credential, identity, provider, transport, or state fallback was added.
- Routing logs are bounded and prohibit bearer, HMAC, capability, prompt, and
  provider-secret material.

### Compatibility and operator action

These changes break the previous source contract and require version `0.2.0`.
Callers must remove `subscriptionDecisionId`, honor structured error codes and
attempt bounds, and pass explicit cost acknowledgement for billable CLI
operations. No durable journal migration is required.

### Qualification

No test, test suite, smoke test, provider call, or deployment validation was
created, modified, or executed while preparing these changes because explicit
human testing consent was not provided. Release qualification remains blocked.

# Changelog

All notable Brama changes are documented here. Brama follows Semantic
Versioning and the pre-one compatibility policy in [`RELEASE.md`](RELEASE.md).

## Unreleased

### Fixed

- Documentation asserted that no immutable public release existed while five
  were published. `README.md`, `ONBOARDING.md`, `RELEASE.md`,
  `examples/README.md`, and `examples/recovery/upgrade-and-rollback.md` now name
  the newest published release and give the download-and-verify install path
  instead of sending every reader to build from source.
- `RELEASE.md` listed an archive layout the release workflow does not produce.
  It now records the real contents, including the bundled Skarbiec entitlements
  router and the launcher's default trust material, and states that bundled
  material as a known gap instead of claiming that credentials are never
  shipped.
- `released-surface.json` recorded `0.2.4` after `0.2.5` was published;
  regenerated with `scripts/baseline.py --write`.

### Qualification

No test, test suite, smoke test, provider call, or deployment validation was
created, modified, or executed while preparing these changes, because explicit
human testing consent was not provided.

## 0.2.5 - 2026-08-05

*Notes reconstructed from Git history; this release was published without them.*

### Changed

- Provider adapters, the OAuth refresh path, and the subscription model catalog
  now share one HTTP client instead of constructing one per request, so
  connections are reused across calls rather than repeating connection and TLS
  setup for every request.
- The operator API source is rustfmt-clean.
- `released-surface.json` was regenerated for the published `0.2.4` release.

### Compatibility and operator action

No HTTP, CLI, MCP, configuration, or journal contract changed. No operator
action is required.

## 0.2.4 - 2026-08-05

*Notes reconstructed from Git history; this release was published without them.*

### Fixed

- The inference routes file is validated on the snapshot path as well, matching
  the validation already applied on the update path.

### Compatibility and operator action

No contract change. No operator action is required.

## 0.2.3 - 2026-08-05

*Notes reconstructed from Git history; this release was published without them.*

### Added

- Desktop control plane: `GET /v1/admin/snapshot`, `PUT /v1/admin/routes`, and
  the `GET`, `POST`, and `DELETE` `/v1/admin/subscriptions/:agent_id` family.
  Only `brama-desktop` may call them, and responses carry identifiers and status
  only; subscription credentials remain write-only.
- `lem` is declared a model-router client.

### Fixed

- A refreshed provider OAuth grant is now used even when it cannot be persisted
  through the local entitlements router. That path previously returned the
  expired credential, so a request could fail with a grant Brama had already
  renewed.
- `scripts/share-service-items.sh` provisions Jeden recipient access.
- Release control fails fast without a qualification record, and downloads
  release assets without requiring `gh` on the host.

### Compatibility and operator action

HTTP additions only; the CLI surface is unchanged. An operator exposing the
desktop control plane must give `brama-desktop` its own bearer and local
Ed25519 workload identity as described in `README.md`.

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

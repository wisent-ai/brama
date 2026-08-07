# Changelog

All notable Brama changes are documented here. Brama follows Semantic
Versioning and the pre-one compatibility policy in [`RELEASE.md`](RELEASE.md).

## Unreleased

### Released under the wrong name

`v0.2.9`, `v0.2.10`, `v0.2.11`, `v0.2.12` and `v0.2.13` were cut from trees that
already declared `0.3.0`, so five published releases carry this breaking change
under patch-looking names. Anyone who upgraded to one of them has a service that
will not start until `bin/provision-skarbiec-trust` has run, and the version
number gave no warning.

A published coordinate is immutable, so the names cannot be corrected. Each of the
five release notes now opens with what the artifact actually contains and what the
operator must do. `scripts/baseline.py` no longer aborts when it meets a release
whose tree disagrees with its name — it reports it, skips it, and keeps looking —
because before that fix one mis-named release froze the baseline eight releases
behind.

### Security

- The release archive no longer contains any signing key. Until now the release
  build ran `scripts/generate-skarbiec-config.mjs`, so every download of one
  archive carried the same Ed25519 workload proof seed in
  `etc/brama-skarbiec/brama-proof.key`, alongside a signed ten-year `policy.json`
  granting provider authentication and request signing, a `trust.json` vouching
  for both, and a `worm-receipt` stub that discarded audit records while the
  policy pinned its digest. The launcher defaulted to exactly that directory,
  which is always present in a published archive.
- Trust material is now generated per installation by
  `bin/provision-skarbiec-trust`, and `bin/start-with-skarbiec` refuses to start
  while any of it is missing rather than falling back to a shared copy. The
  registry pins the absolute path and SHA-256 of the binary allowed to redeem a
  capability, which is knowledge a build machine does not have, so generating it
  on the host is also the only way that pin can be correct.

### Changed

- **Incompatible.** `etc/brama-skarbiec` in the archive now holds only
  `subscriptions.json` and `recipient-public-keys.asc`; the generator ships as
  `libexec/generate-skarbiec-config.mjs` and is not executed at build time. An
  installation that relied on the bundled trust material must run
  `bin/provision-skarbiec-trust` once and re-grant the capabilities that were
  bound to the discarded key. Under the pre-one policy a breaking change advances
  `MINOR` from the published `0.2.5`, so this is `0.3.0`.

### Fixed

- Documentation asserted that no immutable public release existed while five
  were published. `README.md`, `ONBOARDING.md`, `RELEASE.md`,
  `examples/README.md`, and `examples/recovery/upgrade-and-rollback.md` now give
  the download-and-verify install path instead of sending every reader to build
  from source.
- Documentation no longer restates which release is newest, which source version
  is current, or which version an example targets. Every such literal went stale
  within hours of being written: the first correction of this section named
  `v0.2.5` in prose, and two further versions were cut the same day. Prose now
  points at the two machine-maintained records — `released-surface.json` and the
  GitHub Releases list — and `Cargo.toml` stays the single source of the source
  version rather than being duplicated into a sentence.
- `RELEASE.md` records that a tag alone is not a release, because a tagged
  revision whose release workflow did not finish leaves a coordinate with
  nothing installable behind it. This has now happened twice, at `v0.2.0` and
  again at `v0.2.6`.
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

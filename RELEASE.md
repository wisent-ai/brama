# Release and versioning

The [root README](README.md) defines Brama's product contract. This document
defines how that contract is versioned, built, published, upgraded, and
recovered.

## Current release state

Brama is pre-1.0 and does publish immutable binary releases. The newest complete
release is `v0.2.5`: a non-draft, non-prerelease GitHub Release carrying both
supported platform archives and both checksums. `main` remains mutable
development source and is never a production coordinate.

`released-surface.json` records the newest release consumers can actually
obtain. Regenerate it with `scripts/baseline.py --write`, which recovers the
surface from that release, never from the working tree.

## Canonical product version

The `version` field in `Cargo.toml` is the only product version source. CLI and
MCP version output, health output, release manifests, tags, artifact paths, and
release notes derive from it.

Product version and source revision are separate:

- `product_version`: SemVer from `Cargo.toml`;
- `source_revision`: the exact Git commit selected for the build;
- `platform`: target OS and architecture;
- `artifact_digest`: SHA-256 of the immutable runtime archive;
- `built_at`: UTC build timestamp;
- `provenance`: the release manifest shipped with the artifact.

A commit SHA must not be presented as the product version. An artifact without
all five identities is not releasable.

## Version policy and public contract

Brama uses Semantic Versioning. Before `1.0.0`:

- an incompatible change advances `MINOR` and resets `PATCH`;
- an additive or corrective compatible change advances `PATCH`;
- `1.0.0` is an explicit stability declaration.

The public contract includes:

- CLI commands, flags, machine-readable output, and exit behavior;
- MCP protocol version, tool names, schemas, and read-only boundary;
- HTTP methods, paths, request and response fields, status codes, error codes,
  selectors, aliases, and authentication headers;
- environment and generated configuration fields;
- journal record kinds and durable state interpretation;
- capability resource naming and ownership;
- release archive layout and launcher behavior;
- documented limits, retry, cost, compatibility, and recovery behavior.

`scripts/surface.py` extracts the mechanically visible part of this contract.
`released-surface.json` is the immutable published baseline, never a candidate
regenerated from the same working tree. A release owner records semantic
breakage that extraction cannot see in the release notes and selects the larger
required version change.

## Release channels

- **Development:** `main`; maintainers only; mutable; no production guarantee.
- **Preview:** immutable `vX.Y.Z-rc.N`; staging and explicit canaries; retained
  with qualification evidence.
- **Stable:** immutable `vX.Y.Z`; production callers; promoted from the exact
  preview bytes whenever a preview exists.

There is no `latest` production contract. Discovery labels may point to an
immutable release, but installation and rollback always resolve SemVer,
revision, platform, and digest.

## Release artifacts

A release publishes immutable assets on the matching GitHub Release:

```text
brama-vX.Y.Z-linux-amd64.tar.gz
brama-vX.Y.Z-linux-amd64.tar.gz.sha256
brama-vX.Y.Z-darwin-arm64.tar.gz
brama-vX.Y.Z-darwin-arm64.tar.gz.sha256
```

Each archive is self-contained and expands to:

```text
bin/brama
bin/skarbiec-entitlements-router
bin/start-with-skarbiec
etc/brama-skarbiec/subscriptions.json
etc/brama-skarbiec/recipient-public-keys.asc
etc/brama-skarbiec/trust.json
etc/brama-skarbiec/policy.json
etc/brama-skarbiec/policy.sig
etc/brama-skarbiec/registry.json
etc/brama-skarbiec/registry.sig
etc/brama-skarbiec/brama-proof.key
etc/brama-skarbiec/worm-receipt
LICENSE
provenance.json
```

`bin/skarbiec-entitlements-router` is a second product's binary, built from the
revision pinned by the `SKARBIEC_RELEASE_REVISION` repository variable. Brama
cannot redeem a provider capability without it, so the release ships it rather
than asking an operator to match two versions by hand. `provenance.json`
records product name, product version, source revision, that Skarbiec revision
under `dependencies.skarbiec`, platform, build timestamp, and builder identity.
The `.sha256` sidecar records the archive digest because an archive cannot
contain its own final digest.

`etc/brama-skarbiec` is the launcher's default trust material, generated during
the release build by `scripts/generate-skarbiec-config.mjs`.

An archive never contains the Skarbiec vault, provider credentials, host-specific
runtime configuration, or Brama journal state. `SKARBIEC_VAULT_FILE` must always
be supplied by the operator.

### Open defect: bundled workload trust material

Because `etc/brama-skarbiec` is generated at build time, every download of one
archive carries the same Ed25519 seed in `brama-proof.key`, the same signed
ten-year `policy.json`, and the same `trust.json` that vouches for both.
`bin/start-with-skarbiec` defaults to exactly that directory when the bundle
layout is present, which it always is in a published archive.

The bundled `registry.json` declares the workload's executable path, its
SHA-256, and on macOS a code-signing requirement next to the public half of that
seed, so the seed is not by itself a bearer token; whether a mismatch is refused
is Skarbiec's contract and not a claim this document makes. `worm-receipt` is a
stub that discards what it is given while `policy.json` pins its digest as
`worm_command_sha256`, so the bundled default keeps no audit trail.

A per-installation seed and a real receipt sink are what this contract requires,
and the bundled material is neither. Until provisioning moves per installation,
an operator must point `BRAMA_SKARBIEC_CONFIG_DIR` at their own material. The
bundled directory is a known gap, not a supported production configuration.

## Qualification gate

Before creating a tag or publishing a stable GitHub Release, the release owner records:

- README, onboarding, core, integration, example, and test contracts agree;
- the public-surface decision and required version agree;
- the selected clean revision and builder inputs are immutable;
- the built binary reports the expected product version and source revision;
- archive layout, provenance, and digest are complete;
- approved local, integration, credentialed, recovery, and security evidence is
  recorded with omitted layers called out;
- upgrade and rollback are actionable for the candidate;
- `CHANGELOG.md` contains user-impact release notes.

No narrow check qualifies unexecuted layers.

## Release procedure

1. Select one clean source revision from `main`.
2. Review the full public contract, not only CLI command names.
3. Select and commit the required SemVer change in `Cargo.toml` and lockfile.
4. Update `CHANGELOG.md`, compatibility, migration, and operator actions.
5. Build once in the digest-pinned builder with `--locked`.
6. Produce provenance and the runtime archive for one platform.
7. Record SHA-256 and qualification evidence.
8. Create the immutable SemVer tag for that exact revision.
9. Let `.github/workflows/release.yml` publish every supported archive and
   checksum to the matching GitHub Release without overwrite.
10. Download the target archive on the service host and verify its checksum.
11. Install it under an immutable versioned path and switch the host service
    manager only after operator-approved checks.
12. Update `released-surface.json` from the complete release consumers can obtain.

Steps that execute validation or contact environments require the explicit
approval described in [`TESTING.md`](TESTING.md).

## Release notes

Each release section in `CHANGELOG.md` states:

- added, changed, corrected, removed, and deprecated behavior;
- security-relevant changes;
- HTTP, CLI, MCP, configuration, state, and capability changes;
- compatibility and migration requirements;
- operator actions before and after upgrade;
- known limitations;
- exact source revision, platforms, artifact digests, and provenance objects.

Commit titles alone are not release notes.

## Compatibility and migrations

Every release declares compatibility for:

- callers versus server;
- launcher, packaged router, and Brama binary;
- current and previous configuration documents;
- journal record versions and donated-subscription overlay;
- Skarbiec wire protocol and capability resource naming;
- provider protocols and catalog schema;
- rolling or mixed-version service operation.

Brama currently has no database migration. Any future durable-state change must
name preconditions, backup, duration, service impact, retry/resume behavior,
forward path, and rollback. A release must not silently reinterpret journal
records.

## Upgrade

1. Resolve the target SemVer, platform, archive URL, and digest from GitHub Releases.
2. Read every intervening release note and required operator action.
3. Preserve the append-only journal and any operator-required vault backup.
4. Download the immutable archive to the operator-managed host.
5. Verify the published checksum before extraction.
6. Install under a new versioned directory and update the host service manager.
7. Confirm the approved health/version outcome and one authorized catalog path.
8. Retain the previous immutable runtime until the rollback window closes.

## Rollback and recovery

Rollback identifies the previous SemVer, source revision, platform, digest, and
staged launcher path. The operator must:

1. stop promotion and prevent concurrent launchers from mutating the same
   overlay;
2. preserve journal and capability evidence;
3. confirm the previous release can interpret current non-secret state;
4. point the host service manager at the previous verified installation;
5. confirm build identity and the approved health/version outcome;
6. confirm one authorized non-billable discovery path;
7. revoke or rotate credentials only when compromise, not code regression,
   requires it.

If a release changes durable state incompatibly and no reverse migration is
qualified, rollback is unsupported and the release must say so before upgrade.

## Security and ownership

Build, signing, GitHub publication, bearer verification, request-sign, and
provider runtime identities are separate least-privilege credentials. Runtime
credentials never enter artifacts, provenance, or GitHub. Release maintainers
own source/version/provenance; deployment operators own host, network, runtime
grants, state backup, and rollback execution.

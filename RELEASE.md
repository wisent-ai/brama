# Release and versioning

The [root README](README.md) defines Brama's product contract. This document
defines how that contract is versioned, built, published, upgraded, and
recovered.

## Current release state

Brama is pre-1.0 and currently has no published immutable release. `main` is a
mutable development source, not a production coordinate. The checked-in
`released-surface.json` must state this honestly until Stado contains a real
Brama release or an honest SemVer tag identifies one.

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

A release publishes without overwrite:

```text
stado://releases/brama/<semver>/<platform>/brama-runtime.tar.gz
stado://releases/brama/<semver>/<platform>/brama-runtime.tar.gz.sha256
stado://releases/brama/<semver>/<platform>/provenance.json
```

The runtime contains:

```text
bin/brama
bin/start-with-skarbiec
bin/skarbiec-entitlements-router
bin/stado
etc/brama-skarbiec/*
share/brama/provenance.json
```

`provenance.json` records product name, product version, source revision,
platform, build timestamp, archive filename, builder identity, and dependency
lock identity. The sidecar records the archive digest because an archive cannot
contain its own final digest.

## Qualification gate

Before creating a tag or publishing a stable object, the release owner records:

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
9. Publish archive, digest, and provenance with `--if-absent` semantics.
10. Stage those same bytes on the registered service host.
11. Deploy through Stado and confirm the approved public health/version result.
12. Promote the same verified bytes; never rebuild per channel.
13. Update `released-surface.json` from the release that consumers can obtain.

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

1. Resolve the target SemVer, platform, provenance, and digest.
2. Read every intervening release note and required operator action.
3. Preserve the append-only journal and any operator-required vault backup.
4. Materialize the immutable archive on the registered host.
5. Verify the archive digest before extraction.
6. Deploy the packaged launcher through Stado.
7. Confirm the approved health/version outcome and one authorized catalog path.
8. Retain the previous immutable runtime until the rollback window closes.

## Rollback and recovery

Rollback identifies the previous SemVer, source revision, platform, digest, and
staged launcher path. The operator must:

1. stop promotion and prevent concurrent launchers from mutating the same
   overlay;
2. preserve journal and capability evidence;
3. confirm the previous release can interpret current non-secret state;
4. deploy the previous immutable launcher through Stado;
5. confirm build identity and the approved health/version outcome;
6. confirm one authorized non-billable discovery path;
7. revoke or rotate credentials only when compromise, not code regression,
   requires it.

If a release changes durable state incompatibly and no reverse migration is
qualified, rollback is unsupported and the release must say so before upgrade.

## Security and ownership

Build, signing, publication, Stado control, bearer verification, request-sign,
and provider runtime identities are separate least-privilege credentials.
Runtime credentials never enter artifacts, provenance, or GitHub. Release
maintainers own source/version/provenance; deployment operators own host,
network, runtime grants, state backup, and rollback execution.

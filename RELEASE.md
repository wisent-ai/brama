# Release and versioning

The [root README](README.md) defines Brama's product contract. This document
defines how that contract is versioned, built, published, upgraded, and
recovered.

## Current release state

Brama is pre-1.0 and does publish immutable binary releases. `main` remains
mutable development source and is never a production coordinate.

Which release is newest is deliberately not restated here. A version written
into prose goes stale the moment the next tag lands, which is how this section
came to claim that no release existed while five were published. Two records
answer it instead, and a machine maintains both:

- `released-surface.json` names the newest release this repository has recorded;
  `scripts/baseline.py --write` regenerates it from that release rather than
  from the working tree;
- the canonical Stado release receipt names the immutable source snapshot,
  staged archive, digest, qualification, and promoted channel.

A tag alone is not a release. Treat a coordinate as installable only after
Stado has stored the signed release receipt and reconciled its channel.

## Canonical product version

The `version` field in `Cargo.toml` is the only product version source. CLI and
MCP version output, health output, release manifests, tags, artifact paths, and
release notes derive from it.

Product version and source revision are separate:

- `product_version`: SemVer from `Cargo.toml`;
- `source_revision`: the immutable source snapshot selected by Stado;
- `platform`: target OS and architecture;
- `artifact_digest`: SHA-256 of the immutable runtime archive;
- `built_at`: UTC build timestamp;
- `provenance`: the canonical Stado release receipt.

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

`scripts/surface.py` extracts the mechanically visible part of this contract: the
command list a caller invokes by name. `released-surface.json` is the immutable
published baseline, never a candidate regenerated from the same working tree.

Two entries in the list above — the release archive layout and the launcher's
behavior — are contract that no extractor can see, so a release owner declares
that breakage instead of hoping a command list reveals it. The declaration lives
in `declared-breakage.json`, naming the version it belongs to and why, and
`version-check` passes it to the shared rule as `--breaking`, which escalates the
class and can never lower it. It applies only while `Cargo.toml` still declares
that version, so it expires on its own rather than becoming a switch left on that
makes every later release look breaking.

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

A release stores one immutable archive and signed receipt per platform:

```text
stado://releases/brama/X.Y.Z/linux-amd64/release.tar.gz
stado://releases/brama/X.Y.Z/darwin-arm64/release.tar.gz
```

Each archive expands to:

```text
bin/brama
bin/skarbiec-entitlements-router
bin/start-with-skarbiec
bin/provision-skarbiec-trust
libexec/generate-skarbiec-config.mjs
etc/brama-skarbiec/subscriptions.json
etc/brama-skarbiec/recipient-public-keys.asc
LICENSE
```

`bin/skarbiec-entitlements-router` is a second product's binary. The
`.wisent-release.json` input pins its immutable Skarbiec source URI and SHA-256,
and Stado verifies and mounts that snapshot before invoking the build. Brama
therefore ships the compatible router without cloning another provider or
asking an operator to match two versions by hand. Stado records source inputs,
builder identity, stage paths, and the final archive digest in the release
receipt.

An archive never contains the Skarbiec vault, provider credentials, host-specific
runtime configuration, Brama journal state, or any signing key.
`SKARBIEC_VAULT_FILE` must always be supplied by the operator.

### Trust material is provisioned per installation

The signed policy, workload registry, trust root, WORM receipt command and
workload proof key are **not** in the archive. `bin/provision-skarbiec-trust`
generates them on the host, once, before the first start, and
`bin/start-with-skarbiec` refuses to start while any of them is missing.

Two reasons, and the first is the one that decides it. The registry pins the
absolute path and the SHA-256 of the binary permitted to redeem a capability, and
a build machine cannot know where the artifact will be installed. The second is
that a copy generated at build time would ship one Ed25519 proof seed inside
every download of that archive, so the workload identity of every installation
would be a value any downloader already holds.

Re-provisioning replaces the installation's identity, so
`provision-skarbiec-trust` refuses to overwrite existing material unless
`--force` is given, and the capabilities bound to the previous key must be
re-granted afterwards.

When an installation runs against a registry that does not describe it, the
symptom is not a startup failure. Brama comes up, `/health` answers 200, and
every request needing a provider credential returns `503
dependency_unavailable` with `capability redemption denied: peer mismatch` on
stderr. The message names none of the four fields it compared, and three of
them can be right while the fourth is wrong.

The registry pins `executable_path`, `executable_sha256`, `uid` and `gid`, and
on macOS the designated code-signing requirement. The failure seen in practice
was the uid: `generate-skarbiec-config.mjs` defaults to the account the
container image creates, which exists on no host, so an installation
provisioned before this was fixed pinned a workload nobody could be — while
the path and digest in the same record matched exactly, which is what made it
read like a signing or key problem for days. `bin/provision-skarbiec-trust`
now always passes the provisioning account's uid and gid, and
`BRAMA_WORKLOAD_UID` / `BRAMA_WORKLOAD_GID` remain the override.

The launcher now does this itself. When the registry in the installation it is
about to run does not describe that installation - wrong path, wrong digest,
wrong account - it provisions before starting rather than refusing, because the
bundle carries everything the generator needs and the account that runs the
service is the account the registry must name. Copying an installation to a new
directory, which is what the fleet does every time it materialises an artifact
under a digest-named path, therefore heals on the next start instead of
producing a gateway that answers `/health` and serves nothing.

`scripts/brama-diagnose.py` prints that comparison for every installed
generation, beside the units that start Brama, the capabilities actually issued
against the providers the policy grants, each alias route against those
capabilities, where the gateway is reachable and by which scheme, and the
current boot attempt. Run it before concluding anything about a redemption
refusal.

## Qualification gate

Before promoting a stable Stado release, the release owner records:

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
5. Submit the immutable source snapshot to Stado.
6. Let Stado resolve `.wisent-release.json`, verify pinned inputs, run the
   platform recipe, stage the archive, and store the signed release receipt.
7. Promote the exact version to `candidate`.
8. Reconcile the candidate on the target service and record qualification.
9. Promote the same immutable receipts to `stable`.
10. Update `released-surface.json` from the complete canonical release.

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

1. Resolve the target SemVer, platform, archive URI, and digest from Stado.
2. Read every intervening release note and required operator action.
3. Preserve the append-only journal and any operator-required vault backup.
4. Let Stado install the digest-pinned archive under a new versioned directory.
5. Reconcile the selected channel to the host service manager.
6. Confirm the approved health/version outcome and one authorized catalog path.
7. Retain the previous immutable runtime until the rollback window closes.

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

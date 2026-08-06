# Upgrade and roll back an immutable Brama runtime

**Goal:** install one verified GitHub Release and restore the previous known-good installation without changing credentials.

**Status:** complete immutable releases are published; `v0.2.5` is the newest, carrying both platform archives and both checksums. This procedure has not been executed on a host, so it remains an operator procedure rather than recorded evidence.

**Risk:** production-facing recovery mutation. Requires explicit release-owner and deployment-owner approval naming SemVer, source revision, platform, archive digest, host, maintenance window, rollback target, and evidence location.

**Environment:** operator-managed host with a service manager, HTTPS access to GitHub Releases, and least-privilege Skarbiec runtime grants.

**Preconditions:** successful qualification record, immutable SemVer tag, candidate and previous archive digests, backup of non-secret state, and confirmed rollback compatibility. Secrets remain in Skarbiec and never enter an archive or command line.

**Inputs:** release tag, platform archive URL, checksum URL, immutable installation root, service name, current installation link or equivalent service-manager setting, and previous verified installation path.

**Artifacts and side effects:** a new versioned host directory, one service process restart, and operator-owned deployment evidence. Release assets and previous installations are never overwritten.

## Upgrade

1. Download the exact platform archive and `.sha256` sidecar from the matching GitHub Release.
2. Verify the sidecar before extracting anything.
3. Extract into a new versioned directory owned by the service account.
4. Confirm `provenance.json` names the approved version, revision, platform, and builder.
5. Point the host service manager at that directory's `brama` binary and packaged `start-with-skarbiec` launcher.
6. Restart only the Brama service.

Do not copy local `.env`, journal, capability sockets, vaults, or credentials into the installation directory.

Read the new process identity through the approved operational path:

```bash
brama version
curl --fail --silent --show-error "$BRAMA_URL/health"
```

Expected identity matches the approved release record. Health reports `dependencies: "not_probed"`; provider qualification is separate.

## Rollback

When approved success criteria fail and the prior state format remains compatible, point the service manager at the exact previous verified installation and restart Brama. Never rebuild old source, overwrite a GitHub asset, or use mutable `main` as rollback.

Prove the restored process with `version` and `/health`, reconcile request/failure counters, and retain both identities in the incident record.

## Failure path

Digest, provenance, identity, prerequisite, grant, or target-path mismatch stops rollout. Preserve the last known-good process when possible. Do not bypass checksums, widen credentials, delete state, or substitute another product's artifact.

## Cleanup

Remove only failed staging directories owned by this deployment transaction. Retain verified installations, release records, state backup, redacted logs, and incident evidence according to policy. Revoke temporary deployment grants through their owner.

## Next

Follow [`../../RELEASE.md`](../../RELEASE.md) for release gates and [`../../SUPPORT.md`](../../SUPPORT.md) for escalation.

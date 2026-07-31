# Upgrade and roll back an immutable Brama runtime

**Goal:** deploy one content-addressed runtime, prove its identity, and restore the previous known-good runtime without changing credentials.

**Status:** release workflow exists, but no immutable Brama release is currently published. This is a controlled operator procedure, not an executable current-release claim.

**Risk:** production-facing recovery mutation. Requires explicit release-owner and deployment-owner approval naming source revision, SemVer, platform, digest, host, maintenance window, rollback target, and evidence location.

**Environment:** approved self-hosted release runner with pinned Stado and Skarbiec release objects; target host already provisioned with least-privilege runtime grants.

**Preconditions:** successful qualification record, immutable source tag, candidate bundle digest, previous bundle digest, backups of non-secret state, and confirmed rollback compatibility. Secrets remain in Stado/Skarbiec and never enter the bundle or command line.

**Inputs:** `BRAMA_PRODUCT_VERSION`, `BRAMA_SOURCE_REVISION`, `BRAMA_BUILD_TIMESTAMP`, `BRAMA_BUILDER_IDENTITY`, `STADO_RELEASE_VERSION`, `STADO_RELEASE_PLATFORM`, `STADO_RELEASE_ARCHIVE`, `STADO_RELEASE_SHA256`, `STADO_RELEASE_PROVENANCE`, `STADO_SERVICE_HOST`, `STADO_SERVICE_RELEASE_ROOT`, approved Stado API URL/token capability, and immutable tool binaries.

**Artifacts and side effects:** a new content-addressed release object and target-host version directory; service process restart; ordinary deployment audit evidence. Existing release objects are never overwritten.

## Upgrade

After approvals, package from the exact tagged revision using the pinned workflow equivalent:

```bash
scripts/package-stado-release.sh
```

The workflow records SemVer, source revision, platform, archive, and SHA-256. The deployment runner then invokes:

```bash
scripts/deploy-stado-service.sh
```

It stages to a fresh target directory, verifies the immutable archive, installs `bin/brama`, the pinned entitlement broker and launcher, then asks Stado to switch the service program. Do not copy local `.env`, journal, capability sockets, or credentials into the archive.

Read the new process identity:

```bash
brama version
curl --fail --silent --show-error "$BRAMA_URL/health"
```

Expected identity must match the approved version, source revision, platform, build timestamp, and archive digest in the release record. Health reports `dependencies: "not_probed"`; qualification evidence is separate.

## Rollback

Trigger rollback when the approved success criteria fail and the prior state format remains compatible. Set the deployment inputs to the exact previous immutable version/platform/archive/digest and invoke the same deploy script. Never rebuild the old source or overwrite a release object.

Prove the restored process with `version` and `/health`, reconcile request/failure counters, and retain the failed and restored identities in the incident record.

## Failure path

Digest, identity, prerequisite, grant, or target-path mismatch stops the rollout. Preserve the last known-good process when possible. Do not bypass checksum, widen credentials, delete state, or use mutable `main` as rollback.

## Cleanup

Remove only failed staging directories owned by the deployment transaction. Retain both immutable bundles, release records, state backup, redacted logs, and incident evidence according to policy. Revoke temporary deployment grants through their owner.

## Next

Follow [`../../RELEASE.md`](../../RELEASE.md) for release gates and [`../../SUPPORT.md`](../../SUPPORT.md) for escalation.

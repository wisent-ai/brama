# Security policy

## Reporting

Report vulnerabilities through GitHub's private Security Advisory channel for
[`wisent-ai/brama`](https://github.com/wisent-ai/brama/security/advisories/new).
Do not open a normal issue containing credentials, capability handles, prompts,
provider responses, vault metadata, account identifiers, or exploit details.

If a secret may be exposed, revoke or rotate it at its owning authority first:
client token item, request-sign item, Skarbiec capability, provider account,
Stado consumer, or workload signing key. Preserve bounded evidence without
copying the secret.

## Supported versions

No stable Brama release is currently published. Security fixes apply to the
current development line until a stable release declares its support window in
[`RELEASE.md`](RELEASE.md) and `CHANGELOG.md`. `main` is not a production
release coordinate.

## Security boundaries

- Every non-health HTTP route requires one dedicated client bearer.
- Agent-scoped operations additionally require timestamped exact-body HMAC and
  identity agreement across bearer, header, and path.
- Remote cleartext is rejected; forwarded HTTPS is trusted only from explicitly
  configured peer IPs.
- Provider origins are allowlisted HTTPS endpoints or explicit loopback
  overrides; redirects and ambient proxies are disabled.
- Provider and request-sign secrets are redeemed through finite capabilities at
  final use and are forbidden in Brama state, configuration JSON, logs, errors,
  examples, and release artifacts.
- Direct provider, subscription provider, release publisher, runtime reader,
  bearer verifier, request-sign verifier, and Weles reauth identities are
  separate contracts.
- Retry and response sizes are bounded; permanent auth failure retires only the
  exact credential.
- Health, version, and MCP detection are secret-free and non-billable.

## Response ownership

Repository maintainers triage the software boundary and coordinate a corrected
release. Deployment operators own ingress containment and rollback. Skarbiec and
provider owners own credential revocation and account actions. Stado owners own
release/service access containment. Callers own prompt/data incident response.

## Disclosure expectations

A report should include the affected product version/source revision, public
boundary, impact, minimal redacted reproduction description, and whether a
credential or external provider was involved. Do not execute destructive,
billable, credentialed, provider-facing, or production validation without the
explicit authorization of the owning human operator.

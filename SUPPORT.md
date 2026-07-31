# Support

## Product and development issues

Use the private [`wisent-ai/model-router` issue tracker](https://github.com/wisent-ai/model-router/issues)
for defects, capability requests, documentation gaps, and operator-visible
failures that contain no credentials, prompts, provider responses, account IDs,
or personal data.

Include:

- Brama product version and source revision from `brama version`;
- platform and deployment channel;
- public route or CLI operation;
- normalized error status, code, retryability, and attempt count;
- bounded redacted logs;
- whether a catalog, broker, router, provider, or Stado dependency was involved;
- cleanup and current service state.

Do not include bearer tokens, HMAC signatures, capability IDs, OAuth blobs,
provider API keys, raw request bodies, donated secrets, or vault content.

## Operational ownership

- Brama maintainers own HTTP/CLI/MCP contracts, routing limits, normalized
  errors, protocol adapters, and non-secret journal interpretation.
- Deployment operators own DNS, TLS termination, trusted proxy configuration,
  service host, immutable staging, backup, and rollback execution.
- Skarbiec operators own secret authority, capability policy, vault recovery,
  and entitlements router operation.
- Stado operators own release object/service availability and scoped consumers.
- Provider-account owners own subscription delegation, quota, billing, rotation,
  and revocation.
- Calling-product owners own dedicated bearer use, exact agent signatures,
  allowlisted models, safe retries, and prompt/data classification.

## Security

Security issues use the private process in [`SECURITY.md`](SECURITY.md), not an
ordinary issue. Suspected credential exposure requires immediate revocation at
the owning authority before ordinary debugging.

## Escalation

- `invalid_request`, `unauthenticated`, or `forbidden`: caller/product owner
  first; operator only when central policy is wrong.
- `provider_rate_limited` or `subscription_unavailable`: subscription/provider
  owner first; do not broaden credentials as remediation.
- `dependency_unavailable` or `dependency_timeout`: owner of the named
  integration, then Brama maintainer if classification is wrong.
- `internal_error`: Brama maintainer with redacted correlation evidence.
- release/provenance/digest mismatch: stop deployment and contact release owner;
  never bypass integrity checks.

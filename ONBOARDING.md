# Onboarding

Brama gives Wisent services one authenticated gateway for provider-neutral LLM
inference without distributing provider credentials to callers.

## Choose a path

- **Safe local detection:** no credentials, service, state, provider network, or
  model cost. Every maintainer starts here.
- **Authenticated loopback gateway:** real HTTP boundary on one host; requires
  generated client and Skarbiec capability configuration and may contact a
  provider only when an inference endpoint is called.
- **Production Stado service:** registered Linux host, immutable release, trusted
  TLS terminator, central Stado policy, and scoped Skarbiec grants.

Do not begin with production configuration or a provider call.

## Prerequisites

Required for safe local detection:

- macOS or Linux;
- Git;
- Rust/Cargo compatible with `Cargo.lock`;
- a source checkout while no immutable release is published.

Required for an authenticated loopback gateway:

- all safe-path prerequisites;
- the packaged or explicitly selected `entitlements-router`;
- GnuPG and an authorized local Skarbiec vault;
- an owner-bound capability socket and Brama workload signing key;
- generated client identities, aliases, and capability maps;
- one free loopback port.

Additional production prerequisites:

- supported Linux architecture and the runtime shared libraries in the release;
- registered Stado host and service owner;
- HTTPS ingress whose exact peer IP is in `BRAMA_TRUSTED_PROXY_IPS`;
- Docker and Node on the release runner only, not the runtime host;
- scoped Stado consumers documented in `deploy/*.env.example`;
- operator-owned backup and rollback access to the materialized vault and Brama
  journal.

Check prerequisites with their own version/help commands. Do not paste tokens or
private keys into a shell command merely to check them.

## Release selection

No immutable Brama release is published yet. Therefore normal installation and
production onboarding are unavailable. `main` is a development source and must
not be represented as stable.

After the first release, installation must select:

```text
product version + source revision + platform + archive SHA-256 + provenance
```

from the canonical Stado paths in [`RELEASE.md`](RELEASE.md), verify the digest,
and install the packaged launcher. A source checkout is never the normal
production installation model.

## Safe local first success

### Starting state

Use a clean shell. No Brama, Stado, Skarbiec, provider, or agent environment
variables are required. The command below performs local hardware inspection
only.

### Run detection

```bash
git clone https://github.com/wisent-ai/brama.git brama
cd brama
cargo run --locked -- detect
```

### Observe the result

Expected output has host-specific values and these stable labels:

```text
GPU Type: ...
GPU Name: ...
VRAM: ... GB
RAM: ... GB
CPU Cores: ...
CUDA: ...
Metal: ...
Recommended model: ...
Recommended backend: ...
```

The successful result is the detected resource report and recommendation, not
merely exit code zero.

### Confirm the safe boundary

This path does not:

- start the HTTP service;
- read or create a Brama state directory;
- redeem a capability;
- list subscriptions;
- contact a model or catalog provider;
- incur model cost.

### Stop and clean up

No background process is started. Remove the checkout to remove source and
`target/` build cache. No product state or provider resource requires cleanup.

The canonical task is
[`examples/getting-started/detect-local-resources.md`](examples/getting-started/detect-local-resources.md).

## Zero-state behavior

Running `brama` without a subcommand prints the product purpose, command list,
and help supplied by Clap. It must not create state or contact external systems.

`brama serve` intentionally fails closed when required generated configuration
is absent. The first error identifies the missing configuration category; the
operator then uses the packaged launcher rather than hand-authoring capability
maps. A bare service startup is not the safe first-user path.

## Authenticated loopback path

Use the packaged `start-with-skarbiec` launcher. It owns these steps:

1. load the selected service environment file without overriding individual
   central policy values;
2. materialize the authorized encrypted vault;
3. import the scoped Brama recipient key into an owner-only runtime directory;
4. generate and verify Skarbiec trust, policy, workload, and capability files;
5. read each accepted client bearer from its dedicated item;
6. load central Content Platform, Oko, and Weles request-sign identities;
7. derive provider and agent capability maps from live vault resources;
8. start the owner-bound capability socket;
9. exec `brama serve --port <port>`.

Use loopback HTTP only from an authenticated local caller. A generic bearer-only
`GET /v1/models` is the first non-billable machine workflow. Do not use
`/v1/chat/completions` until the intended provider, account, attempt limit, and
cost are explicit.

## Production path

Production uses the exact runtime built and published by the release workflow.
The runner must be the registered service host or must first materialize the
immutable object there through an approved Stado object/machine path. The
operator configures:

- `STADO_SERVICE_HOST` and immutable release root;
- the central Brama service document containing client allowlists and aliases;
- verifier-only consumers for dedicated client token items;
- request-sign consumers for the exact central identities;
- Brama's provider capability grants;
- the trusted TLS terminator IP list;
- journal backup and rollback retention.

Ingress, DNS, host registration, vault authorization, and consumer grants remain
operator-owned prerequisites. The repository does not create them silently.

## Machine onboarding contract

Automation must use stable machine interfaces:

- `brama version` emits one JSON object with product/build identity;
- `GET /health` emits one secret-free JSON object;
- protected HTTP endpoints use stable status and error codes from
  [`CORE.md`](CORE.md);
- non-health HTTP uses exactly one bearer authorization value;
- agent-scoped operations additionally sign timestamp, agent ID, and exact body;
- configuration is noninteractive and fails before serving on invalid input.

Automation must not parse decorative human log output.

## Credentials and permissions

Keep these identities separate:

- release publisher: write only the Brama release namespace;
- runtime Stado reader: read only the Brama service recipient key;
- bearer verifier: read only each dedicated client token item;
- request-sign verifier: read only the named central HMAC items;
- Brama workload: redeem only configured Brama capability IDs;
- Weles reauthentication: one dedicated finite token, not a console token;
- caller: one dedicated bearer and, when needed, one exact agent identity.

Credentials are rotated or revoked at their authority: Stado consumer, Skarbiec
capability, or product-owned request-sign item. Never place them in committed
env files, URLs, request examples, logs, screenshots, or provider payloads.

## Common failures

### Missing `BRAMA_MODEL_ROUTER_CLIENT_IDENTITIES`

**Observed:** startup fails before binding the port.

**Meaning:** the verifier registry was not generated.

**Action:** launch through the packaged wrapper and confirm the verifier Stado
consumer can read every dedicated client token item. Do not create a shared
fallback token.

### Missing or invalid model aliases

**Observed:** startup reports that the exact alias set or matching provider
capability is unavailable.

**Meaning:** central service policy and capability grants disagree.

**Action:** correct the central policy or grant, then restart. Do not hand-edit a
runtime alias around the policy.

### HTTP 426 `secure_transport_required`

**Observed:** a remote request is rejected before auth dispatch.

**Meaning:** the peer is neither loopback nor an approved HTTPS terminator.

**Action:** use HTTPS and configure the exact terminator IP in
`BRAMA_TRUSTED_PROXY_IPS`; do not trust arbitrary forwarded headers.

### HTTP 401 `unauthenticated`

**Observed:** bearer or agent HMAC is missing, malformed, expired, or unknown.

**Action:** identify the intended client and agent, rotate or repair that exact
credential at its authority, and sign the exact transmitted body.

### HTTP 403 `forbidden`

**Observed:** identity is valid but not allowed for the requested model, agent,
or path.

**Action:** inspect the central client binding and model allowlist. Do not use a
broader client's token.

### HTTP 429 `provider_rate_limited` or `subscription_unavailable`

**Observed:** bounded eligible attempts were exhausted.

**Action:** inspect protected stats and structured routing logs, then wait,
select an authorized billing target, or repair the intended subscription. Do
not retry indefinitely.

### HTTP 503 `dependency_unavailable`

**Observed:** Skarbiec, entitlements router, catalog, or configured provider is
unavailable.

**Action:** restore that named dependency or use a different explicitly allowed
route. No credential or storage fallback is permitted.

### Port unavailable

**Observed:** bind failure on the requested port.

**Action:** stop the conflicting process or select another loopback port. Do not
bind an unauthenticated alternative interface.

## Uninstall and reset

- Stop the Stado service through its owner-approved service operation.
- Revoke runtime, verifier, request-sign, and publisher grants independently.
- Retain or securely remove the journal according to operator policy.
- Remove only the immutable staged release being decommissioned after its
  rollback window closes.
- Remove `/tmp/brama-*` process caches only when no Brama process uses them.
- Provider vault resources remain owned by Skarbiec/entitlements policy; do not
  delete them as an uninstall shortcut.

Continue with [`CORE.md`](CORE.md), [`INTEGRATIONS.md`](INTEGRATIONS.md), and the
risk-labeled [`examples/`](examples/README.md).

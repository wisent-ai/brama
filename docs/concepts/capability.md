# Capability

The authority to obtain a credential at final use. Brama holds no secret at
rest: a capability is a short-lived, single-use handle the local Skarbiec
broker issues and redeems, and the plaintext it yields exists only for the
one provider call it pays for. Skarbiec owns the secrets and the redemption
verdict; Brama owns only the seam.

## Shape

A capability binds three things (`src/gateway/broker.rs`):

- a **purpose** — `brama.provider.authenticate` for provider credentials,
  `brama.request.sign` for agent request-sign secrets;
- a **resource** — the vault coordinate the operator's routes table names:
  `provider:<slugged-provider>` for a direct credential,
  `provider:<provider>:<subscription>` for a subscription credential,
  `agent:<slugged-agent>` for a signing secret. Identifiers are slugged into
  a stable resource alphabet; the original identifier remains the lookup key
  in trusted config;
- an **agent** — capabilities are issued to `brama-runtime`, the acquisition
  consumer bound to the workload proof key this installation provisions
  (deliberately not `brama-service`, which holds only a `read` capability
  and can never redeem).

No lifetime or use count is requested at issue time, and that is the point:
Skarbiec refuses a TTL over an hour and a use count over sixteen, and the
broker's defaults are a short life and a single use. Nothing is cached — a
single-use capability has nothing worth keeping, and an id the launcher put
in the environment of a running process cannot be refreshed at all.

## Lifecycle

1. The launcher seeds one capability id per provider in
   `BRAMA_PROVIDER_CAPABILITY_IDS` at boot. Those expire within the hour, so
   their refusal later is the expected steady state, not a fault.
2. On the request path, immediately before the provider HTTP call, the
   gateway obtains a fresh capability for the purpose and resource and
   redeems it through the owner-bound broker socket.
3. Where issuance or redemption is refused, the gateway falls back to the
   exact field-scoped **read grant** the vault may carry for that coordinate
   (`read:provider:local-openai#token` is exactly that shape). Nothing is
   widened: the router presents this host's consumer identity and the
   authority still decides.
4. The returned secret is used once and dropped; plaintext never outlives
   the call.

Startup, alias resolution, and `/readyz` all ask the same question the
request path asks: is there a capability or a read grant for this provider.

## Standalone mode

In standalone desktop deployments there is no broker: the launcher passes a
provider-to-credential JSON object (`{"openai": "sk-...", ...}`) once over
`brama serve --local-credentials-stdin`. The input is consumed and zeroized;
plaintext then remains only in zeroizing process memory for that server
lifetime. `/readyz` in this mode reports a provider `credential: true` from
the in-memory map without contacting the provider.

## Invariants

- **Two pools, no substitution.** Holding a direct capability for a provider
  does not let a caller spend an agent's [subscription](subscription.md) on
  that provider, and owning a subscription does not unlock the deployment's
  direct credential. No fallback between the pools is silent.
- **Final-use only.** `/stats` reports the dependency policy as
  `"capabilityBroker": "final-use"`: no timer, listing, or catalog read
  redeems a capability; only a request that is about to spend does, plus the
  explicit `/readyz` probe.
- **Coordinates are named in failures.** The dispatch path names the vault
  coordinate in provider failures, because the repair to a credential that
  cannot be used is always at the coordinate it came from.

## Refusals

- A direct route whose provider has no capability, read grant, or in-memory
  credential: `direct '<provider>' credential is unavailable` — surfaced as
  `503` `dependency_unavailable` at the HTTP edge (observed against 0.2.38).
- A refused redemption on the request path is `503 credential_unauthorized`
  with `retryable: false`, never a `429`: waiting does not repair an
  authorization that does not match ([errors](../errors.md)).
- The operator envelope for a capability that yielded no usable credential
  carries the failure point `brama.gateway.credential-redeem`
  ([failure-point](failure-point.md)).

## Not to be confused with

- **A provider.** A [provider](provider.md) is the protocol adapter; the
  capability is the permission to spend through it.
- **A subscription credential.** Same redemption seam, different pool and
  different owner — see [subscription](subscription.md) and
  [entitlement](entitlement.md).

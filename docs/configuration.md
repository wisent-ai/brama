# Configuration

Production configuration is generated, not hand-authored:
`scripts/start-with-skarbiec.sh` assembles the security-bearing variables
from operator-owned policy and scoped Skarbiec consumers, then `exec`s
`brama serve`. Missing, malformed, duplicate, or contradictory security
configuration fails startup, and no credential, provider, identity, or
storage fallback is silent. This page lists every knob the gateway itself
reads, then the launcher's own inputs.

## Listener

| Variable | Default | Meaning |
|---|---|---|
| `--port` (serve flag) | `8080` | listen port |
| `BRAMA_BIND_ADDRESS` | loopback | bind IP; must parse as an IP address. Requests from this address count as local for the transport guard |
| `BRAMA_TRUSTED_PROXY_IPS` | unset | comma-separated TLS-terminator peer IPs whose `https` forwarded headers are trusted; empty trusts none |
| `BRAMA_ENCRYPTED_PEER_IPS` | unset | comma-separated peer IPs whose hop is already encrypted (mesh nodes); accepted without forwarded headers |

## Identity and policy

| Variable | Default | Meaning |
|---|---|---|
| `BRAMA_MODEL_ROUTER_CLIENT_IDENTITIES` | unset | JSON array of `{client_id, token, agent_id?, allowed_models?}`. Absent means "ask the authority", not "misconfigured": unknown bearers resolve against Skarbiec |
| `BRAMA_MODEL_ALIASES` | required in managed mode | JSON object of alias → canonical route. Must contain the seven required aliases; extra names are permitted. In standalone mode absent means `{}` |
| `BRAMA_INFERENCE_ROUTES_FILE` | unset (launcher default `~/.config/brama/inference-routes.json`) | owner-only route snapshot, reloaded per request; rejects symlinks and group/other-readable files |
| `BRAMA_REQUEST_SIGN_IDENTITIES` | unset | central request-sign identity map (Echo, legacy Content Platform, Oko, Weles) |
| `BRAMA_REQUEST_SIGN_CAPABILITY_IDS` | unset | capability ids for agent request-sign secrets |
| `BRAMA_PROVIDER_CAPABILITY_IDS` | unset | one seeded capability id per direct provider |
| `BRAMA_SUBSCRIPTION_CATALOG` | unset | trusted startup subscription metadata, used only when live discovery fails |
| `WC_SKARBIEC_URL`, `BRAMA_SKARBIEC_CONSUMER`, `BRAMA_SKARBIEC_TOKEN_FILE` | unset | bearer introspection against Skarbiec for tokens not in the boot table |
| `BRAMA_WISENT_AUTH_URL`, `BRAMA_WISENT_AUTH_ANON_KEY` | unset | Wisent Identity endpoint and public anon key for desktop user sessions; never a service-role key |
| `ENTITLEMENTS_ROUTER_BIN` | `entitlements-router` | binary shelled for vault listing and credential writes |

## State and ledgers

| Variable | Default | Meaning |
|---|---|---|
| `BRAMA_STATE_DIR` | `$HOME/.brama` | holds `journal.jsonl`, the append-only retirement/quality/refresh journal; readers take the last matching record; credential material is never written here |
| `BRAMA_SUBSCRIPTION_USAGE_FILE` | `~/.config/brama/subscription-usage.json` | per-subscription usage ledger, owner-readable, atomic writes |
| `BRAMA_DONATED_SUBSCRIPTIONS_FILE` | `/tmp/brama-skarbiec/donated-subscriptions.json` | metadata-only donation overlay, atomic 0600 rewrite |
| `BRAMA_PERF_PATH` | `/tmp/brama-perf.json` | replaceable process telemetry |

## Timers and freshness

| Variable | Default | Meaning |
|---|---|---|
| `BRAMA_PLAN_USAGE_TTL_SECS` | `300` | freshness window per subscription's provider usage report; also when a reading turns `stale` |
| `BRAMA_PLAN_USAGE_SWEEP_SECS` | `60` | how often aged-out readings are looked for; `0` disables the sweep |
| `BRAMA_PLAN_USAGE_RETENTION_SECS` | `86400` | when a stale reading stops being served |
| `BRAMA_CREDENTIAL_REFRESH_INTERVAL_SECS` | `60` | refresh-ahead sweep interval; `0` disables it |
| `BRAMA_CREDENTIAL_REFRESH_SKEW_SECS` | `300` | refresh credentials expiring within this window |

With defaults, no timer performs a quota-consuming request: usage reports and
token refreshes are free, and the only quota-spending check is the explicit
admin probe route.

## Catalog and providers

| Variable | Default | Meaning |
|---|---|---|
| `BRAMA_MODEL_CATALOG_URL` | `https://models.dev/api.json` | public metadata origin |
| `BRAMA_MODEL_CATALOG_TTL_SECONDS` | `900` | in-memory catalog TTL |
| `BRAMA_MODEL_CATALOG_PATH` | unset | read the catalog from a local file instead |
| `BRAMA_MODEL_CATALOG_CACHE` | `/tmp/brama-models-dev-cache.json` | replaceable on-disk cache |
| `BRAMA_CATALOG_REVISION` | `brama-v1` | revision string reported by `/v1/models` |
| `BRAMA_PROVIDER_<PROVIDER>_BASE_URL` | built-in | per-provider base override; exact trusted HTTPS host or explicit loopback only |
| `STADO_INTEGRATION_API_URL`, `BRAMA_STADO_INTEGRATION_TOKEN` | unset | optional remote Stado transport for `brama onboard` |

Build identity is baked at compile time from `BRAMA_SOURCE_REVISION`,
`BRAMA_BUILD_PLATFORM`, and `BRAMA_BUILD_TIMESTAMP`, and reported by
`brama version`, `/health`, `/readyz`, and `/stats`.

## The launcher's own inputs

`scripts/start-with-skarbiec.sh` reads `BRAMA_SERVICE_ENV_FILE` (default
`~/.config/brama/service.env`) first, then requires:

- `BRAMA_GNUPG_HOME` — the service's GnuPG home; startup fails without it.
- `SKARBIEC_VAULT_FILE` — the authorized encrypted vault.
- `BRAMA_CONTROL_CONFIG` — the authoritative non-secret service policy
  document; the launcher reads
  `services.brama.{allowed_models,model_aliases,required_provider_capabilities}`
  and rejects absent, malformed, or inconsistent policy.

From those it derives everything in the identity section above, provisions or
verifies this installation's trust material (`BRAMA_SKARBIEC_CONFIG_DIR`,
proof key at `BRAMA_PROOF_KEY_FILE`, default `~/.stado/brama-proof.key`),
starts the owner-bound capability socket (`BRAMA_CAP_SOCKET`, default
`~/.stado/run/brama-capability.sock`), seeds the routes file when absent, and
optionally starts the renewal loop (`BRAMA_RENEWAL_ENABLED`, default on).
`BRAMA_RUNTIME_DIR` (default `/tmp/brama-skarbiec-<installation>`) holds the
per-installation socket, GnuPG, and receipt directories. Runtime capability
IDs are freshly issued from vault bindings — never configure capability IDs
or provider credentials in the non-secret env file. `deploy/*.env.example`
documents the operator-facing subset.

Restart after any startup policy change; the gateway does not reload client
identities, aliases (other than the routes file), or capability maps in
place.

# Inspect build identity, health, and stats

**Goal:** identify the exact Brama process without redeeming a provider credential.

**Status:** implemented on development source `0.1.0`; release requires `0.2.0`.

**Risk:** `brama version` is local read-only; `/health` is public and secret-free; `/stats` is authenticated read-only.

**Environment:** installed binary or source checkout; `/health` and `/stats` additionally require a running loopback or approved HTTPS service.

**Preconditions:** for stats, load the caller's dedicated bearer through its approved consumer into `BRAMA_BEARER`; do not type it into shell history.

**Inputs:** `BRAMA_URL`, defaulting operationally to an approved HTTPS URL or explicit loopback.

**Artifacts and side effects:** none beyond ordinary bounded access logs.

## Steps

Read binary identity:

```bash
brama version
```

Expected JSON shape:

```json
{"product":"brama","version":"0.1.0","source_revision":"...","platform":"...","built_at":"..."}
```

Read public process health:

```bash
curl --fail --silent --show-error "$BRAMA_URL/health"
```

Expected shape includes `status`, `build`, and `dependencies: "not_probed"`. Health deliberately performs no dependency or provider probe.

Read protected process telemetry:

```bash
curl --fail --silent --show-error \
  -H "Authorization: Bearer $BRAMA_BEARER" \
  "$BRAMA_URL/stats"
```

Expected shape includes build identity, request/failure/provider-attempt counters, token totals, configured direct-provider count, and documented limits. It contains no credential or request body.

## Failure path

`401 unauthenticated` on `/stats` means the dedicated bearer is absent or invalid. Repair that exact consumer or token; do not use another product's bearer. A build identity containing `development` or `not-recorded` is acceptable only for an explicitly documented source build, never an immutable release.

## Cleanup

Unset the process-local bearer variable and leave the service unchanged:

```bash
unset BRAMA_BEARER
```

## Next

Continue with [`../core/call-http-api.md`](../core/call-http-api.md) for catalog discovery before any provider call.

# Brama examples

This directory is the canonical catalog of supported Brama user outcomes. Each
example states version/status, risk, environment, preconditions, inputs, side
effects, ordered public-interface steps, expected observable result, failure,
cleanup, and next action.

## Status and consent

This catalog was written against the `0.1.0` development contract and has not
been re-checked command by command against the newest published release, so
treat any version-specific detail below as unverified until it is. The newest
published release is named by `released-surface.json` and by the repository's
GitHub Releases list, never restated here. Examples are **documented, not
executed**: none was run while creating this catalog because explicit testing or
provider-execution consent was not provided. Do not treat a documented example
as qualification evidence.

Risk labels:

- **Local read-only:** no service, credential, provider, or product state.
- **Authenticated read-only:** reads secret-free service metadata but requires a
  dedicated bearer or agent signature.
- **Credentialed/provider-facing:** may redeem a capability and contact an
  external provider.
- **Billable:** may consume provider quota or money.
- **Security mutation:** donates, retires, rotates, or revokes access.
- **Recovery mutation:** changes the deployed immutable runtime.

Credentialed, billable, security-mutation, recovery, and production-facing
examples require the owning human operator's explicit approval before use.

## Coverage matrix

| Actor | Outcome | Interface | Risk | Canonical example | Evidence |
|---|---|---|---|---|---|
| New maintainer | Detect local resources and recommendation | CLI `detect` | Local read-only | [`getting-started/detect-local-resources.md`](getting-started/detect-local-resources.md) | Documented; not executed |
| Operator | Read product/build identity | CLI `version` | Local read-only | [`operations/inspect-build-and-health.md`](operations/inspect-build-and-health.md) | Documented; not executed |
| Operator | Read running process health | `GET /health` | Authenticated transport; endpoint public | [`operations/inspect-build-and-health.md`](operations/inspect-build-and-health.md) | Documented; not executed |
| Service | List bearer-authorized public catalog | `GET /v1/models` | Authenticated read-only | [`core/call-http-api.md`](core/call-http-api.md) | Documented; not executed |
| Service | Execute a direct alias/canonical chat | `POST /v1/chat/completions` | Credentialed, provider-facing, billable | [`core/call-http-api.md`](core/call-http-api.md) | Controlled qualification required |
| Service | Create embeddings | `POST /v1/embeddings` | Credentialed, provider-facing, billable | [`core/call-http-api.md`](core/call-http-api.md) | Controlled qualification required |
| Service | Moderate text | `POST /v1/moderations` | Credentialed, provider-facing, billable | [`core/call-http-api.md`](core/call-http-api.md) | Controlled qualification required |
| Jeden runtime | Use exact subscription billing target | signed chat API | Credentialed, provider-facing, billable | [`core/call-agent-selectors.md`](core/call-agent-selectors.md) | Controlled qualification required |
| Jeden runtime | Select `any` | signed chat API | Credentialed, provider-facing, billable | [`core/call-agent-selectors.md`](core/call-agent-selectors.md) | Controlled qualification required |
| Jeden runtime | Select `any-vision-capable` | signed chat API | Credentialed, provider-facing, billable | [`core/call-agent-selectors.md`](core/call-agent-selectors.md) | Controlled qualification required |
| Jeden runtime | Select `task:<name>` | signed chat API | Credentialed, provider-facing, billable | [`core/call-agent-selectors.md`](core/call-agent-selectors.md) | Controlled qualification required |
| Jeden runtime | List owned subscriptions | signed `GET /v1/subscriptions/:agent` | Authenticated read-only | [`operations/manage-subscriptions.md`](operations/manage-subscriptions.md) | Documented; not executed |
| Subscription owner | Donate Claude Code credential | signed subscription POST | Security mutation | [`operations/manage-subscriptions.md`](operations/manage-subscriptions.md) | Controlled qualification required |
| Subscription owner | Retire delegated subscription | signed subscription DELETE | Security mutation | [`operations/manage-subscriptions.md`](operations/manage-subscriptions.md) | Controlled qualification required |
| Operator | Collect bounded task-quality evidence | CLI `collect-task-quality` | Credentialed, billable; optional state mutation | [`operations/collect-task-quality.md`](operations/collect-task-quality.md) | Controlled qualification required |
| Operator | Run one manual provider diagnostic | CLI `test` | Credentialed, billable | [`operations/collect-task-quality.md`](operations/collect-task-quality.md) | Controlled qualification required |
| Agent/tool host | Detect hardware over MCP | stdio MCP | Local read-only | [`automation/use-mcp-detect.md`](automation/use-mcp-detect.md) | Documented; not executed |
| Operator | Inspect protected process stats | `GET /stats` | Authenticated read-only | [`operations/inspect-build-and-health.md`](operations/inspect-build-and-health.md) | Documented; not executed |
| Caller | Diagnose cleartext, bearer, HMAC, allowlist rejection | HTTP error contract | Read-only negative path | [`failures/auth-and-transport.md`](failures/auth-and-transport.md) | Documented; not executed |
| Caller | Diagnose provider limit, timeout, or outage | HTTP error contract | Provider-facing failure | [`failures/provider-unavailable.md`](failures/provider-unavailable.md) | Controlled qualification required |
| Release operator | Upgrade one immutable runtime | Verified GitHub Release archive | Recovery mutation | [`recovery/upgrade-and-rollback.md`](recovery/upgrade-and-rollback.md) | Controlled qualification required |
| Release operator | Roll back and prove previous identity | Versioned host installation | Recovery mutation | [`recovery/upgrade-and-rollback.md`](recovery/upgrade-and-rollback.md) | Controlled qualification required |
| Developer | Execute local inference in Brama | — | — | Not supported; see README non-goals | Not supported |
| Caller | Stream completion tokens | `POST /v1/chat/completions`, `/v1/messages`, `/v1/responses` with `"stream": true` | Provider-billable generation | [`inference/stream-a-completion.md`](inference/stream-a-completion.md) | Documented; not executed |

## Shared rules

- Use an immutable release and digest when one exists; source checkout is the
  explicitly labeled pre-release exception.
- Obtain bearer, agent secret, and provider capability only through the owning
  consumer/broker. Never paste them into an example, URL, shell history, or
  committed file.
- Keep request bodies in owner-only temporary files when their exact bytes are
  signed.
- Treat every model call and quality collection as billable even when the
  provider later returns an error.
- Do not increase `max_tokens`, candidate count, or retries beyond
  [`CORE.md`](../CORE.md).
- Cleanup only resources created by the example. Retain bounded audit evidence
  and never delete vault resources as a debugging shortcut.
- Expected output shows shape and stable fields, never invented provider text or
  real identifiers.

## Selecting an example

Start with local detection, then inspect build/health, then perform bearer-only
catalog discovery. Provider-facing and mutation examples come only after the
operator confirms identity, account, route, limits, spend, side effects, and
cleanup.

Return to the [product contract](../README.md), [onboarding](../ONBOARDING.md),
[core contract](../CORE.md), [integration contracts](../INTEGRATIONS.md), and
[qualification contract](../TESTING.md).

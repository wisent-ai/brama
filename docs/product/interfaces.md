# Primary interfaces

The HTTP, CLI and MCP surfaces Brama offers, and the contract each keeps. Back
to the [README](../../README.md).

- **HTTP inference:** `POST /v1/chat/completions`, `/v1/messages`,
  `/v1/responses`, `/v1/embeddings`, and `/v1/moderations` are the canonical
  model-execution interfaces. The three chat formats share one routing
  decision, one identity contract, and one attempt budget; they differ only in
  the shape of the request and the answer.
- **Streaming:** any of the three chat formats streams when the request asks
  for it -- `"stream": true` on chat completions and Anthropic Messages,
  `"stream": true` on Responses. The response is `text/event-stream` in the
  caller's own dialect: `chat.completion.chunk` frames closed by
  `data: [DONE]`, Anthropic `message_start`/`content_block_*`/`message_stop`
  events, or `response.*` events closed by `response.completed`. A stream that
  ends without its terminal event is a generation that was cut after it
  committed; the caller holds an incomplete answer and Brama has already
  stopped. Rotation across models and credentials happens only before the
  first byte, so a committed stream is never silently re-run.
- **Authentication:** static and Skarbiec-issued workload/model-client bearers
  remain workload identities and do not send an organization header. A bearer
  validated by Wisent Supabase is a human identity on account, discovery, and
  model routes, and always requires the server-verified organization context
  described below. A missing or malformed organization header returns `400`,
  an invalid bearer `401`, no membership `403`, and an unavailable identity
  authority `503`.
- **Account API keys and subscriptions:** authenticated Wisent users use
  `GET|POST /v1/account/subscriptions` and
  `DELETE /v1/account/subscriptions/:subscription_id`. Every human request sends
  `Authorization: Bearer <Supabase JWT>` and
  `X-Wisent-Organization-ID: <uuid>`. Brama validates the JWT with the Wisent
  Identity authority named by `BRAMA_WISENT_AUTH_URL` and
  `BRAMA_WISENT_AUTH_ANON_KEY` (Stado's service catalog sets both for the
  gateway), calls `authorize_organization` with the same bearer, and accepts
  only the returned user, organization, and typed `owner`, `admin`, or
  `member` role. No authority is compiled in: with either variable unset the
  identity authority is unavailable, logged as `wisent_identity_unconfigured`,
  instead of authentication being weakened. The
  organization is verified request context, not subscription ownership: Brama
  continues to derive the subscription owner from the verified user, and the
  caller never supplies an account or agent identifier. `POST` accepts an API
  key for any supported remote provider, stores it through Skarbiec without
  returning it, and makes that user's canonical `provider/model` routes
  available for buffered and streamed calls.
- **HTTP discovery:** `GET /v1/models`; account discovery combines public
  catalog metadata with models executable by that account's stored keys, while
  signed agent discovery includes the shared subscription pool.
- **Subscription lifecycle:** `GET`, `POST`, and `DELETE`
  `/v1/subscriptions/:agent_id`; always bearer- and HMAC-protected. A `GET`
  returns, per subscription, the plan windows the provider itself reported
  (`limits`: `used_fraction`, `window_label`, `resets_at_ms`, and the
  `recorded_at_ms` the reading was taken at), what Brama measured (`measured`:
  requests, failures, input and output tokens, first and last use), any
  rate-limit `block` in force, when the record last changed
  (`observed_at_ms`), where the newest window came from and whether it is still
  current (`usage_source`: `provider`, `traffic` or `probe`, and `stale`), the
  newest proactive check (`probe`: `attempted_at_ms`, `ok`, `detail` when there
  is something to explain, and `source` -- `usage_report` or `completion`), and
  where the credential itself stands (`credential`: `state` -- `active`,
  `needs_reauthorization` or `disabled` -- with `cause`, `recorded_at_ms`,
  `expires_at_ms` and `refreshed_at_ms`; `null` while nothing has been recorded
  about the grant). `credential.state` is what separates a subscription that is
  quiet from one whose sign-in is overdue, and `cause` is the provider's own
  sentence for the refusal.
- **How fresh a plan window is:** `usage_source` names which statement the
  newest window is -- the provider's own usage report, the headers of real
  traffic, or an operator's probe -- and is `null` when there is no window to
  attribute. `stale` is true when the newest usage report failed or a served
  window's own reset instant has passed. Every recorded reading is served,
  because a number that says when it was taken is information and an empty plan
  is not; a newer reading replaces it.
- **What an empty `limits` array means:** one of four states, and the newest
  check is what tells them apart. Windows present: render them, aged by the
  newest `recorded_at_ms`, and say "as of" only when `stale` is false. Empty with
  `probe.ok` false: the credential or the provider refused, and `probe.detail` is
  its own sentence. Empty with no `probe` and nothing measured: nothing has ever
  gone through this subscription. Empty with `probe.ok` true: the provider
  genuinely publishes no plan state, and `probe.detail` says so when the provider
  publishes no usage report at all. Only the last of those may be shown as "no
  plan window", and none of them is a zero.
- **Operations:** public `GET /health` and `GET /readyz`; protected `GET /stats`.
  `/health` is liveness only and says so in its body (`dependencies:
  not_probed`): it answers `ok` from a gateway whose every credential
  redemption is being refused. A background readiness loop redeems one
  capability per configured provider, discovers active subscription models and
  redeems each active subscription through the request path; `/readyz` returns
  the latest completed result immediately, with no secret in the body. HTTP 200
  and `ready: true` mean at least one configured direct-provider or subscription
  route can carry traffic. `degraded: true` keeps every other denied,
  unredeemable, or unroutable account visible without blocking a release that
  contains its repair; HTTP 503 means no configured route can serve. Deploy
  checks and uptime monitors should read `/readyz`; `/health` only proves the
  process is running.
- **Error contract:** a refused redemption is `503 authorization_error` with
  code `credential_unauthorized` and `retryable: false`, never a `429
  capacity_error`. Waiting does not repair an authorization id that does not
  match, and classifying it as capacity sends the caller into retries and the
  operator into the subscription catalogue.
- **Desktop control plane:** `brama-desktop` alone may call
  `GET /v1/admin/snapshot`, `PUT /v1/admin/routes`, the `GET`, `POST`, and
  `DELETE` `/v1/admin/subscriptions/:agent_id` family, and
  `POST /v1/admin/subscriptions/:agent_id/:subscription_id/probe`, which is the
  only endpoint in the product that deliberately spends plan quota. These
  endpoints return identifiers, usage and status only; subscription credentials
  remain write-only.
- **CLI:** `serve`, `version`, `detect`, `onboard`, `onboard --reset`, `test`,
  `subscriptions list`, `subscription refresh`, `tasks measure|show|list`, and
  `mcp`. Billable commands require an explicit cost acknowledgement, and
  commands that mutate state require an explicit `--reason`.
- **MCP:** read-only stdio JSON-RPC exposing `brama_detect` only. Model execution,
  credential discovery, collection, and mutation are deliberately excluded.

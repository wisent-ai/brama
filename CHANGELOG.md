# Changelog

All notable Brama changes are documented here. Brama follows Semantic
Versioning and the pre-one compatibility policy in [`RELEASE.md`](RELEASE.md).

## Unreleased

### The subscription pool can be read and refreshed from the CLI

Browser automation across the company stopped for most of a working day because
this pool was empty. Both codex subscription credentials were burnt at the same
time, every `best`-aliased call answered `429 subscription_unavailable`, and the
product would not say which of the two possible reasons was true. The gateway had
known since its first refresh sweep -- the ledger carried `needs_reauthorization`
against both grants with the provider's own sentence beside them -- and the only
way to reach that was to grep `brama-always-on.err` for the code and read
timestamps by hand. Repairing it was worse: a burnt grant is never inside the
refresh timer's skew window, so the timer that exists to replace grants was
precisely the thing that would not touch these, and there was no command to make
it.

`brama subscriptions list` now reports the pool as the gateway sees it: one row
per subscription with its provider, its `state` -- `live`, `expired`, `burnt` or
`unknown` -- the provider's stated expiry, and the refusal standing in its way in
the words of whatever refused it. It is read-only in the strict sense: no
provider is contacted, no capability is redeemed, and the ledger is read without
being written back, so it is safe against a gateway that is serving traffic. The
rows come from the deployment's subscription listing joined to the ledger,
because either source alone hides accounts the other holds -- and a listing that
needs the entitlements router on `PATH` answers nothing at all from the shell an
operator diagnoses an empty pool from.

`brama subscription refresh <provider> --reason <text>` runs the same refresh the
timer runs, for one provider, now, and reports whether a credential was obtained.
It shares the sweep's code path, so a grant cannot come back alive here and dead
there and every refusal is classified once. `--reason` is required, because
rotating a grant invalidates the previous refresh token, and the reason is
appended to the journal beside the verdict. An attempt that cannot proceed says
which of three reasons it was -- no usable subscription in the pool, a provider
whose credentials are API keys and have no refresh path, or no usable credential
source in this environment -- rather than reporting a broken account. A retired
subscription is left alone.

Both commands support `--json` for the desktop console. Neither can print
credential material: the listing reads a ledger that has never held any, and the
refresh drops the credential it obtains without looking at it.

### A credential that never existed is no longer reported as a busy provider

Running the first-use journey on a workstation with no capability and no read
grant produced `429 capacity_error`, `retryable: true`, and the sentence "all
bounded 'codex' credentials unavailable for agent". No provider had been asked
anything. The vault produced nothing to present, which is a broken
authorization chain -- a missing capability, a missing read grant, or missing
installation trust material -- and no amount of waiting mends one.

That pool-emptying reason is now its own answer: `503 authorization_error` with
code `credential_unauthorized` and `retryable: false`, saying which of the
three links is missing. The two other reasons keep their own verdicts: a
provider that refused every credential still answers
`subscription_reauthorization_required`, and a genuinely exhausted pool still
answers `429 subscription_unavailable`, which is the only one of the three that
is worth waiting out.

This is the third time the same shape has been fixed here -- refused
redemption, refused grant, and now absent credential were all reported as
capacity -- so the classification is now keyed on which link broke rather than
on the sentence the last layer happened to write.

### Callers can stream, and a committed stream is never re-run

Brama answered every generation in one piece. A caller waiting on a model
therefore waited for all of it, which made the gateway unusable for anything
interactive and made Brama the only model router on this fleet that could not
do what every provider behind it already does.

`POST /v1/chat/completions` now streams when the request says `"stream": true`,
as `text/event-stream` carrying `chat.completion.chunk` frames closed by
`data: [DONE]`, with a keep-alive comment frame every 15 seconds so an idle
proxy does not close a stream that is legitimately silent while the model
thinks.

The retry contract is what needed deciding, and the boundary is the provider's
response status. Before it commits, everything documented in
[`CORE.md`](CORE.md) still applies: three model candidates for a selector, two
bounded credentials each, one forced OAuth refresh on an auth refusal, the
300-second whole-request deadline, and a refusal is an ordinary error document
because the caller has received nothing. After it commits, nothing is retried
at all. A provider that fails, stalls for more than 255 seconds between reads,
or ends without its terminal event ends the caller's stream without one too --
no `data: [DONE]` -- because a second attempt on another credential would
duplicate both the bill and the emitted text. The whole-request deadline stops
applying at that same moment: a total budget cannot tell a model that is
thinking from a socket that has died, and the per-read timeout can.

Subscription spend is recorded once per stream whichever way it ends, including
a stream the caller abandoned: the tokens the provider reported and the plan
windows its headers carried are ledger facts regardless of who stopped
listening. Kimi streams carry no token meter, because its coding endpoint's
field set is pinned and publishes none; that absence is recorded as absence
rather than filled in with an estimate.

### Anthropic Messages and OpenAI Responses are accepted at the ingress

Brama spoke one dialect, OpenAI chat completions. Every client that speaks
Anthropic Messages or OpenAI Responses -- which includes the agent runtimes
whose subscriptions this gateway holds -- needed a translating proxy in front,
which is one more process holding one more copy of the routing policy.

`POST /v1/messages` and `POST /v1/responses` are now served directly, buffered
or streamed, in the caller's own format: Anthropic `message_start`,
`content_block_*`, `message_delta` and `message_stop` events, or `response.*`
events closed by `response.completed`.

They are one workflow with the chat endpoint, not three. Client identity, model
allowlist, alias resolution, selector semantics, caller-scoped subscription
ownership, attempt bounds, ledger accounting and the error contract are shared
line for line; only the shape of the request and the answer differs. Each still
requires a canonical `provider/model` route or a supported selector, because a
bare vendor model name is not a routing decision Brama is willing to guess.

Inbound translation keeps only what a provider-neutral request can hold. Stop
sequences, cache-control hints, reasoning options, stored-response identifiers
and non-function tool types are accepted and dropped rather than approximated,
and outbound translation states only what the provider actually reported.

### Routing reads the plan windows it already collects

Brama has been reading each subscription's plan windows from the providers'
own free usage reports for a while, and then choosing which subscription to
spend by shuffling. An account the provider said was 95 percent spent was as
likely to be picked as an idle one, so a fleet with four Claude subscriptions
still walked into 429s it could have seen coming.

Selection now orders candidates by what their plans have left, from the ledger
and at no provider cost: a selector orders its model candidates by the freest
usable subscription behind each route, and an explicit route orders its bounded
credentials the same way. Chance now only breaks exact ties, so accounts at
equal utilization still decorrelate. A window whose own reset instant has
passed counts as empty, because the provider's clock says it rolled; a
subscription with no reading counts as free, because its first call writes the
reading that corrects the placement. Quality still outranks quota: inside one
`task:` score, plan headroom is the tiebreak, not the ranking.

### One agent stays on one account for the length of a window

Consecutive requests from one agent could land on a different subscription each
time. Two things were quietly lost: a provider's prompt cache lives behind one
account, so scattering an agent's turns across a pool threw the cache away on
every turn, and one conversation's spend was smeared across every account the
agent owned instead of accumulating where an operator could read it.

The credential that served an agent is now remembered per provider and tried
first on that agent's next request, until the tightest window it reported
resets -- five hours when the provider named no reset, capped at a day. It is a
preference and never a grant: it is consulted after eligibility, skipped for a
credential inside a block or reporting a full window, never overrides
`billingTarget`, and never widens the bounded attempt count. It lives in the
serving process only, because a pin whose window has passed is worth nothing to
the process that starts next.

### A credential is refreshed before it dies, and a refused one says so

Brama refreshed an OAuth grant at exactly one moment: inside a request, after
the local expiry said the token was spent or after the provider rejected it.
Nothing at all happened for a subscription no request reached, so four
credentials whose refresh tokens their providers had already disowned sat in the
vault reading as active for five days while every request that touched them
failed.

Brama now refreshes every active subscription credential that expires within
`BRAMA_CREDENTIAL_REFRESH_SKEW_SECS` (default 300) every
`BRAMA_CREDENTIAL_REFRESH_INTERVAL_SECS` seconds (default 60, `0` disables the
task), single-flighted per subscription so a slow refresh is never started
twice. Refreshing spends no plan quota, because a token endpoint is not a
metered endpoint.

Every refresh failure is now classified rather than logged and forgotten. A
definitive refusal -- `invalid_grant`, `invalid_token`, a revoked or
unauthorized refresh token, or a 401/403 that is not a transport failure -- is
recorded against that subscription, which is left alone until a sign-in replaces
it. A transient failure -- a timeout, a refused connection, any transport
failure -- changes nothing and is retried by the next sweep. A refreshed grant
that cannot be written back to the vault is now a failed refresh rather than a
success with a warning attached: the rotated grant is dropped instead of being
spent from memory, because the provider has already invalidated the one still
stored.

A subscription row therefore carries a new `credential` object: `state`
(`active`, `needs_reauthorization`, `disabled`), the provider's own sentence as
`cause`, and `recorded_at_ms`, `expires_at_ms` and `refreshed_at_ms` when they
are known. The object is absent while nothing has been recorded about a grant,
every field is optional for older readers, and a ledger written by an older
gateway still loads.

### Plan usage is read from the provider, not bought with a completion

A provider states its plan windows in the headers of an answer, so a window used
to exist only for the account that happened to serve a request: of seven
subscriptions on the fleet exactly one reported a plan and the other six were
indistinguishable blanks. The first fix for that spent one deliberately minimal
completion per active subscription every quarter hour -- it bought the statistic
with the quota it was measuring. That timer is gone, and with default
configuration no timer performs a quota-consuming request at all.

Brama now reads each provider's own usage report, which costs no quota:
`claude-code` from `GET /api/oauth/usage` on `api.anthropic.com`, `codex` from
`GET /backend-api/wham/usage` on `chatgpt.com`, and `kimi` from
`GET /coding/v1/usages` on `api.kimi.com`. A provider that publishes no report is
recorded as publishing none, which is a fact about the vendor rather than an
error. Each subscription is read at most once per `BRAMA_PLAN_USAGE_TTL_SECS`
(default 300), spread by up to a quarter either way from the subscription's own
id so seven accounts on one host never fan out into one burst against a provider
that rate-limits usage reads per address, and single-flighted per subscription;
the sweep that notices aged-out rows runs every `BRAMA_PLAN_USAGE_SWEEP_SECS`
(default 60, `0` disables it). A failed read never blanks a row: the last good
reading is kept and served with `stale` true, and is dropped only once it is
older than `BRAMA_PLAN_USAGE_RETENTION_SECS` (default 86400).

A subscription row therefore says where its newest window came from and how fresh
it is: `usage_source` is `provider`, `traffic` or `probe`, and `stale` is true
once the newest reading has aged past the freshness window. Together with the
instant each reading was taken (`recorded_at_ms`), when the record last changed
(`observed_at_ms`) and the newest check verdict (`probe`, now carrying `source`:
`usage_report` or `completion`), a reader can tell apart the reasons a plan can be
blank -- the provider publishes none, nothing has ever gone through this account,
or the credential is being refused -- with the provider's own sentence for the
last one.

The paid check still exists, because a free report cannot say whether a provider
will actually serve a credential. It is now an explicit operator action:
`POST /v1/admin/subscriptions/:agent_id/:subscription_id/probe`, reachable by the
desktop console's identity, spends one minimal completion against one named
subscription and returns the verdict it recorded as `probe`. A subscription inside
a recorded rate-limit block is refused with `409` rather than probed, the probe
rotates to no other credential and retires nothing, and a ledger written by an
older gateway still loads.

### Local inference yields to fleet work

Deployment-owned aliases can now fall back from local vLLM to the same
`TheDrummer/Cydonia-24B-v4.3` model on Featherless. Featherless is a native
OpenAI-compatible direct provider, and per-installation Skarbiec policy now
derives its direct-provider grants from the authoritative Brama control
document instead of an unrelated subscription list.


### OAuth credentials recover from provider rejection

Subscription access grants can be rejected before their local expiry claim.
Brama now forces one refresh-token exchange after a provider authentication
failure, persists the rotated credential, and retries the request once. An
authentication failure that remains after refresh rotates to the next
credential without being mislabeled as a 15-minute rate-limit block.

### Probierz vision calls use the Probierz identity

The `probierz-model-router` bearer is now bound to `probierz`, whose dedicated
request-signing secret is loaded from `probierz-agent-auth`. The two Codex
subscription routes admit that identity, so `any-vision-capable` evaluations no
longer fail after capture with a missing `x-agent-id` header or borrow another
product's signing identity.

### Readiness now answers for the credential chain

`GET /health` is liveness and always was: its own body says
`dependencies: not_probed`. It is nevertheless what every deploy check in this
repository used as proof that a release worked, and on 2026-08-11 it answered
`ok` for a full day from a gateway whose every capability redemption was being
refused. The failure surfaced only when a person asked a model a question.

`GET /readyz` is new and public. It redeems one capability per configured
provider and returns `503` naming the providers that failed, carrying no secret:
only the provider name and whether its credential could be obtained. Deploy
checks and monitors should read it; `/health` proves only that the process is
running.

A refused redemption is also classified honestly now. It was reported as
`429 capacity_error` with code `subscription_unavailable` and `retryable: true`,
while Skarbiec was saying `capability is not issued, has expired, has no uses
left, or its authorization id does not match`. It is now
`503 authorization_error`, code `credential_unauthorized`, `retryable: false`.
No amount of waiting repairs an authorization id that does not match, and the
old contract sent callers into retries and operators into the subscription
catalogue.

### Released under the wrong name

`v0.2.9`, `v0.2.10`, `v0.2.11`, `v0.2.12` and `v0.2.13` were cut from trees that
already declared `0.3.0`, so five published releases carry this breaking change
under patch-looking names. Anyone who upgraded to one of them has a service that
will not start until `bin/provision-skarbiec-trust` has run, and the version
number gave no warning.

A published coordinate is immutable, so the names cannot be corrected. Each of the
five release notes now opens with what the artifact actually contains and what the
operator must do. `scripts/baseline.py` no longer aborts when it meets a release
whose tree disagrees with its name — it reports it, skips it, and keeps looking —
because before that fix one mis-named release froze the baseline eight releases
behind.

### Security

- The release archive no longer contains any signing key. Until now the release
  build ran `scripts/generate-skarbiec-config.mjs`, so every download of one
  archive carried the same Ed25519 workload proof seed in
  `etc/brama-skarbiec/brama-proof.key`, alongside a signed ten-year `policy.json`
  granting provider authentication and request signing, a `trust.json` vouching
  for both, and a `worm-receipt` stub that discarded audit records while the
  policy pinned its digest. The launcher defaulted to exactly that directory,
  which is always present in a published archive.
- Trust material is now generated per installation by
  `bin/provision-skarbiec-trust`, and `bin/start-with-skarbiec` refuses to start
  while any of it is missing rather than falling back to a shared copy. The
  registry pins the absolute path and SHA-256 of the binary allowed to redeem a
  capability, which is knowledge a build machine does not have, so generating it
  on the host is also the only way that pin can be correct.

### Changed

- **Incompatible.** `etc/brama-skarbiec` in the archive now holds only
  `subscriptions.json` and `recipient-public-keys.asc`; the generator ships as
  `libexec/generate-skarbiec-config.mjs` and is not executed at build time. An
  installation that relied on the bundled trust material must run
  `bin/provision-skarbiec-trust` once and re-grant the capabilities that were
  bound to the discarded key. Under the pre-one policy a breaking change advances
  `MINOR` from the published `0.2.5`, so this is `0.3.0`.

### Fixed

- `POST /v1/chat/completions` now accepts OpenAI-compatible `tool_choice` and
  carries forced tool selection through OpenAI Chat, OpenAI Responses,
  Anthropic Messages, and Google GenerateContent adapters instead of rejecting
  the request as an unknown field.
- Documentation asserted that no immutable public release existed while five
  were published. `README.md`, `ONBOARDING.md`, `RELEASE.md`,
  `examples/README.md`, and `examples/recovery/upgrade-and-rollback.md` now give
  the download-and-verify install path instead of sending every reader to build
  from source.
- Documentation no longer restates which release is newest, which source version
  is current, or which version an example targets. Every such literal went stale
  within hours of being written: the first correction of this section named
  `v0.2.5` in prose, and two further versions were cut the same day. Prose now
  points at the two machine-maintained records — `released-surface.json` and the
  GitHub Releases list — and `Cargo.toml` stays the single source of the source
  version rather than being duplicated into a sentence.
- `RELEASE.md` records that a tag alone is not a release, because a tagged
  revision whose release workflow did not finish leaves a coordinate with
  nothing installable behind it. This has now happened twice, at `v0.2.0` and
  again at `v0.2.6`.
- `RELEASE.md` listed an archive layout the release workflow does not produce.
  It now records the real contents, including the bundled Skarbiec entitlements
  router and the launcher's default trust material, and states that bundled
  material as a known gap instead of claiming that credentials are never
  shipped.
- `released-surface.json` recorded `0.2.4` after `0.2.5` was published;
  regenerated with `scripts/baseline.py --write`.

### Qualification

No test, test suite, smoke test, provider call, or deployment validation was
created, modified, or executed while preparing these changes, because explicit
human testing consent was not provided.

## 0.2.5 - 2026-08-05

*Notes reconstructed from Git history; this release was published without them.*

### Changed

- Provider adapters, the OAuth refresh path, and the subscription model catalog
  now share one HTTP client instead of constructing one per request, so
  connections are reused across calls rather than repeating connection and TLS
  setup for every request.
- The operator API source is rustfmt-clean.
- `released-surface.json` was regenerated for the published `0.2.4` release.

### Compatibility and operator action

No HTTP, CLI, MCP, configuration, or journal contract changed. No operator
action is required.

## 0.2.4 - 2026-08-05

*Notes reconstructed from Git history; this release was published without them.*

### Fixed

- The inference routes file is validated on the snapshot path as well, matching
  the validation already applied on the update path.

### Compatibility and operator action

No contract change. No operator action is required.

## 0.2.3 - 2026-08-05

*Notes reconstructed from Git history; this release was published without them.*

### Added

- Desktop control plane: `GET /v1/admin/snapshot`, `PUT /v1/admin/routes`, and
  the `GET`, `POST`, and `DELETE` `/v1/admin/subscriptions/:agent_id` family.
  Only `brama-desktop` may call them, and responses carry identifiers and status
  only; subscription credentials remain write-only.
- `lem` is declared a model-router client.

### Fixed

- A refreshed provider OAuth grant is now used even when it cannot be persisted
  through the local entitlements router. That path previously returned the
  expired credential, so a request could fail with a grant Brama had already
  renewed.
- `scripts/share-service-items.sh` provisions Jeden recipient access.
- Release control fails fast without a qualification record, and downloads
  release assets without requiring `gh` on the host.

### Compatibility and operator action

HTTP additions only; the CLI surface is unchanged. An operator exposing the
desktop control plane must give `brama-desktop` its own bearer and local
Ed25519 workload identity as described in `README.md`.

## 0.2.2 - 2026-08-04

The `v0.2.0` tag produced no release artifacts because its external onboarding
source was not yet pinned for isolated CI builds.
The `v0.2.1` artifacts were built but not qualified or promoted because the
Weles reauthorization reader still expected the retired Skarbiec item shape.

### Added

- Product-contract README, onboarding, core, integration, release, security,
  support, example, and qualification documentation.
- Secret-free CLI and health build identity.
- Stable machine-readable error codes, retryability, and attempt counts.
- Whole-request, selector, credential, output-token, collection, and cost
  acknowledgement limits.
- Public-surface versioning for HTTP, MCP, configuration, state, and CLI
  contracts rather than command names alone.
- Immutable release provenance and digest sidecars.
- Signed immutable Stado release publication, promotion, blue-green rollout,
  centralized status, quarantine, and automatic rollback integration.

### Changed

- Brama is documented as an HTTP gateway and hardware detector, not a local
  inference runtime manager.
- Billable CLI operations require explicit cost acknowledgement.
- Selector routing is bounded to three model candidates and six provider calls;
  explicit subscription routing is bounded to two provider calls.
- The global output-token limit is 32768 and the whole inference request deadline
  is 300 seconds.
- Provider failures use stable normalized HTTP semantics.

### Removed

- The unused `subscriptionDecisionId` request field.
- The undocumented `brama_models` MCP claim. The read-only MCP surface exposes
  `brama_detect` only.

### Security

- No credential, identity, provider, transport, or state fallback was added.
- Routing logs are bounded and prohibit bearer, HMAC, capability, prompt, and
  provider-secret material.

### Compatibility and operator action

These changes break the previous source contract and require version `0.2.0`.
Callers must remove `subscriptionDecisionId`, honor structured error codes and
attempt bounds, and pass explicit cost acknowledgement for billable CLI
operations. No durable journal migration is required.

### Qualification

No test, test suite, smoke test, provider call, or deployment validation was
created, modified, or executed while preparing these changes because explicit
human testing consent was not provided. Release qualification remains blocked.

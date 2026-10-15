# Complete administration lifecycles

Every mutable Brama resource has one owner, one full create/read/update/delete
path, and one equivalent Brama Desktop surface. Back to the
[README](../../README.md).

The desktop bearer may call the administration endpoints; ordinary model
clients may not. Credential values are write-only and never appear in list,
snapshot, stats, readiness, or error responses.

## Route aliases

Read the registry with `GET /v1/admin/snapshot`. Create an alias with:

```http
PUT /v1/admin/routes
Authorization: Bearer <brama-desktop bearer>
Content-Type: application/json

{"alias":"support/chat","primary":"openai/gpt-5.4","fallbacks":["anthropic/claude-sonnet-4-6"]}
```

Send another `PUT` for the same alias to replace its primary and ordered
fallback chain. Delete it with
`DELETE /v1/admin/routes` and `{"alias":"support/chat"}`. Names must use
lowercase ASCII letters, digits, `-`, `_`, `.`, or `/`; every route must be
available and support the alias's request shape; duplicate routes are refused.
The required product aliases cannot be deleted. Brama Desktop exposes the same
create, replace, and delete lifecycle under **Routing**: **Add alias…**, select a
user-owned alias and **Edit this alias…**, or **Delete this alias…**.

## Standalone provider keys

This lifecycle exists only when Brama was started with its standalone
in-memory credential store:

```http
GET /v1/admin/credentials
PUT /v1/admin/credentials
{"provider":"openai","credential":"<new key>"}
DELETE /v1/admin/credentials
{"provider":"openai"}
```

`GET` returns provider names only. The first `PUT` adds the key; another `PUT`
for that provider atomically replaces it; `DELETE` removes it. An empty,
unsupported, local-only, or absent provider is refused without changing the
store. Brama Desktop exposes the same lifecycle under **Subscriptions** →
**Local provider keys**: **Add local key…**, select the provider and **Replace
this provider key…**, or **Remove this provider key…**. The desktop app keeps
the durable copy in macOS Keychain and sends the current set to its private
Brama process over standard input.

## Managed agent subscriptions

List one agent's subscriptions with
`GET /v1/admin/subscriptions/:agent_id`. Add one with:

```http
POST /v1/admin/subscriptions/wisent-app
Authorization: Bearer <brama-desktop bearer>
Content-Type: application/json

{"provider":"openai","label":"primary","api_key":"<credential>"}
```

Without `subscription_id`, Brama uses the deterministic primary subscription for
that agent and provider. Repeating the `POST` replaces that credential in place.
To renew a secondary or other existing subscription, include its exact
`subscription_id` from the listing. An unknown or unowned selection returns
`404 subscription not found`; a provider mismatch returns
`409 selected subscription belongs to a different provider`.
A deliberate provider check is
`POST /v1/admin/subscriptions/:agent_id/:subscription_id/probe`; it performs one
minimal real completion and therefore spends provider quota. Retire the
subscription and its credential with
`DELETE /v1/admin/subscriptions/:agent_id/:subscription_id`. Listing, probing,
and deleting never return the credential.

Brama Desktop exposes this lifecycle under **Subscriptions** → **Managed
agent**: **Connect a subscription** adds an agent/provider subscription,
**Replace this subscription credential…** replaces the credential and optional
label, **Verify with provider…** runs the deliberate one-request probe, and
**Retire this subscription…** removes it. Under **My account**, the same
add-or-replace and retire semantics remain scoped by the signed-in Wisent user
through `GET`/`POST /v1/account/subscriptions` and
`DELETE /v1/account/subscriptions/:subscription_id`; an account can never read
or mutate another user's subscriptions. These calls require both the Supabase
bearer and a server-verified `X-Wisent-Organization-ID`, but switching
organizations does not move, duplicate, or relabel user-owned subscriptions.

## Subscription pool

`GET /v1/admin/subscription-pool` and `brama subscriptions list` expose the
same secret-free pool states. Refresh one provider through
`POST /v1/admin/subscription-pool/refresh` with
`{"provider":"codex","reason":"<operator reason>"}`, or through
`brama subscription refresh codex --reason '<operator reason>'`. Repair a
provider-disowned Claude Code, Codex, or Kimi grant with
`brama subscription sign-in <provider> --by weles --reason '<operator reason>'`; Brama
calls Weles's real `/reauth` trajectory, confirms the exact account row, then
refreshes the grant. Brama Desktop exposes these operations under
**Subscription Pool**. A non-empty reason is required, the result is appended
to the operational journal, and no credential value is returned. Reading and
repairing the pool is described in
[subscription-pool.md](subscription-pool.md).

Three console verbs change membership, each with
`{"subscription_id":"<id>","reason":"<operator reason>"}`:
`POST /v1/admin/subscription-pool/disown` retires a member the console
adopted and removes its stored credential,
`POST /v1/admin/subscription-pool/reinstate` returns a retired member to the
rotation (it still needs a grant of this gateway's own), and
`POST /v1/admin/subscription-pool/forget` drops a member only the usage ledger
remembers. A missing or blank reason is refused with HTTP 400 and
`reason is required: say why you <disown|reinstate|forget> this member`;
nothing is written in its place.

The complete state, error, retry, authorization, and resource contract is in
[`CORE.md`](https://brama.wisent.com/docs/core). Provider capability and lifecycle contracts are in
[`INTEGRATIONS.md`](https://brama.wisent.com/docs/integrations).

## Functional test journeys

The provider-facing tests run the public Brama binary against the real
Skarbiec vault, real provider accounts, real quota, and Weles sign-ins. They
contain no provider server, canned provider response, fake key, dry run, or
smoke-test substitute. Run them inside the launcher environment, with the
source-tree binary named explicitly so Skarbiec binds capabilities to the
binary Cargo executes:

```console
BRAMA_BIN_OVERRIDE="$PWD/target/debug/brama" \
  scripts/start-with-skarbiec.sh --exec "$HOME/.cargo/bin/cargo" \
  test --test admin_real -- --test-threads=1
BRAMA_BIN_OVERRIDE="$PWD/target/debug/brama" \
  scripts/start-with-skarbiec.sh --exec "$HOME/.cargo/bin/cargo" \
  test --test http_api_real -- --test-threads=1
BRAMA_BIN_OVERRIDE="$PWD/target/debug/brama" \
  scripts/start-with-skarbiec.sh --exec "$HOME/.cargo/bin/cargo" \
  test --test capability_real -- --test-threads=1
BRAMA_BIN_OVERRIDE="$PWD/target/debug/brama" \
  scripts/start-with-skarbiec.sh --exec "$HOME/.cargo/bin/cargo" \
  test --test subscription_real -- --test-threads=1
```

`admin_real` reads the real OpenRouter credential through Brama's configured
Skarbiec route without printing it. It then proves the full alias, standalone
key, and managed-subscription lifecycles: add, read or provider probe, replace,
another real completion, delete, and the final refusal. Its subscription uses
a dedicated qualification agent and removes that credential before the test
returns. The same target sends real buffered and streamed requests through the
OpenAI Chat, Anthropic Messages, and OpenAI Responses surfaces, then reads the
resulting model catalogue, readiness, and statistics. `http_api_real` starts
`brama serve` and requires a real authenticated
completion plus Brama's persisted perf record. `capability_real` requires a
real completion funded by each deployment capability. `subscription_real`
requires a real completion, a provider-side OAuth rotation, and a Weles-driven
sign-in for each subscription provider, checking the usage ledger and journal
after every operation.

An expired grant, invalid key, exhausted provider balance, missing Weles token,
or unavailable account fails the corresponding journey with the provider or
product sentence; none is converted into a pass. Lower-level parser and refusal
contracts remain useful tests, but they are not reported as functional evidence.

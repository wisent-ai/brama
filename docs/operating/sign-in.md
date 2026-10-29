# Signing a subscription back in

Back to the [README](../../README.md). Reading the pool first is in
[subscription-pool](subscription-pool.md).

## `brama subscription login <provider> --by weles|hand --reason <text>`

Repairs a provider-disowned `claude-code`, `codex`, or `kimi` grant. `--by` is
required and names who logs in. `--by weles` runs the provider's real login
trajectory through Weles. Before any browser opens,
Brama reads Weles's health contract, resolves the named `login_item` or the one
Weles explicitly declares primary, and verifies an automatically selected row
belongs to the exact subscription being repaired. Success requires Weles to
echo that row and the exact subscription refresh to answer `refreshed`.

```bash
brama subscription login codex --by weles \
  --login-item <login-item> \
  --reason 'provider disowned the stored grant' \
  --json
```

For a Codex grant, a provider request uses `tokens.account_id` when the grant
states it, otherwise the `chatgpt_account_id` claim in `tokens.id_token`.
An absent claim does not invent an account header. A present but empty or
wrongly typed account id, or a malformed identity token, refuses the request
as `credential_unauthorized` with the field or token defect before contacting
OpenAI. The refusal names the damaged field or token on the request path;
sign the affected account in again through the normal flow above rather
than editing a stored grant. `brama subscriptions list --json` and Brama
Desktop's Subscription Pool report the credential state last recorded by
the gateway, which may not yet include a request refused before OpenAI.

`--by hand` signs one `claude-code` subscription in through the operator's own
browser: Brama prints the provider's authorize page
(`https://claude.com/cai/oauth/authorize`), whose redirect is the provider's
own code page (`https://platform.claude.com/oauth/code/callback`), and
redeems and later refreshes the grant at
`https://platform.claude.com/v1/oauth/token`, the addresses Claude Code
2.1.296 itself uses (one declaration of the client serves sign-in and
refresh),
so nothing on the signing-in machine listens for the browser; it reads the
`code#state` that page shows (or the whole redirect URL) on stdin, stores the
grant and proves it with one minimal completion. A host whose `/dev/urandom`
cannot be read refuses the sign-in naming it and the OS error, since the PKCE
verifier would otherwise be guessable; Brama Desktop's hand sign-in answers
that refusal as the gateway's own failure (HTTP 500), and a provider or
subscription the hand sign-in does not serve as a conflict (HTTP 409).
`--subscription-id` is required;
`--gateway-consumer <name>` or `--gateway <origin>` with `--bearer-role <role>`
runs the same two steps on the serving gateway, so a workstation without its
own vault can sign a pool member in. `--login-item` belongs to `--by weles`
and the gateway flags to `--by hand`; a contradicting flag is a usage error.
On a standalone gateway (a launcher's private gateway, such as Jeden's own) the
console's `POST /v1/admin/subscription-pool/sign-in` with `"by": "hand"` may
also name `provider` and `account` for a subscription id no member holds yet:
the gateway then banks that member itself, adding it to the pool just before
the pasted code is exchanged and taking it out again if the exchange fails,
because such a launcher holds no credential to donate it with. A managed
gateway refuses that, one of the two named alone is refused, and an id the pool
already holds is refused with "already in the pool". The id is checked again
when the code is pasted: one a donation or another sign-in banked while the
page was open is refused with "joined the pool while this sign-in's page was
open", and a failed exchange takes out only the member this sign-in banked.

```bash
brama subscription login claude-code --by hand \
  --subscription-id <id> \
  --reason 'the account a person can log into' \
  --gateway-consumer <consumer> --bearer-role <role>
```

A browser sign-in has no clock that ends it: it runs until the provider's pages
answer, and some answers wait for a person. So the command says what the run is
doing while it does it, one line on stderr per event Weles reports (the verdict
stays on stdout):

```text
Weles admitted the sign-in of <account-email> with login <login-item>
Weles run <run-id> started on <weles host> at 2026-10-06T18:10:02Z
2026-10-06T18:10:04Z stage google_email
2026-10-06T18:10:11Z stage google_2fa_check
WAITING FOR THE OPERATOR since 2026-10-06T18:10:12Z: Approve the Google sign-in for <account-email> on your phone. Choose 42 if the phone asks you to match the number. (account <account-email>, request <request-id>, paged). Approved and nothing moved, or no prompt arrived? `weles runs answer <run-id> --approved | --not-received`; to stop waiting, `weles runs cancel <run-id> --detail <why>` (or Weles Desktop > Running)
```

`WAITING FOR THE OPERATOR` means the run stands still until Google's page moves
or the operator answers the run; `NOT paged` in it means no alert channel
took the request, so nobody was told but this line. Approving on the phone
needs no answer: Weles sees Google move on. `weles runs answer <run>
--not-received` makes Weles press Google's "Resend it" (or end the run saying
Google offers none), `--approved` while Google still shows the prompt records
what Google shows, and `weles runs cancel <run> --detail <why>` ends the run.
The verdict records every stage the run reached and the operator
request it waited on. When Weles stops answering before its result, the
verdict is `failed` with `weles_execution_unconfirmed` and names the run, its
host and its last stage, never a bare transport error, with the two commands
that follow it up: `weles runs show <run>` prints what the run last wrote and
`weles runs cancel <run> --detail <why>` ends it. Neither verdict holds the
next sign-in, and both are recorded `retryable: true`: Weles admits a sign-in
of the same account into the run already under way and replays its events, so
the gateway's next maintenance pass reattaches to it, and a run the operator
cancelled (`run_cancelled`) leaves the account to a new run. Readiness lists
such a member in `automatic_sign_in.blocked` without asking an operator to act.
A refusal is recorded with the failure object `{code, stage}` Weles reports
with every refusal. An answer that refuses without one is recorded as
`weles_response_incomplete` at stage `weles_response`, with a `detail` naming
what was missing and the `weles_error` and `weles_stage` Weles did state (null
where it stated none), rather than given a classification Weles did not make.
Brama Desktop shows the
same lines while its sign-in runs (the gateway's sign-in routes answer them as
`application/x-ndjson`, ending in `verdict` or `refused`), and the gateway's
automatic sign-in logs them as `credential_sign_in_progress`.

## Credentials the sign-in uses

Brama and Weles each acquire the `token` of the vault item that plays the role
`brama-weles-reauth` (the one live item tagged `stado:role:brama-weles-reauth`)
under their own workload identities when their service starts. Every secret the
launcher preloads is selected the same way, by role, never by item name: the
model-router bearers by `weles-model-router`, `wisent-backend-model-router`,
`wisent-app-model-router` and `brama-desktop-model-router`, and each product's
request-sign secret by `<product>-agent-auth` (`wisent-app-agent` for Jeden's
runtime identity). A role no item plays, or plays twice, is refused at start
with the role and its tag. `brama launcher field --router ROUTER ROLE
FIELD` prints one field of the item playing ROLE.
Brama receives `BRAMA_WELES_REAUTH_TOKEN` and presents it only to `POST /reauth`; Weles
receives the same field and accepts it only on that route. An operator's
`brama subscription login` on a workstation, where no launcher ran, reads
the same role through `stado credentials get --role brama-weles-reauth --field
token` (`BRAMA_STADO_BIN` names another `stado`), so it uses the vault Stado
resolves for that machine and never a local copy. At every start the
Brama launcher reads `agent_skarbiec_url` from the host's fleet Stado config
while retaining the dedicated `brama-service` identity; a stale endpoint in an
older service-specific config therefore cannot disconnect Brama from the
canonical vault. `BRAMA_WELES_URL` is an explicit override; otherwise Brama
resolves `weles-admission` from Stado's service directory at the moment a
sign-in starts, using the declared `brama` consumer and
`credential-lifecycle` capability. Once the `brama-service` private key is
present in Brama's dedicated GPG home, later starts reuse that exact key instead
of asking Skarbiec to decrypt and import it again. A fresh installation still
acquires the key from Skarbiec and fails closed if that first acquisition cannot
complete. Neither service reads the other's files, the token is never placed in
argv or the journal, and no browser opens on the machine running the Brama
command.

## Automatic sign-in

The in-process refresh sweep renews OAuth grants before expiry. A definitive
provider refusal, a declared OAuth subscription whose vault row yields no
credential, or a historical OAuth item missing its exact Weles account schedules
one Weles sign-in at a time.
Completed browser runs and permanent account-mapping refusals keep their
cooldown in the journal, so a Brama restart does not repeat them; a transient
Weles preflight failure does not consume that cooldown.

A vault item that still names an exact account — `brama:id:`,
`brama:provider:` and `brama:account:` — but lost its `brama:subscription`
mark is outside the pool: discovery does not list it, and `/readyz` reports it
under `unroutable_accounts` with the route that admits it. A row whose item
carries no `brama:provider:` tag has `provider: null` — no provider is named,
not one called unknown — and Brama Desktop titles that decision "Account
<id> names no provider and is outside the pool". The sweep admits it
on its next pass: the mark is restored on the item as it is (no field is
written, no credential imported, a tag that disagrees with the identity is
refused by name), the member is scheduled for its Weles sign-in like a
discovered account, and the independent grant Weles banks is what makes it
routable. The sweep's answer lists each under `recovered` (`id`, `provider`,
`item`) and each it could not admit under `unrecoverable` with the cause — an
item that names no `brama:account:` is refused `names no provider account
(brama:account:), so no login can be resolved for it; record the account on
the item and sign in again`. The same admission runs when the account is
named by hand: `brama subscription login <provider> --by weles
--subscription-id <id> --reason <why>` here or through the gateway
(`POST /v1/admin/subscription-pool/sign-in`, the account and agent routes)
restores the mark before Weles resolves the login, instead of answering
`subscription not found`; an id that is neither in the pool nor recoverable
is refused `Skarbiec lists no subscription <id> for <provider>: neither in the
pool nor as a recoverable account`. Brama Desktop's Subscription Pool sign-in
goes through the same route.

The real functional journeys in `tests/providers/subscription_real.rs` run one
Weles login and one provider refresh for each of Claude Code, Codex, and Kimi.
They pass only when the exact login row is confirmed, the provider returns a
usable grant, the pool leaves `needs_reauthorization`, and Brama records the
`subscription_sign_in` journal entry.

No credential material is printed by either command: the listing reads a ledger
that has never held any, and the refresh drops the credential it obtains without
looking at it.

# Reading and repairing the subscription pool

Back to the [README](../../README.md).

Every `best`-aliased call is paid for by a subscription credential, so an empty
pool stops browser automation across the company. It did: both codex grants were
burnt at the same time, every call answered `429 subscription_unavailable`, and
the state that explained it -- `needs_reauthorization` with the provider's own
sentence beside it -- was reachable only by grepping `brama-always-on.err` for
the code and reading timestamps by hand. Two commands report that pool and repair
it.

## `brama subscriptions list`

Read-only. It contacts no provider, redeems no capability and writes nothing: it
joins the deployment's subscription listing to the usage ledger and states what
is already recorded, so it is safe to run against a gateway that is serving
traffic.

```bash
brama subscriptions list --json
```

```json
{
  "providers": [
    {
      "provider": "codex",
      "subscription_id": "<subscription-id>",
      "state": "burnt",
      "expires_at": null,
      "last_redeem_error": "invalid_grant: refresh token is no longer accepted"
    }
  ]
}
```

`state` is one of four words. `live`: nothing has refused this grant and its
expiry, if it states one at all, is still ahead. `expired`: the recorded expiry
has passed. `burnt`: the provider disowned the grant, or somebody retired the
subscription, and only a sign-in returns it to the pool. `unknown`: nothing has
ever been recorded about this grant, which is not the same statement as a working
one. `expires_at` is the provider's own instant, and `null` for an API key that
states none. `last_redeem_error` is the refusal standing in the way, in the words
of whatever refused it: the credential's own cause first, then a rate-limit block
still in force, then the newest failed check. A lapsed block is deliberately not
reported, because a stale refusal printed beside a `live` grant is what sends an
operator looking for a sign-in nothing needs.

Without `--json` the same report is printed as lines, led by how many credentials
are live -- which is the question that gets the command run.

Each row also says where the member stands, from fields the document already
carries: `retired: <cause> (recorded <instant>)` for a member taken out of the
rotation, the cause and instant being `credential.cause` and
`credential.recorded_at_ms` written with the retirement (`retired by its owning
agent`, a provider's refusal, a reconciliation); `last sign-in: <result> at
<instant>: <detail>` from `sign_in`, the newest journaled sign-in, automatic or
operator-run; `membership: discovered account <account> has not been signed in
by Brama` for a member discovery registered that holds no sign-in; and
`membership: unknown; the retirement journal could not be read: <cause>` from
`credential.journal_error`. Brama Desktop shows the same as its **Membership**
column and section.

The report's head prints `last purchase of <provider>: <result> at <instant>:
<detail>`, followed by who asked (`trigger`), the `reason`, the `shortage`
bought for, the `account`, the `plan_tier` and the `request_id`, for every
provider `providers.json` declares, from the document's `acquisition` object:
the newest journaled acquisition per provider, `null` for one never bought
for. A journaled purchase is `requested` (Weles was asked and has not
answered; nothing more is bought for that provider until it ends),
`acquired` or `failed` (the detail names the step and cause, and `brama
maintain` stops buying for that provider until an operator runs the
acquisition again). A refusal is a decision, not a purchase: it is not
journaled, and `brama subscription acquire` prints it with its code and cause
when it is asked. A gateway whose acquisition journal cannot be read names
that among the report's errors and leaves the object out; the line then reads
`last purchase: the acquisition journal could not be read (see errors)`.

## `brama subscription refresh <provider> --reason <text>`

Runs the refresh the gateway's own timer runs, for one provider, now. A burnt or
expired grant is never inside the timer's skew window, and a timer that will not
try it is exactly why an empty pool stays empty. `--reason` is required because
this rotates a grant -- the provider invalidates the previous refresh token the
moment it issues a new one -- and the reason is appended to the journal beside
the verdict.

```bash
brama subscription refresh codex --reason 'pool empty; every best call answered 429' --json
```

```json
{
  "provider": "codex",
  "attempted": 2,
  "result": "failed",
  "detail": "refreshed no `codex` grant out of 2 tried; <subscription-id>: invalid_grant: refresh token is no longer accepted"
}
```

`attempted` counts the subscriptions a refresh was tried for, so `0` means the
command found nothing to do and `detail` says which of three reasons it was: no
usable subscription for that provider in the pool, a provider whose credentials
are API keys and have no refresh path at all, or no usable credential source in
this environment -- the last being what a shell without the launcher's capability
environment gets, and not a broken account. A retired subscription is never
refreshed, because rotating its grant would put back what somebody removed. The
exit status is non-zero unless a credential was obtained.

## Reading the hosted subscription view

Read the serving gateway's subscription view, including its provider usage
windows, without copying a credential into a shell command:

```bash
brama subscription list --gateway-consumer <consumer> --bearer-role brama-desktop-model-router --json
```

The gateway's origin comes from Stado's service directory and the console
bearer from the vault item that plays the role, read with
`stado credentials get --role`; no item is named. `brama-desktop-model-router`
is the role Brama Desktop reads its gateway bearer by, and the one the
gateway's console verbs accept (`list` and `attribute` answer with it); a
bearer read by another role is refused with HTTP 401. A refusal exits nonzero
with the gateway's own sentence. A missing or stale provider usage report is not
zero usage; read each window's observation and reset times.

## What each account has left

Members are rows; accounts are what the operator pays for, and two members of
one account draw on one quota. The pool document therefore carries
`accounts.capacity`, one object per account the rows attribute, with the
newest reading of every plan window any of its members holds (for Claude the
`five_hour` and `seven_day` windows): the provider's `used_fraction`, the
`remaining_fraction` (the whole limit less it; `null` for a window whose reset
instant has passed and that nothing has read since), `resets_at_ms`,
`read_at_ms` and the `member` it was read off. Beside the windows:
`usable_members`, `refused_members` with the ledger's reason, `blocked_until_ms`,
`live_sessions` (`null` when the lease register could not be read),
`tightest_remaining_fraction` and `available`, whether a session put on the
account now would be served. `brama subscription list` prints it under the
account lines, one line per account and one per window; Brama Desktop shows
it as **Account capacity** on the subscription pool screen. The acquisition
and a session-placing policy read the same object, so what they decide on is
what the operator sees. Brama computes no number the provider did not state:
an account with no reading has an empty `windows`, not a free plan.

What the usage ledger holds, and what loading one that cannot be read does, is
in [the usage ledger page](../usage-ledger.md). Signing a disowned grant back in
is in [sign-in](sign-in.md).

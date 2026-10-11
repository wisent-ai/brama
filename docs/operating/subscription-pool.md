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
brama subscription list --gateway-consumer <consumer> --bearer-role <console bearer role> --json
```

The gateway's origin comes from Stado's service directory and the console
bearer from the vault item that plays the role, read with
`stado credentials get --role`; no item is named. A refusal exits nonzero with
the gateway's own sentence. A missing or stale provider usage report is not
zero usage; read each window's observation and reset times.

What the usage ledger holds, and what loading one that cannot be read does, is
in [the usage ledger page](../usage-ledger.md). Signing a disowned grant back in
is in [sign-in](sign-in.md).

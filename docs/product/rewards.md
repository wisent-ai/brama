# Rewards: what Brama reports and what it gates

Wisent keeps one contribution ledger, in compute.wisent.com: it credits
contributions and sells entitlements for the credits. Brama is one of the
products that report to it and the one that asks it before serving.

A gateway that declares no ledger reports nothing and gates nothing; it
serves exactly as before.

## Declaring the ledger

| setting | meaning |
|---|---|
| `BRAMA_CONTRIBUTION_LEDGER_URL` | the ledger's origin, e.g. `https://compute.example.test` |
| `BRAMA_ENTITLEMENT_GATED_ALIASES` | comma-separated aliases served beyond a credential's model scope only to holders of the `model.access` entitlement whose scope is the alias, e.g. `wisent/sol` |
| Skarbiec route for `contribution-ledger` | the item and field holding the bearer Brama presents as the reporter `brama`; the ledger's operator declares its SHA-256 |

## What Brama reports

`subscription.pooled`, one per UTC day a donated subscription answers a caller
from the shared pool, credited to its donor: the Wisent account id when a
signed-in user donated it (Brama keeps it under that account's agent,
`user-<id>`), else the agent that banked it. The event id is `<subscription>:<day>`, so a day
is one event however many calls it served and however often the gateway
restarted. A report that fails is logged as `contribution_report_failed` with
its cause and is asked again by the day's next call.

## Who the ledger knows

The ledger knows a signed-in Wisent user by their account id, the same id a
donation of theirs is credited to, and a workload caller by the agent it
signed as, or by its client id when it signed as none.

Brama links no identity. A signed-in user links their account once on the
Rewards page of compute.wisent.com with **Link my Brama donations**
(`POST /api/v1/rewards/identities/account` `{"product": "brama"}`), which
reads the account from that sign-in, so nobody can link someone else's
donations. A workload's agent signature or bearer is presented by every
process that holds it, so nothing it sends could show that a person asked:
its donations and calls are credited to no one, and the operator's shell
has no link verb. A donation whose account agent names no account id is
not reported, and `contribution_report_failed` says why.

## Gated aliases

A credential's model scope is unchanged. For a model outside it, Brama asks
the ledger only when the model is a gated alias:

| ledger answer | Brama answers |
|---|---|
| granted | routes the call as any other |
| denied | 403 `entitlement_required`: the scope refusal, then why the ledger denied (`entitlement_absent`, `contributor_not_linked`), with `details.reason`, `details.contributor_id`, `details.scope` |
| no ledger declared | 501 `contribution_ledger_unanswered`, naming `BRAMA_CONTRIBUTION_LEDGER_URL` |
| ledger did not answer | 503 `contribution_ledger_unanswered` with its cause |

A model that is not gated is refused by the scope as before, without asking
the ledger, and a model inside the scope never waits for it.

`brama rewards status [--alias A [--agent-id ID]] [--json]` reports the
declared ledger, whether the alias is gated, and what the ledger answers
about the agent's entitlement. The console reads the same report with
`POST /v1/rewards/status` and `{"alias"?, "agent_id"?}`, which Brama
Desktop's Rewards screen shows; any other caller is refused, `agent_id`
without `alias` is refused, an unreadable
declaration is 503 `contribution_declaration_unreadable`, and a ledger that
did not answer is `contribution_ledger_unanswered` as above.

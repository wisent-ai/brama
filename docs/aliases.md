# What an alias promises

The three kinds of name this gateway answers to, and what each one guarantees a
caller: a text alias, a decision alias and a media alias, plus what the
catalogue says about any model behind them.


An alias is a deployment-owned model name that resolves to exactly one
canonical `provider/model` route. There is no ordered list of spare
destinations behind it: when the route refuses, that refusal is the caller's
answer, carrying the state and the route it was refused for. An operator who
wants a different destination changes the one route.

The route registry is a JSON document with three known top-level fields —
`schema_version`, `deployments`, and `routes`, which maps each alias to its
single destination. An unknown top-level field is refused by name and the whole
document is rejected, so nothing in it serves until it is repaired.
`brama routes migrate` performs that repair in one idempotent operation, and
[`/docs/cli/routes/migrate`](https://brama.wisent.com/docs/cli/routes/migrate)
documents it. The shape, the validation guards, and the atomic owner-only write
are in
[`/docs/configuration/route-registry`](https://brama.wisent.com/docs/configuration/route-registry);
the alias vocabulary and its four states are in
[`/docs/concepts/alias`](https://brama.wisent.com/docs/concepts/alias).

`brama routes set <alias> <destination>` declares an alias and
`brama routes rm <alias>` retires one, through the same validated atomic
owner-only write `PUT /v1/admin/routes` performs, for a host whose gateway is
not running.

## What a decision alias promises

Two names answer typed questions instead of generating text:
`decision-model`, on a route this deployment pays for, and
`best-decision-model`, whose route may be `best`, so the caller's own signed
identity selects the subscription that pays. `POST /v1/decisions` takes a
state and a map of typed questions — a yes/no `noul`, a `choice` over the
options the caller declared, a `score` over the rubric it gave — and answers
one typed answer each, with the distribution it came from.

TypeSafe AI's System One model answers that wire natively and generates no
text at all, so it serves no chat; every other route is served by asking a
chat model for the probability mass over each question's declared labels,
from which Brama computes the answer itself. Either way the answer is inside
the schema the caller declared or it is refused. `brama decide` asks one from
a shell. The vocabulary, the two engines and every refusal are in
[`/docs/concepts/decision`](https://brama.wisent.com/docs/concepts/decision).

## What a media alias promises

Three names generate something that is not text: `image-model` on
`POST /v1/images/generations`, `video-model` on `POST /v1/videos` with
`GET /v1/videos/{id}` reading the job back, and `voice-model` on
`POST /v1/audio/speech`, whose answer is the provider's encoded audio rather
than JSON. Each accepts its own alias or a canonical route whose provider
serves that shape and which the catalogue lists as that kind, so a chat model
named on the image endpoint is refused before a credential is redeemed.

Media is paid by the deployment's own capability. No subscription in the pool
carries an image, video or voice quota, so `best` cannot stand behind a media
alias and the registry refuses one that tries. Brama keeps no job state: a
video identifier belongs to the provider that issued it, so a status read
names the same model that started it. `brama image`, `brama video` and
`brama speak` drive all three from a shell. The shapes, the providers that
serve them and every bound are in
[`/docs/concepts/media`](https://brama.wisent.com/docs/concepts/media).

## What the catalogue says about a model

Every model carries what it produces — `kind` is `text`, `image`, `video` or
`audio`, decided from the output modalities its source declares — and whether
its weights are published: `openWeights` is the catalogue's own flag, and
`null` where the source said nothing, which is not the same claim as `false`.

Categories are the operator's own grouping, declared in the route registry by
provider, by exact route, or by a term the publisher put in the model's name.
`uncensored` is the first one anybody asked for. Brama infers none, because a
gateway that guessed would be publishing an opinion as metadata.
`GET /v1/models` narrows on `kind`, `weights`, `category` and `provider`,
`GET /v1/categories` reports each declaration with how many models it holds,
and `brama models` and `brama categories` are the same surface from a shell.
See [`/docs/concepts/category`](https://brama.wisent.com/docs/concepts/category).


# Quick start

How do you go from no Brama state to one answered request? This page is the
one happy path: install an exact release, run the safe first command,
provision the installation's trust material, start the gateway through its
launcher, and verify it with the two health endpoints and one model listing.
Everything else — client provisioning, production ingress, failure guidance —
lives in [`ONBOARDING.md`](../ONBOARDING.md) and [configuration](configuration.md).

## Install an exact release

There is no `latest` production contract; the version is selected
deliberately, never resolved for you. Download the archive for your platform
and verify its published checksum before extracting:

```bash
curl --fail --silent --proto '=https' --tlsv1.2 \
  https://api.github.com/repos/wisent-ai/brama/releases | grep '"tag_name"'

version=<chosen SemVer, without the v prefix>
platform=darwin-arm64   # or linux-amd64
base="https://github.com/wisent-ai/brama/releases/download/v${version}"
curl --fail --location --proto '=https' --tlsv1.2 --remote-name-all \
  "${base}/brama-v${version}-${platform}.tar.gz" \
  "${base}/brama-v${version}-${platform}.tar.gz.sha256"
shasum -a 256 --check "brama-v${version}-${platform}.tar.gz.sha256"
tar -xzf "brama-v${version}-${platform}.tar.gz"
```

Maintainers working on unreleased source run the same commands from a
checkout with `cargo run --locked --` instead of `./bin/brama`.

## Run the safe first command

```bash
./bin/brama detect
```

`detect` reads local hardware, performs no provider request, reads no
credential, creates no Brama state, and incurs no model cost. Expected output
contains these labels with host-specific values:

```text
GPU Type: ...
VRAM: ... GB
RAM: ... GB
CPU Cores: ...
Recommended model: ...
Recommended backend: ...
```

`./bin/brama version` prints one JSON object with secret-free product and
build identity. Neither command starts a service.

## Provision trust material once

Serving traffic takes more than the archive. The release ships no signing
key: the workload registry pins the absolute path and SHA-256 of the binary
allowed to redeem a capability, and a build machine cannot know either.
Provision this installation once:

```bash
bin/provision-skarbiec-trust
```

This writes the signed policy, workload registry, trust root, receipt
command, and workload proof key. Re-running it replaces the installation's
identity, so it refuses without `--force`. The launcher refuses to start
until this material exists.

## Start the gateway

```bash
scripts/start-with-skarbiec.sh
```

The launcher is the supported start path: it materializes the vault, starts
the owner-bound capability socket, reads client bearers and request-sign
identities from their Skarbiec items, assembles `BRAMA_MODEL_ALIASES` and the
capability maps, and finally `exec`s `brama serve --port 8080` (override with
`BRAMA_PORT_OVERRIDE` or `PORT`). Starting the binary directly fails closed:
`brama serve` refuses when the generated configuration is absent, and the
error names the launcher as the repair. The gateway binds loopback unless
`BRAMA_BIND_ADDRESS` names an IP address.

Standalone Brama Desktop instead launches its bundled binary with
`brama serve --local-credentials-stdin`, passing a provider-to-credential
JSON object once over standard input; the credentials live only in zeroizing
process memory.

## Verify liveness, then readiness

```bash
curl -s http://127.0.0.1:8080/health
```

`/health` proves only that the process is up; its body says
`"dependencies": "not_probed"`. Do not stop here — it answers `ok` even when
every credential redemption is refused.

```bash
curl -s http://127.0.0.1:8080/readyz
```

`/readyz` redeems one capability per configured provider and one credential
per active subscription, and is the only evidence the gateway can serve. A
`503` body names the providers or subscriptions that failed, without secret
material. See [http-api](http-api.md) for the full distinction.

## First authenticated request

`GET /v1/models` is the first non-billable machine workflow:

```bash
curl -s -H "Authorization: Bearer $BRAMA_CLIENT_TOKEN" \
  http://127.0.0.1:8080/v1/models
```

Then one completion — this spends real provider quota:

```bash
curl -s -H "Authorization: Bearer $BRAMA_CLIENT_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"model": "wisent-backend/chat/primary",
       "messages": [{"role": "user", "content": "Say hello in one sentence."}]}' \
  http://127.0.0.1:8080/v1/chat/completions
```

The response is one OpenAI-compatible `chat.completion` document; add
`"stream": true` for server-sent events. The model name must be an alias your
bearer's allowlist permits, a canonical `provider/model` route, or a selector
— the vocabulary is in [clients and aliases](clients-and-aliases.md), and the
full request contract in [http-api](http-api.md).

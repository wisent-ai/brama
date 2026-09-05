#!/bin/sh
// 2>/dev/null; exec /usr/bin/env PATH="/opt/homebrew/bin:/usr/local/bin:$PATH" node "$0" "$@"
//
// The two lines above are a shell header, not JavaScript: an installed helper is
// executed through sh, which cannot read a module, and a helper runs with a
// minimal PATH that `env node` alone would miss.

// Report which subscriptions the gateway will actually spend, per agent.
//
// Counting items in the vault answers a different question from the one that
// matters: a credential is spendable only by an agent the item is tagged for,
// so a vault holding six healthy credentials can serve exactly one route. The
// only honest measure is to ask the gateway as each agent it recognises.
//
// This runs on the gateway's own host, so no signing secret leaves it, and the
// identity sources are the ones the launcher projects into the gateway.
//
// Read-only: sends a fixed one-word prompt, prints statuses and model names.

import crypto from 'node:crypto';
import os from 'node:os';
import { execFileSync } from 'node:child_process';

const HOME = os.homedir();
const CLI = `${HOME}/.stado/bin/skarbiec`;
const ENV = {
  ...process.env,
  SKARBIEC_VAULT_FILE: process.env.SKARBIEC_VAULT_FILE || `${HOME}/.stado/skarbiec.vault.json`,
  PATH: '/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin',
};
const GATEWAY = process.env.BRAMA_URL || 'http://127.0.0.1:8080';
const ROUNDS = 6;
const MAX_TOKENS = 8;
const MILLIS_PER_SECOND = 1000;
const OK = 200;

// Exactly the map `start-with-skarbiec.sh` projects, so this measures the
// identities the gateway really accepts rather than ones invented here. The
// bearer is separate from the signature and both are required: the bearer says
// which client is calling, the signature says which agent it acts as, and
// sending only one gets a bare 401 that names neither.
const SOURCES = {
  'wisent-app': { item: 'agent:wisent-app', field: 'value', bearer: 'jeden-model-router' },
  lem: { item: 'lem-agent-auth', field: 'agent_auth_secret', bearer: 'lem-model-router' },
  probierz: { item: 'probierz-agent-auth', field: 'agent_auth_secret', bearer: 'lem-model-router' },
  echo: { item: 'echo-agent-auth', field: 'agent_auth_secret', bearer: 'echo-model-router' },
};

const read = (item, field) => {
  try {
    return JSON.parse(execFileSync(CLI, ['get', item], { env: ENV, encoding: 'utf8' })).fields[field];
  } catch {
    return null;
  }
};

console.log(`gateway: ${GATEWAY}`);

for (const [agent, source] of Object.entries(SOURCES)) {
  const secret = read(source.item, source.field);
  const bearer = read(source.bearer, 'token');
  if (!secret) {
    console.log(`\n${agent}: no signing secret in this vault (${source.item})`);
    continue;
  }
  if (!bearer) {
    console.log(`\n${agent}: no client bearer in this vault (${source.bearer})`);
    continue;
  }

  const served = new Map();
  let refusal = null;

  for (let round = 0; round < ROUNDS; round += 1) {
    const body = JSON.stringify({
      model: 'any',
      messages: [{ role: 'user', content: 'Say OK' }],
      max_tokens: MAX_TOKENS,
    });
    const stamp = String(Math.floor(Date.now() / MILLIS_PER_SECOND));
    const signature = crypto
      .createHmac('sha256', secret)
      .update(`${agent}:${stamp}:${crypto.createHash('sha256').update(body).digest('hex')}`)
      .digest('hex');
    const response = await fetch(`${GATEWAY}/v1/chat/completions`, {
      method: 'POST',
      headers: {
        authorization: `Bearer ${bearer}`,
        'x-agent-id': agent,
        'x-agent-timestamp': stamp,
        'x-agent-signature': signature,
        'content-type': 'application/json',
      },
      body,
    });
    const payload = await response.json().catch(() => ({}));
    if (response.status === OK) {
      const name = String(payload.model || 'unknown');
      served.set(name, (served.get(name) ?? 0) + 1);
    } else {
      refusal = `${response.status} ${String(payload.error?.message ?? '').slice(0, 140)}`;
    }
  }

  console.log(`\n${agent}:`);
  for (const [name, count] of served) console.log(`  ok x${count}  ${name}`);
  if (refusal) console.log(`  refused: ${refusal}`);
  if (!served.size && !refusal) console.log('  no answer at all');
}

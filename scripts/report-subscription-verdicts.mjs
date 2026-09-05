#!/bin/sh
// 2>/dev/null; exec /usr/bin/env PATH="/opt/homebrew/bin:/usr/local/bin:$PATH" node "$0" "$@"
//
// The two lines above are a shell header, not JavaScript: an installed helper is
// executed through sh, which cannot read a module, and a helper runs with a
// minimal PATH that `env node` alone would miss.

// Ask the gateway, one subscription at a time, whether it can spend it.
//
// The `any` selector returns on the first model that answers, so a fleet whose
// first pick always works looks like a fleet with one subscription. Pinning the
// billing target forces the dispatcher onto exactly one credential, which is
// the only way to get a verdict per subscription rather than per request.
//
// Runs on the gateway's host so no signing secret leaves it.
//
// Read-only: one fixed one-word prompt per subscription.

import crypto from 'node:crypto';
import os from 'node:os';
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';

const HOME = os.homedir();
const CLI = `${HOME}/.stado/bin/skarbiec`;
const ENV = {
  ...process.env,
  SKARBIEC_VAULT_FILE: process.env.SKARBIEC_VAULT_FILE || `${HOME}/.stado/skarbiec.vault.json`,
  PATH: '/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin',
};
const TRUST = process.env.BRAMA_SKARBIEC_CONFIG_DIR || `${HOME}/.config/brama/trust`;
const GATEWAY = process.env.BRAMA_URL || 'http://127.0.0.1:8080';
const AGENT = process.env.PROBE_AGENT || 'wisent-app';
const MAX_TOKENS = 8;
const MILLIS_PER_SECOND = 1000;
const OK = 200;

const read = (item, field) => {
  try {
    return JSON.parse(execFileSync(CLI, ['get', item], { env: ENV, encoding: 'utf8' })).fields[field];
  } catch {
    return null;
  }
};

const secret = AGENT === 'wisent-app'
  ? read('agent:wisent-app', 'value')
  : read(`${AGENT}-agent-auth`, 'agent_auth_secret');
const bearer = read(AGENT === 'wisent-app' ? 'jeden-model-router' : `${AGENT}-model-router`, 'token')
  ?? read('lem-model-router', 'token');
if (!secret || !bearer) {
  console.log(`cannot sign as ${AGENT}: ${secret ? 'no client bearer' : 'no signing secret'}`);
  process.exit(1);
}

const policy = JSON.parse(readFileSync(`${TRUST}/policy.json`, 'utf8'));
const allowed = new Set(
  (policy.roles?.['brama-runtime'] ?? [])
    .filter((rule) => rule && rule.purpose === 'brama.provider.authenticate')
    .map((rule) => rule.resource)
    .filter((resource) => typeof resource === 'string' && resource.split(':').length === 3),
);
const items = JSON.parse(readFileSync(ENV.SKARBIEC_VAULT_FILE, 'utf8')).items ?? {};
const banked = Object.keys(items)
  .filter((name) => name.includes(':brama-sub-') && !items[name].deleted)
  .sort();

console.log(`agent:   ${AGENT}`);
console.log(`gateway: ${GATEWAY}`);

for (const name of banked) {
  const [, provider, subscription] = name.split(':');
  const body = JSON.stringify({
    model: 'any',
    messages: [{ role: 'user', content: 'Say OK' }],
    max_tokens: MAX_TOKENS,
    billingTarget: { providerId: provider, accountId: 'wisent-app', subscriptionId: subscription },
  });
  const stamp = String(Math.floor(Date.now() / MILLIS_PER_SECOND));
  const signature = crypto
    .createHmac('sha256', secret)
    .update(`${AGENT}:${stamp}:${crypto.createHash('sha256').update(body).digest('hex')}`)
    .digest('hex');
  const response = await fetch(`${GATEWAY}/v1/chat/completions`, {
    method: 'POST',
    headers: {
      authorization: `Bearer ${bearer}`,
      'x-agent-id': AGENT,
      'x-agent-timestamp': stamp,
      'x-agent-signature': signature,
      'content-type': 'application/json',
    },
    body,
  });
  const payload = await response.json().catch(() => ({}));
  const label = subscription.padEnd(40);
  const policyMark = allowed.has(name) ? '' : '  [not in policy]';
  if (response.status === OK) {
    console.log(`  SPENDS   ${label} ${payload.model ?? ''}${policyMark}`);
  } else {
    console.log(`  refused  ${label} ${response.status} ${String(payload.error?.message ?? '').slice(0, 90)}${policyMark}`);
  }
}

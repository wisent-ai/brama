#!/bin/sh
// 2>/dev/null; exec /usr/bin/env PATH="/opt/homebrew/bin:/usr/local/bin:$PATH" node "$0" "$@"
//
// The two lines above are a shell header, not JavaScript: an installed helper is
// executed through sh, which cannot read a module, and a helper runs with a
// minimal PATH that `env node` alone would miss.

// Ask Anthropic why it rejects the Claude refresh, in its own words.
//
// The gateway records `OAuth refresh rejected with HTTP 400` and nothing else,
// and 400 is ambiguous in the way that matters: a dead refresh token and a
// malformed request look identical from the status alone. One says the
// subscription needs a new sign-in, the other says the gateway has a bug, and
// they call for opposite work.
//
// This repeats exactly the request `gateway/oauth_refresh.rs` makes for
// claude-code -- same endpoint, same client id, same JSON body -- and prints the
// provider's error code. On success it prints which fields came back, never
// their values, and writes nothing anywhere.

import os from 'node:os';
import { execFileSync } from 'node:child_process';

const HOME = os.homedir();
const CLI = `${HOME}/.stado/bin/skarbiec`;
const ENV = {
  ...process.env,
  SKARBIEC_VAULT_FILE: process.env.SKARBIEC_VAULT_FILE || `${HOME}/.stado/skarbiec.vault.json`,
  PATH: '/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin',
};
// Taken from src/gateway/oauth_refresh.rs so this probe cannot drift from what
// the gateway actually sends.
const ENDPOINT = 'https://claude.ai/v1/oauth/token';
const CLIENT_ID = '9d1c250a-e61b-44d9-88ed-5944d1962f5e';
const ITEMS = [
  'provider:claude-code:brama-sub-wisent-app-claude-primary',
  'provider:claude-code:brama-sub-wisent-app-claude-1',
  'provider:claude-code:brama-sub-wisent-app-claude-2',
];

for (const item of ITEMS) {
  let blob;
  try {
    const document = JSON.parse(execFileSync(CLI, ['get', item], { env: ENV, encoding: 'utf8' }));
    const value = document.fields.value;
    blob = typeof value === 'string' ? JSON.parse(value) : value;
  } catch (error) {
    console.log(`${item}: unreadable (${error.code || error.name})`);
    continue;
  }
  const refresh = blob?.claudeAiOauth?.refreshToken;
  if (typeof refresh !== 'string' || !refresh) {
    console.log(`${item}: carries no claudeAiOauth.refreshToken`);
    continue;
  }

  const response = await fetch(ENDPOINT, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ grant_type: 'refresh_token', refresh_token: refresh, client_id: CLIENT_ID }),
  });
  const text = await response.text();
  let parsed;
  try {
    parsed = JSON.parse(text);
  } catch {
    parsed = null;
  }
  const name = item.split(':').pop();
  if (response.ok) {
    console.log(`${name}: HTTP ${response.status} refreshed; fields ${Object.keys(parsed ?? {}).sort().join(',')}`);
  } else {
    const code = parsed?.error ?? '(no error field)';
    const detail = String(parsed?.error_description ?? text).slice(0, 160);
    console.log(`${name}: HTTP ${response.status} ${code} -- ${detail}`);
  }
}

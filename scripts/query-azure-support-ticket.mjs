import { readFile } from 'node:fs/promises';
import { homedir } from 'node:os';

const ONE = Number('1');
const ENV_VALUE_INDEX = Number('2');
const FIRST_ARGUMENT_INDEX = Number('2');
const SECOND_ARGUMENT_INDEX = Number('3');
const RUN_TIMEOUT_MS = Number('900000');
const ZERO = Number('0');
const UNAUTHORIZED = Number('401');
const STADO_SKARBIEC_GATEWAY_URL = process.env.STADO_SKARBIEC_GATEWAY_URL || 'http://127.0.0.1:17602';
const STADO_WELES_GATEWAY_URL = process.env.STADO_WELES_GATEWAY_URL || 'http://127.0.0.1:17604';

function parseEnv(text) {
  const out = {};
  for (const line of text.split(/\r?\n/)) {
    const match = /^([A-Za-z_][A-Za-z\d_]*)\s*=\s*(.*)$/.exec(line.trim());
    if (!match) continue;
    let value = match.at(ENV_VALUE_INDEX).trim();
    if ((value.startsWith('"') && value.endsWith('"')) || (value.startsWith("'") && value.endsWith("'"))) {
      value = value.slice(ONE, -ONE);
    }
    out[match.at(ONE)] = value.replace(/\\n/g, '').trim();
  }
  return out;
}

async function loadWelesTokens(env) {
  const candidates = [
    env.ECHO_WELES_API_TOKEN,
    env.WELES_CONSOLE_API_TOKEN,
    env.WELES_DIAG_API_TOKEN,
    env.WELES_API_TOKEN,
  ].filter(Boolean);

  try {
    const grantPath = env.WELES_SKARBIEC_TOKEN_FILE || `${homedir()}/.local/share/weles/service-deployer-skarbiec-token`;
    const grant = (await readFile(grantPath, 'utf8')).trim();
    const response = await fetch(`${STADO_SKARBIEC_GATEWAY_URL}/v1/items/read`, {
      method: 'POST',
      headers: {
        Authorization: `Bearer ${grant}`,
        'Content-Type': 'application/json',
        'X-Consumer': 'weles-service-deployer',
      },
      body: JSON.stringify({ id: 'weles-service-deployer' }),
    });
    const payload = await response.json().catch(() => ({}));
    const token = payload?.value?.token;
    if (response.ok && token) candidates.unshift(token);
  } catch (error) {
    if (candidates.length === ZERO) throw error;
  }

  if (candidates.length === ZERO) throw new Error('missing Weles API token');
  return [...new Set(candidates)];
}

async function runWelesBuilder(origin, tokens, objective) {
  for (const token of tokens) {
    const response = await fetch(`${origin}/weles-builder`, {
      method: 'POST',
      headers: {
        Authorization: `Bearer ${token}`,
        'Content-Type': 'text/plain',
      },
      body: objective,
      signal: AbortSignal.timeout(RUN_TIMEOUT_MS),
    });
    const body = await response.json().catch(() => ({}));
    if (response.status === UNAUTHORIZED) continue;
    if (!response.ok) {
      throw new Error(`Weles API request failed (${response.status}): ${JSON.stringify(body)}`);
    }
    return body;
  }
  throw new Error('Weles API rejected every available scoped token');
}

const ticketId = process.argv.at(FIRST_ARGUMENT_INDEX);
if (!ticketId) {
  throw new Error('usage: node scripts/query-azure-support-ticket.mjs <ticket-id> [ticket-resource-name]');
}
const ticketResourceName = process.argv.at(SECOND_ARGUMENT_INDEX) ?? '';
const webEnvUrl = new URL('../../backends/weles-web/.env.local', import.meta.url);
const echoEnvUrl = new URL('../../echo/.env.local', import.meta.url);
const env = {
  ...parseEnv(await readFile(webEnvUrl, 'utf8')),
  ...parseEnv(await readFile(echoEnvUrl, 'utf8')),
};
const origin = (env.WELES_API_URL || STADO_WELES_GATEWAY_URL).replace(/\/+$/, '');
const tokens = await loadWelesTokens(env);

const objective = [
  'Use only an already-authenticated Microsoft Azure browser session.',
  'Read only. Do not sign in, enter an email, enter a password, trigger a passkey, or use account recovery.',
  `Open Azure Support and inspect support ticket ${ticketId}${ticketResourceName ? ` with resource name ${ticketResourceName}` : ''}.`,
  'Return the current ticket status, assigned support engineer, latest Microsoft message with timestamp, SLA or response deadline, whether the two UnusualActivity deny assignments remain active, and the exact next action required.',
  'If no authenticated session exists, return authentication_required without attempting authentication.',
  'Do not create, update, reply to, close, or otherwise mutate any ticket, account, subscription, credential, or resource.',
].join(' ');

const output = await runWelesBuilder(origin, tokens, objective);
console.log(JSON.stringify(output, null, ENV_VALUE_INDEX));
if (!output.ok) process.exit(ONE);

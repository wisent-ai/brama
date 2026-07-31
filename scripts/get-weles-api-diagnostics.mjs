import { readFile } from 'node:fs/promises';
import { homedir } from 'node:os';

const ZERO = Number('0');
const ONE = Number('1');
const ENV_VALUE_INDEX = Number('2');
const RUN_ID_INDEX = Number('2');
const FILE_PATH_INDEX = Number('3');
const UNAUTHORIZED = Number('401');
const REQUEST_TIMEOUT_MS = Number('30000');

function parseEnv(text) {
  const out = {};
  for (const line of text.split(/\r?\n/)) {
    const match = /^([A-Za-z_][A-Za-z\d_]*)\s*=\s*(.*)$/.exec(line.trim());
    if (!match) continue;
    let value = match.at(ENV_VALUE_INDEX).trim();
    if ((value.startsWith('"') && value.endsWith('"')) || (value.startsWith("'") && value.endsWith("'"))) value = value.slice(ONE, -ONE);
    out[match.at(ONE)] = value.replace(/\\n/g, '').trim();
  }
  return out;
}

async function tokens(env) {
  const out = [env.WELES_CONSOLE_API_TOKEN, env.WELES_DIAG_API_TOKEN, env.WELES_API_TOKEN].filter(Boolean);
  const grant = (await readFile(`${homedir()}/.stado/weles-service-deployer-skarbiec-token`, 'utf8')).trim();
  const response = await fetch('http://127.0.0.1:8787/v1/items/read', {
    method: 'POST',
    headers: { Authorization: `Bearer ${grant}`, 'Content-Type': 'application/json', 'X-Consumer': 'weles-service-deployer' },
    body: JSON.stringify({ id: 'weles-service-deployer' }),
  });
  const payload = await response.json().catch(() => ({}));
  if (response.ok && payload?.value?.token) out.unshift(payload.value.token);
  return [...new Set(out)];
}

const runId = process.argv.at(RUN_ID_INDEX);
const filePath = process.argv.at(FILE_PATH_INDEX);
if (!runId) throw new Error('usage: node scripts/get-weles-api-diagnostics.mjs <run-id> [file-path]');
const webEnvUrl = new URL('../../backends/weles-web/.env.local', import.meta.url);
const echoEnvUrl = new URL('../../echo/.env.local', import.meta.url);
const env = { ...parseEnv(await readFile(webEnvUrl, 'utf8')), ...parseEnv(await readFile(echoEnvUrl, 'utf8')) };
const origin = (env.ECHO_WELES_API_URL || env.WELES_MAC_MINI_API_URL || 'http://100.120.25.24:8788').replace(/\/+$/, '');

for (const token of await tokens(env)) {
  const path = filePath
    ? `/diagnostics/${encodeURIComponent(runId)}/file?path=${encodeURIComponent(filePath)}`
    : `/diagnostics/${encodeURIComponent(runId)}`;
  const response = await fetch(`${origin}${path}`, {
    headers: { Authorization: `Bearer ${token}` },
    signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
  });
  const text = await response.text();
  if (response.status === UNAUTHORIZED) continue;
  if (filePath) {
    process.stdout.write(text);
    if (!text.endsWith('\n')) process.stdout.write('\n');
    if (!response.ok) process.exit(ONE);
    process.exit(ZERO);
  }
  const body = JSON.parse(text);
  console.log(JSON.stringify({ http_status: response.status, ...body }, null, ENV_VALUE_INDEX));
  if (!response.ok || !body.ok) process.exit(ONE);
  process.exit(ZERO);
}
throw new Error('Weles API rejected every available scoped token');

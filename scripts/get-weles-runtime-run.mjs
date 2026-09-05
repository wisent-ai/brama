import { readFile } from 'node:fs/promises';

const ZERO = Number('0');
const ONE = Number('1');
const ENV_VALUE_INDEX = Number('2');
const RUN_ID_INDEX = Number('2');
const POLL_INTERVAL_MS = Number('5000');
const RUN_TIMEOUT_MS = Number('900000');

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

function terminal(status) {
  return new Set(['completed', 'failed', 'cancelled', 'rejected', 'approved', 'pending_review']).has(status);
}

async function getRun(origin, token, runId) {
  const select = 'id,status,params,result,error,scheduled_at,started_at,completed_at,claimed_at,claimed_by';
  const response = await fetch(`${origin}/rest/v1/account_action_logs?id=eq.${encodeURIComponent(runId)}&select=${select}&limit=1`, {
    headers: { apikey: token, Authorization: `Bearer ${token}` },
    signal: AbortSignal.timeout(POLL_INTERVAL_MS),
  });
  const body = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(`Weles runtime API failed (${response.status}): ${JSON.stringify(body)}`);
  const run = body.at(ZERO);
  if (!run) throw new Error(`Weles run not found: ${runId}`);
  return run;
}

const runId = process.argv.at(RUN_ID_INDEX);
if (!runId) throw new Error('usage: node scripts/get-weles-runtime-run.mjs <run-id>');
const envUrl = new URL('../../echo/.env.local', import.meta.url);
const env = parseEnv(await readFile(envUrl, 'utf8'));
const origin = (env.WELES_SUPABASE_URL || '').replace(/\/+$/, '');
const token = env.WELES_SUPABASE_SERVICE_ROLE_KEY;
if (!origin || !token) throw new Error('missing Weles runtime API credentials');

const deadline = Date.now() + RUN_TIMEOUT_MS;
let run = await getRun(origin, token, runId);
while (!terminal(run.status) && Date.now() < deadline) {
  await new Promise((resolve) => setTimeout(resolve, POLL_INTERVAL_MS));
  run = await getRun(origin, token, runId);
}
console.log(JSON.stringify({
  run_id: run.id,
  status: run.status,
  scheduled_at: run.scheduled_at,
  started_at: run.started_at,
  completed_at: run.completed_at,
  claimed_at: run.claimed_at,
  claimed_by: run.claimed_by,
  error: run.error,
  result: run.result,
}, null, ENV_VALUE_INDEX));
if (!terminal(run.status)) process.exit(Number('2'));
if (!new Set(['completed', 'approved', 'pending_review']).has(run.status)) process.exit(ONE);
